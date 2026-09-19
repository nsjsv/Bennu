use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;
use tokio_util::sync::CancellationToken;

use super::super::FileOperationControls;
use super::{inspect_file_identity, FileIdentity, FileObjectKind, RecoverableTransferError};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceManifestEntry {
    #[serde(with = "super::path_codec")]
    pub relative_path: PathBuf,
    pub identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceManifest {
    #[serde(with = "super::path_codec")]
    pub root: PathBuf,
    pub entries: Vec<SourceManifestEntry>,
}

#[cfg(test)]
pub async fn build_source_manifest(
    root: &Path,
) -> Result<SourceManifest, RecoverableTransferError> {
    let mut controls = FileOperationControls::running(CancellationToken::new());
    build_source_manifest_with_controls(root, &mut controls).await
}

pub async fn build_source_manifest_with_controls(
    root: &Path,
    controls: &mut FileOperationControls,
) -> Result<SourceManifest, RecoverableTransferError> {
    controls.wait_until_running().await?;
    let root_identity = inspect_file_identity(root).await?;
    let mut entries = vec![SourceManifestEntry {
        relative_path: PathBuf::new(),
        identity: root_identity.clone(),
    }];

    if root_identity.object_kind != FileObjectKind::Directory {
        controls.wait_until_running().await?;
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        controls.wait_until_running().await?;
        return Ok(SourceManifest {
            root: root.to_path_buf(),
            entries,
        });
    }

    // 并行目录扫描:每个目录(前后身份复核 + read_dir + 逐子项检查)是独立
    // 扫描单元;目录间互不依赖,manifest 最终统一排序(authoritative),
    // 条目收集顺序不影响结果。海量小文件树上的 lstat 风暴是 staging 的
    // 主要剩余开销,单遍扫描并行化即可摊薄所有后续重验路径。
    let scan_workers = std::thread::available_parallelism()
        .map_or(4, |count| count.get())
        .clamp(1, 8);

    struct DirectoryScanJob {
        path: PathBuf,
        relative_path: PathBuf,
        expected_identity: FileIdentity,
    }

    struct DirectoryScanResult {
        entries: Vec<SourceManifestEntry>,
        subdirectories: Vec<DirectoryScanJob>,
    }

    async fn scan_directory(
        job: DirectoryScanJob,
        controls: FileOperationControls,
        result_tx: tokio::sync::mpsc::UnboundedSender<
            Result<DirectoryScanResult, RecoverableTransferError>,
        >,
    ) -> bool {
        let mut controls = controls;
        let scan_result = async {
            controls.wait_until_running().await?;
            let before = inspect_file_identity(&job.path).await?;
            if before != job.expected_identity {
                return Err(RecoverableTransferError::SourceChanged { path: job.path });
            }

            let mut reader = fs::read_dir(&job.path).await.map_err(|source| {
                RecoverableTransferError::file_system("read directory", &job.path, source)
            })?;
            let mut entries = Vec::new();
            let mut subdirectories = Vec::new();
            while let Some(child) = reader.next_entry().await.map_err(|source| {
                RecoverableTransferError::file_system("read directory entry in", &job.path, source)
            })? {
                controls.wait_until_running().await?;
                let relative_path = job.relative_path.join(child.file_name());
                let child_path = child.path();
                let identity = inspect_file_identity(&child_path).await?;
                entries.push(SourceManifestEntry {
                    relative_path: relative_path.clone(),
                    identity: identity.clone(),
                });
                if identity.object_kind == FileObjectKind::Directory {
                    subdirectories.push(DirectoryScanJob {
                        path: child_path,
                        relative_path,
                        expected_identity: identity,
                    });
                }
            }

            let after = inspect_file_identity(&job.path).await?;
            if after != before {
                return Err(RecoverableTransferError::SourceChanged { path: job.path });
            }
            Ok(DirectoryScanResult {
                entries,
                subdirectories,
            })
        }
        .await;

        let result = match scan_result {
            Ok(result) => result,
            Err(error) => return result_tx.send(Err(error)).is_ok(),
        };
        result_tx.send(Ok(result)).is_ok()
    }

    let (job_tx, job_rx) = tokio::sync::mpsc::channel::<DirectoryScanJob>(scan_workers);
    let job_rx = Arc::new(tokio::sync::Mutex::new(job_rx));
    let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel::<
        Result<DirectoryScanResult, RecoverableTransferError>,
    >();
    let mut workers = Vec::with_capacity(scan_workers);
    for _ in 0..scan_workers {
        let job_rx = job_rx.clone();
        let controls = controls.clone();
        let result_tx = result_tx.clone();
        workers.push(tokio::spawn(async move {
            loop {
                let job = {
                    let mut receiver = job_rx.lock().await;
                    receiver.recv().await
                };
                let Some(job) = job else { break };
                if !scan_directory(job, controls.clone(), result_tx.clone()).await {
                    break;
                }
            }
        }));
    }
    drop(result_tx);

    job_tx
        .send(DirectoryScanJob {
            path: root.to_path_buf(),
            relative_path: PathBuf::new(),
            expected_identity: root_identity,
        })
        .await
        .map_err(|_| RecoverableTransferError::Journal {
            message: "manifest scan workers exited before the root scan".to_owned(),
        })?;

    let mut outstanding = 1usize;
    let mut job_tx = Some(job_tx);
    while outstanding > 0 {
        let result = result_rx
            .recv()
            .await
            .ok_or_else(|| RecoverableTransferError::Journal {
                message: "manifest scan workers exited with scans outstanding".to_owned(),
            })?;
        outstanding -= 1;
        let result = match result {
            Ok(result) => result,
            Err(error) => return Err(error),
        };
        entries.extend(result.entries);
        for subdirectory in result.subdirectories {
            outstanding += 1;
            match job_tx.as_ref() {
                Some(job_tx) => {
                    if job_tx.send(subdirectory).await.is_err() {
                        outstanding -= 1;
                    }
                }
                None => {
                    outstanding -= 1;
                }
            }
        }
        if outstanding == 0 {
            // 全部目录扫描完成:关闭任务通道,worker 收到 None 后退出。
            job_tx = None;
        }
    }
    for worker in workers {
        let _ = worker.await;
    }

    controls.wait_until_running().await?;
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    controls.wait_until_running().await?;
    Ok(SourceManifest {
        root: root.to_path_buf(),
        entries,
    })
}

pub async fn verify_source_manifest(
    expected: &SourceManifest,
) -> Result<(), RecoverableTransferError> {
    let mut controls = FileOperationControls::running(CancellationToken::new());
    verify_source_manifest_with_controls(expected, &mut controls).await
}

pub async fn verify_source_manifest_with_controls(
    expected: &SourceManifest,
    controls: &mut FileOperationControls,
) -> Result<(), RecoverableTransferError> {
    let actual = build_source_manifest_with_controls(&expected.root, controls).await?;
    if actual == *expected {
        return Ok(());
    }
    Err(RecoverableTransferError::SourceChanged {
        path: expected.root.clone(),
    })
}
