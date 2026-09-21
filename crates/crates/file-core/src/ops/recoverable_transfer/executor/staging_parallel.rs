//! 目录 payload 的并行 staging:两阶段(保序准备 + 并行文件搬运)。
//!
//! 协议不变量保持方式:
//! - 目录创建串行保序;符号链接串行创建,绝不跟随。
//! - 文件搬运是唯一并行点:payload 是全新空树,名称天然无冲突;每个文件
//!   独立走策略阶梯 + 类型/长度校验 + 元数据尽力恢复。
//! - 目录元数据在全部文件完成后按后序恢复(后代先于祖先,与旧 LIFO 等价)。
//! - 崩溃语义不变:partial payload 本就是"清理后整项重做"。
//! - 树级内容证明(身份→哈希→身份/memo)仍由 stage_transfer 在聚合后执行。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::ops::transfer_metadata::apply_transfer_metadata_best_effort;
use crate::ops::transfer_object::{inspect_transfer_source, TransferSourceKind};
use crate::ops::transfer_strategy::{
    copy_regular_file_payload, RegularFilePayloadCopy, TransferStrategyEngine,
};
use crate::FileOperationControls;

use super::super::{inspect_file_identity, FileObjectKind, RecoverableTransferError};

/// 并行搬运的单个文件条目。
struct StagedFileEntry {
    source: PathBuf,
    payload: PathBuf,
    length: u64,
}

/// 后序恢复元数据的目录条目。
struct StagedDirectoryEntry {
    source: PathBuf,
    payload: PathBuf,
    source_object: crate::ops::TransferSourceObject,
}

/// worker 间的共享终态:首个错误或累计完成数。
#[derive(Default)]
struct StagingProgress {
    error: Option<RecoverableTransferError>,
    files_copied: u64,
}

impl StagingProgress {
    fn record_error(&mut self, error: RecoverableTransferError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

/// 把 source_root 完整搬运到 payload_root(payload 必须已存在且为空)。
pub(super) async fn copy_tree_to_payload_parallel(
    source_root: &Path,
    payload_root: &Path,
    engine: &Arc<TransferStrategyEngine>,
    controls: &FileOperationControls,
    progress: Option<&crate::ops::ProgressSender>,
) -> Result<(), RecoverableTransferError> {
    let mut directories: Vec<StagedDirectoryEntry> = Vec::new();
    let mut files: Vec<StagedFileEntry> = Vec::new();

    // payload 根目录由本函数负责(owned artifact 只创建 artifact root);
    // 旧串行路径由 copy_directory 的 ensure 步骤承担。根目录同样进入后序
    // 元数据恢复列表(快照比对按源 mtime 校验,漏掉必然 SourceChanged)。
    tokio::fs::create_dir_all(payload_root)
        .await
        .map_err(|source| {
            RecoverableTransferError::file_system(
                "create staging payload root",
                payload_root,
                source,
            )
        })?;
    let root_source_object = inspect_transfer_source(source_root)
        .await
        .map_err(RecoverableTransferError::FileOperation)?;
    directories.push(StagedDirectoryEntry {
        source: source_root.to_path_buf(),
        payload: payload_root.to_path_buf(),
        source_object: root_source_object,
    });

    // 阶段一:保序准备。目录 mkdir + 符号链接串行,文件仅登记。
    prepare_tree(
        source_root,
        payload_root,
        &mut directories,
        &mut files,
        controls,
    )
    .await?;

    // 阶段二:文件并行搬运。
    let parallelism = engine.parallelism();
    let progress_state = Arc::new(tokio::sync::Mutex::new(StagingProgress::default()));
    let (dispatch_tx, dispatch_rx) = mpsc::channel::<StagedFileEntry>(parallelism);
    let dispatch_rx = Arc::new(tokio::sync::Mutex::new(dispatch_rx));
    let mut workers = Vec::with_capacity(parallelism);
    for _ in 0..parallelism {
        let dispatch_rx = dispatch_rx.clone();
        let engine = engine.clone();
        let controls = controls.clone();
        let mut controls = controls;
        let progress = progress.cloned();
        let progress_state = progress_state.clone();
        workers.push(tokio::spawn(async move {
            loop {
                let entry = {
                    let mut dispatch = dispatch_rx.lock().await;
                    dispatch.recv().await
                };
                let Some(entry) = entry else {
                    break;
                };
                if let Err(error) = controls.wait_until_running().await {
                    progress_state
                        .lock()
                        .await
                        .record_error(RecoverableTransferError::FileOperation(error));
                    break;
                }
                if let Err(error) =
                    copy_staged_file(&entry, &engine, &controls, progress.as_ref()).await
                {
                    progress_state.lock().await.record_error(error);
                    break;
                }
                progress_state.lock().await.files_copied += 1;
            }
        }));
    }

    // 调度:逐个派发;任一 worker 失败即停止派发,channel 关闭让其余 worker
    // 收尾退出,然后在 join 处等待全部终止。
    let dispatch_error = {
        let mut dispatch_error = None;
        let dispatch = dispatch_tx;
        for entry in files {
            if progress_state.lock().await.error.is_some() {
                break;
            }
            if dispatch.send(entry).await.is_err() {
                break;
            }
        }
        drop(dispatch);
        for worker in workers {
            if let Err(join_error) = worker.await {
                dispatch_error = Some(RecoverableTransferError::file_system(
                    "join staging worker",
                    payload_root,
                    std::io::Error::other(join_error),
                ));
            }
        }
        dispatch_error
    };

    let progress_state_error = progress_state.lock().await.error.take();
    if let Some(error) = dispatch_error.or(progress_state_error) {
        return Err(error);
    }

    // 阶段三:目录元数据后序恢复。
    let mut controls = controls.clone();
    for directory in directories.iter().rev() {
        apply_transfer_metadata_best_effort(
            &directory.source,
            &directory.payload,
            &directory.source_object,
        )
        .await;
        controls
            .wait_until_running()
            .await
            .map_err(RecoverableTransferError::FileOperation)?;
    }

    Ok(())
}

async fn prepare_tree(
    source_root: &Path,
    payload_root: &Path,
    directories: &mut Vec<StagedDirectoryEntry>,
    files: &mut Vec<StagedFileEntry>,
    controls: &FileOperationControls,
) -> Result<(), RecoverableTransferError> {
    let mut controls = controls.clone();
    let mut pending: Vec<(PathBuf, PathBuf)> =
        vec![(source_root.to_path_buf(), payload_root.to_path_buf())];

    while let Some((source_dir, payload_dir)) = pending.pop() {
        controls
            .wait_until_running()
            .await
            .map_err(RecoverableTransferError::FileOperation)?;
        let mut reader = tokio::fs::read_dir(&source_dir).await.map_err(|source| {
            RecoverableTransferError::file_system(
                "read staging source directory",
                &source_dir,
                source,
            )
        })?;
        while let Some(entry) = reader.next_entry().await.map_err(|source| {
            RecoverableTransferError::file_system(
                "read staging source entry in",
                &source_dir,
                source,
            )
        })? {
            controls
                .wait_until_running()
                .await
                .map_err(RecoverableTransferError::FileOperation)?;
            let child_source = entry.path();
            let child_payload = payload_dir.join(entry.file_name());
            let source_object = inspect_transfer_source(&child_source)
                .await
                .map_err(RecoverableTransferError::FileOperation)?;
            match &source_object.kind {
                TransferSourceKind::Directory => {
                    tokio::fs::create_dir(&child_payload)
                        .await
                        .map_err(|source| {
                            RecoverableTransferError::file_system(
                                "create staging payload directory",
                                &child_payload,
                                source,
                            )
                        })?;
                    directories.push(StagedDirectoryEntry {
                        source: child_source.clone(),
                        payload: child_payload.clone(),
                        source_object,
                    });
                    pending.push((child_source, child_payload));
                }
                TransferSourceKind::RegularFile => {
                    files.push(StagedFileEntry {
                        source: child_source,
                        payload: child_payload,
                        length: source_object.metadata.len(),
                    });
                }
                TransferSourceKind::SymbolicLink { target } => {
                    #[cfg(unix)]
                    tokio::fs::symlink(target, &child_payload)
                        .await
                        .map_err(|source| {
                            RecoverableTransferError::file_system(
                                "create staging payload symbolic link",
                                &child_payload,
                                source,
                            )
                        })?;
                    #[cfg(not(unix))]
                    {
                        let _ = target;
                        let _ = child_payload;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn copy_staged_file(
    entry: &StagedFileEntry,
    engine: &Arc<TransferStrategyEngine>,
    controls: &FileOperationControls,
    progress: Option<&crate::ops::ProgressSender>,
) -> Result<(), RecoverableTransferError> {
    let source_object = inspect_transfer_source(&entry.source)
        .await
        .map_err(RecoverableTransferError::FileOperation)?;
    let mut controls = controls.clone();
    {
        use std::os::unix::fs::MetadataExt;
        copy_regular_file_payload(RegularFilePayloadCopy {
            source: &entry.source,
            target: &entry.payload,
            source_device: source_object.metadata.dev(),
            bytes_total: entry.length,
            engine,
            controls: &mut controls,
            progress,
            source_hasher: None,
        })
        .await
        .map_err(RecoverableTransferError::FileOperation)?;
    }

    // 硬成功校验:类型 + 长度(树级内容证明由 stage_transfer 聚合执行)。
    let payload_identity = inspect_file_identity(&entry.payload).await?;
    if payload_identity.object_kind != FileObjectKind::RegularFile
        || payload_identity.size != entry.length
    {
        return Err(RecoverableTransferError::FingerprintMismatch {
            path: entry.payload.clone(),
        });
    }
    apply_transfer_metadata_best_effort(&entry.source, &entry.payload, &source_object).await;
    Ok(())
}
