use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::transfer_conflict::{first_available_entry_path, transfer_target_metadata_if_exists};
use crate::{EntryNameKind, FileError};

use super::copy_verification::{
    verify_copied_directory, verify_copied_file, verify_copied_symbolic_link,
};
use super::metadata_loss::{MetadataKind, MetadataLossSummary};
use super::transfer_metadata::apply_transfer_metadata;
use super::transfer_object::{inspect_transfer_source, TransferSourceKind, TransferSourceObject};
use super::{already_exists_error, ensure_replace_target_does_not_contain_source_path};

const COPY_BUFFER_SIZE: usize = 1024 * 1024;

/// 一次复制的结果:`target` 为 `None` 表示冲突策略跳过了本次复制;
/// `metadata_loss` 是内容已复制成功、但没能保留下来的元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedTarget {
    pub target: Option<PathBuf>,
    pub metadata_loss: MetadataLossSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyProgress {
    pub from: PathBuf,
    pub to: PathBuf,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub type ProgressSender = mpsc::UnboundedSender<CopyProgress>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferConflictStrategy {
    Fail,
    Replace,
    Skip,
    KeepBoth,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperationVerification {
    #[default]
    BasicMetadata,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperationRunState {
    Running,
    Paused,
    ApplicationStopping,
}

#[derive(Clone)]
pub struct FileOperationControls {
    cancel: CancellationToken,
    run_state: watch::Receiver<FileOperationRunState>,
}

impl FileOperationControls {
    pub fn new(
        cancel: CancellationToken,
        run_state: watch::Receiver<FileOperationRunState>,
    ) -> Self {
        Self { cancel, run_state }
    }

    pub fn running(cancel: CancellationToken) -> Self {
        let (_run_state_sender, run_state) = watch::channel(FileOperationRunState::Running);
        Self { cancel, run_state }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn checkpoint_now(&self) -> Result<(), FileError> {
        match *self.run_state.borrow() {
            FileOperationRunState::ApplicationStopping => Err(FileError::ApplicationStopping),
            FileOperationRunState::Running if self.cancel.is_cancelled() => {
                Err(FileError::Cancelled)
            }
            FileOperationRunState::Running | FileOperationRunState::Paused => Ok(()),
        }
    }

    pub async fn wait_until_running(&mut self) -> Result<(), FileError> {
        loop {
            match *self.run_state.borrow() {
                FileOperationRunState::ApplicationStopping => {
                    return Err(FileError::ApplicationStopping)
                }
                FileOperationRunState::Running if self.cancel.is_cancelled() => {
                    return Err(FileError::Cancelled)
                }
                FileOperationRunState::Running => return Ok(()),
                FileOperationRunState::Paused => {}
            }

            tokio::select! {
                changed = self.run_state.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
                _ = self.cancel.cancelled() => {
                    if *self.run_state.borrow() == FileOperationRunState::ApplicationStopping {
                        return Err(FileError::ApplicationStopping);
                    }
                    return Err(FileError::Cancelled);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct FileTransferOptions {
    pub(super) controls: FileOperationControls,
    pub(super) progress: Option<ProgressSender>,
    pub(super) conflict_strategy: TransferConflictStrategy,
    pub(super) verification: FileOperationVerification,
    /// Basic 校验的运行内证明记忆;Strong 校验忽略该字段。
    pub(super) proof_memo: Option<crate::ops::recoverable_transfer::SharedProofMemo>,
    /// 按设备对缓存的搬运策略引擎(克隆/内核快拷/用户态阶梯)。
    pub(super) strategy_engine:
        Option<std::sync::Arc<crate::ops::transfer_strategy::TransferStrategyEngine>>,
}

impl FileTransferOptions {
    pub fn new(controls: FileOperationControls) -> Self {
        Self {
            controls,
            progress: None,
            conflict_strategy: TransferConflictStrategy::Fail,
            verification: FileOperationVerification::default(),
            proof_memo: None,
            strategy_engine: None,
        }
    }

    pub fn running(cancel: CancellationToken) -> Self {
        Self::new(FileOperationControls::running(cancel))
    }

    pub fn with_progress_sender(mut self, progress: ProgressSender) -> Self {
        self.progress = Some(progress);
        self
    }

    pub fn with_optional_progress(mut self, progress: Option<ProgressSender>) -> Self {
        self.progress = progress;
        self
    }

    pub fn with_conflict_strategy(mut self, conflict_strategy: TransferConflictStrategy) -> Self {
        self.conflict_strategy = conflict_strategy;
        self
    }

    pub fn with_verification(mut self, verification: FileOperationVerification) -> Self {
        self.verification = verification;
        self
    }

    /// 附加运行内证明记忆。同一批传输(同一任务)应共享同一份 memo,
    /// 使"每对象每次运行至多全文哈希一次"跨批次记录生效。
    pub fn with_proof_memo(
        mut self,
        proof_memo: crate::ops::recoverable_transfer::SharedProofMemo,
    ) -> Self {
        self.proof_memo = Some(proof_memo);
        self
    }

    /// 附加搬运策略引擎;同一任务的全部记录共享设备能力缓存。
    pub fn with_strategy_engine(
        mut self,
        strategy_engine: std::sync::Arc<crate::ops::transfer_strategy::TransferStrategyEngine>,
    ) -> Self {
        self.strategy_engine = Some(strategy_engine);
        self
    }
}

pub async fn copy_path(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    cancel: CancellationToken,
    progress: Option<ProgressSender>,
) -> Result<MetadataLossSummary, FileError> {
    copy_path_with_options(
        from,
        to,
        FileTransferOptions::running(cancel).with_optional_progress(progress),
    )
    .await
    .map(|copied| copied.metadata_loss)
}

pub async fn copy_path_with_options(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    transfer_options: FileTransferOptions,
) -> Result<CopiedTarget, FileError> {
    copy_path_with_transfer_options(from, to, transfer_options).await
}

async fn copy_path_with_transfer_options(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    transfer_options: FileTransferOptions,
) -> Result<CopiedTarget, FileError> {
    let from = from.as_ref().to_path_buf();
    let to = to.as_ref().to_path_buf();
    let mut controls = transfer_options.controls.clone();
    controls.wait_until_running().await?;
    let source_object = inspect_transfer_source(&from).await?;

    copy_path_with_inspected_source(&from, &to, &source_object, transfer_options).await
}

pub(super) async fn copy_path_with_inspected_source(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
    transfer_options: FileTransferOptions,
) -> Result<CopiedTarget, FileError> {
    let mut controls = transfer_options.controls;
    let progress = transfer_options.progress;
    let conflict_strategy = transfer_options.conflict_strategy;
    let verification = transfer_options.verification;
    controls.wait_until_running().await?;

    let Some(to) =
        prepare_copy_target(from, to, &source_object.metadata, conflict_strategy).await?
    else {
        return Ok(CopiedTarget {
            target: None,
            metadata_loss: MetadataLossSummary::default(),
        });
    };

    let metadata_loss = match &source_object.kind {
        TransferSourceKind::Directory => {
            copy_directory(
                from,
                &to,
                source_object,
                &mut controls,
                progress.as_ref(),
                conflict_strategy,
                verification,
                transfer_options.strategy_engine.as_deref(),
            )
            .await?
        }
        TransferSourceKind::RegularFile => {
            let mut buffer = vec![0; COPY_BUFFER_SIZE];
            let lost = copy_file_to_target(
                FileCopyTarget {
                    from,
                    to: &to,
                    source_object,
                    verification,
                    strategy_engine: transfer_options.strategy_engine.as_deref(),
                },
                &mut controls,
                progress.as_ref(),
                &mut buffer,
            )
            .await?;
            single_entry_metadata_loss(&to, lost)
        }
        TransferSourceKind::SymbolicLink { .. } => {
            let lost =
                copy_symbolic_link_to_target(from, &to, source_object, &mut controls).await?;
            single_entry_metadata_loss(&to, lost)
        }
    };

    Ok(CopiedTarget {
        target: Some(to),
        metadata_loss,
    })
}

fn single_entry_metadata_loss(target: &Path, lost: BTreeSet<MetadataKind>) -> MetadataLossSummary {
    let mut summary = MetadataLossSummary::default();
    summary.record(target, lost);
    summary
}

/// 单个普通文件落到目标的请求集合;引擎未注入时按设备现场探测。
struct FileCopyTarget<'a> {
    from: &'a Path,
    to: &'a Path,
    source_object: &'a TransferSourceObject,
    verification: FileOperationVerification,
    strategy_engine: Option<&'a crate::ops::transfer_strategy::TransferStrategyEngine>,
}

async fn copy_file_to_target(
    target: FileCopyTarget<'_>,
    controls: &mut FileOperationControls,
    progress: Option<&ProgressSender>,
    buffer: &mut [u8],
) -> Result<BTreeSet<MetadataKind>, FileError> {
    let FileCopyTarget {
        from,
        to,
        source_object,
        verification,
        strategy_engine,
    } = target;
    let metadata = &source_object.metadata;

    // 引擎未注入(内部低层调用)时按调用点设备现场探测,能力缓存仅覆盖
    // 本文件;用户任务的引擎由 run 层注入并跨文件复用。
    let local_engine;
    let engine = match strategy_engine {
        Some(engine) => engine,
        None => {
            local_engine =
                crate::ops::transfer_strategy::TransferStrategyEngine::probe(metadata.dev(), to);
            &local_engine
        }
    };

    // Strong 校验在 UserLoop 内联喂数据;克隆/内核快拷不过用户态,
    // 由下方补读源内容计算,总读次数与历史实现持平。
    let mut source_content_hasher =
        (verification == FileOperationVerification::Strong).then(blake3::Hasher::new);
    let payload_outcome = {
        crate::ops::transfer_strategy::copy_regular_file_payload(
            crate::ops::transfer_strategy::RegularFilePayloadCopy {
                source: from,
                target: to,
                source_device: metadata.dev(),
                bytes_total: metadata.len(),
                engine,
                controls,
                progress,
                source_hasher: match source_content_hasher.as_mut() {
                    Some(hasher) => Some(hasher),
                    None => None,
                },
            },
        )
        .await
    };
    if let Err(error) = payload_outcome {
        let _ = fs::remove_file(to).await;
        return Err(error);
    }

    if verification == FileOperationVerification::Strong {
        let writer = fs::OpenOptions::new().write(true).open(to).await;
        match writer {
            Ok(writer) => {
                if let Err(source) = writer.sync_all().await {
                    let _ = fs::remove_file(to).await;
                    return Err(FileError::Copy {
                        from: from.to_path_buf(),
                        to: to.to_path_buf(),
                        source,
                    });
                }
            }
            Err(source) => {
                let _ = fs::remove_file(to).await;
                return Err(FileError::Copy {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    source,
                });
            }
        }
    }

    let source_content_hash = match verification {
        FileOperationVerification::Strong => {
            if strategy_inlined_hash(payload_outcome.as_ref().unwrap()) {
                source_content_hasher.map(|hasher| hasher.finalize())
            } else {
                let hash = hash_source_content(from, controls, buffer).await;
                if hash.is_err() {
                    let _ = fs::remove_file(to).await;
                }
                Some(hash?)
            }
        }
        FileOperationVerification::BasicMetadata => None,
    };
    if let Err(error) =
        verify_copied_file(from, to, metadata, controls, buffer, source_content_hash).await
    {
        let _ = fs::remove_file(to).await;
        return Err(error);
    }

    let lost = match apply_transfer_metadata(from, to, source_object).await {
        Ok(lost) => lost,
        Err(error) => {
            let _ = fs::remove_file(to).await;
            return Err(error);
        }
    };
    if let Err(error) = controls.wait_until_running().await {
        let _ = fs::remove_file(to).await;
        return Err(error);
    }

    Ok(lost)
}

fn strategy_inlined_hash(outcome: &crate::ops::transfer_strategy::PayloadCopyOutcome) -> bool {
    outcome.strategy == crate::ops::transfer_strategy::PayloadCopyStrategy::UserLoop
}

/// 补读源内容计算 BLAKE3:克隆/内核快拷路径的字节不过用户态,Strong 校验
/// 用它换取与历史实现相同的读次数(内联源哈希 + 目标重读)。
async fn hash_source_content(
    from: &Path,
    controls: &mut FileOperationControls,
    buffer: &mut [u8],
) -> Result<blake3::Hash, FileError> {
    let mut reader = fs::File::open(from)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: from.to_path_buf(),
            source,
        })?;
    let mut hasher = blake3::Hasher::new();
    loop {
        controls.wait_until_running().await?;
        let read = reader
            .read(buffer)
            .await
            .map_err(|source| FileError::Copy {
                from: from.to_path_buf(),
                to: from.to_path_buf(),
                source,
            })?;
        if read == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buffer[..read]);
    }
}

/// 符号链接复制的唯一规则：建链接、校验链接目标、尽力恢复元数据，返回没能保留的种类。
/// 串行复制和可恢复传输的并行暂存都走这里，保真结果不因路径不同而不同。
#[cfg(unix)]
pub(crate) async fn copy_symbolic_link_to_target(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
    controls: &mut FileOperationControls,
) -> Result<BTreeSet<MetadataKind>, FileError> {
    let TransferSourceKind::SymbolicLink {
        target: link_target,
    } = &source_object.kind
    else {
        unreachable!();
    };
    fs::symlink(link_target, to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;

    let completion = async {
        verify_copied_symbolic_link(from, to, link_target).await?;
        controls.wait_until_running().await?;
        let lost = apply_transfer_metadata(from, to, source_object).await?;
        controls.wait_until_running().await?;
        Ok(lost)
    }
    .await;

    if completion.is_err() {
        let _ = fs::remove_file(to).await;
    }
    completion
}

#[cfg(not(unix))]
pub(crate) async fn copy_symbolic_link_to_target(
    from: &Path,
    _to: &Path,
    _source_object: &TransferSourceObject,
    _controls: &mut FileOperationControls,
) -> Result<BTreeSet<MetadataKind>, FileError> {
    Err(FileError::InvalidInput {
        path: from.to_path_buf(),
        message: "cannot transfer symbolic links on this platform".to_owned(),
    })
}

enum DirectoryCopyStep {
    CopyChildren {
        source: PathBuf,
        target: PathBuf,
    },
    PreserveMetadata {
        source: PathBuf,
        target: PathBuf,
        source_object: TransferSourceObject,
    },
}

#[allow(clippy::too_many_arguments)]
async fn copy_directory(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
    controls: &mut FileOperationControls,
    progress: Option<&ProgressSender>,
    conflict_strategy: TransferConflictStrategy,
    verification: FileOperationVerification,
    strategy_engine: Option<&crate::ops::transfer_strategy::TransferStrategyEngine>,
) -> Result<MetadataLossSummary, FileError> {
    if to.starts_with(from) {
        return Err(FileError::InvalidInput {
            path: to.to_path_buf(),
            message: "cannot copy a directory into itself".to_owned(),
        });
    }

    let created_root = ensure_copy_directory_target(from, to).await?;
    let mut pending_steps = Vec::new();
    if created_root {
        pending_steps.push(DirectoryCopyStep::PreserveMetadata {
            source: from.to_path_buf(),
            target: to.to_path_buf(),
            source_object: source_object.clone(),
        });
    }
    pending_steps.push(DirectoryCopyStep::CopyChildren {
        source: from.to_path_buf(),
        target: to.to_path_buf(),
    });

    let copy_outcome = async {
        verify_copied_directory(from, to).await?;
        copy_directory_contents(
            pending_steps,
            controls,
            progress,
            conflict_strategy,
            verification,
            strategy_engine,
        )
        .await
    }
    .await;

    if copy_outcome.is_err() && created_root {
        let _ = fs::remove_dir_all(to).await;
    }
    copy_outcome
}

async fn copy_directory_contents(
    mut pending_steps: Vec<DirectoryCopyStep>,
    controls: &mut FileOperationControls,
    progress: Option<&ProgressSender>,
    conflict_strategy: TransferConflictStrategy,
    verification: FileOperationVerification,
    strategy_engine: Option<&crate::ops::transfer_strategy::TransferStrategyEngine>,
) -> Result<MetadataLossSummary, FileError> {
    let mut buffer = vec![0; COPY_BUFFER_SIZE];
    let mut metadata_loss = MetadataLossSummary::default();

    while let Some(step) = pending_steps.pop() {
        controls.wait_until_running().await?;
        let (source_directory, target_directory) = match step {
            DirectoryCopyStep::CopyChildren { source, target } => (source, target),
            DirectoryCopyStep::PreserveMetadata {
                source,
                target,
                source_object,
            } => {
                let lost = apply_transfer_metadata(&source, &target, &source_object).await?;
                metadata_loss.record(&target, lost);
                controls.wait_until_running().await?;
                continue;
            }
        };
        let mut entries =
            fs::read_dir(&source_directory)
                .await
                .map_err(|source| FileError::Copy {
                    from: source_directory.clone(),
                    to: target_directory.clone(),
                    source,
                })?;

        loop {
            controls.wait_until_running().await?;
            let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|source| FileError::Copy {
                    from: source_directory.clone(),
                    to: target_directory.clone(),
                    source,
                })?
            else {
                break;
            };

            let source_child = entry.path();
            let target_child = target_directory.join(entry.file_name());
            let source_object = inspect_transfer_source(&source_child).await?;
            let child_conflict_strategy = match &source_object.kind {
                TransferSourceKind::Directory => conflict_strategy,
                TransferSourceKind::RegularFile | TransferSourceKind::SymbolicLink { .. } => {
                    nested_copy_conflict_strategy(conflict_strategy)
                }
            };
            let Some(target_child) = prepare_copy_target(
                &source_child,
                &target_child,
                &source_object.metadata,
                child_conflict_strategy,
            )
            .await?
            else {
                continue;
            };

            match &source_object.kind {
                TransferSourceKind::Directory => {
                    let created_directory =
                        ensure_copy_directory_target(&source_child, &target_child).await?;
                    verify_copied_directory(&source_child, &target_child).await?;
                    if created_directory {
                        pending_steps.push(DirectoryCopyStep::PreserveMetadata {
                            source: source_child.clone(),
                            target: target_child.clone(),
                            source_object: source_object.clone(),
                        });
                    }
                    pending_steps.push(DirectoryCopyStep::CopyChildren {
                        source: source_child,
                        target: target_child,
                    });
                }
                TransferSourceKind::RegularFile => {
                    let lost = copy_file_to_target(
                        FileCopyTarget {
                            from: &source_child,
                            to: &target_child,
                            source_object: &source_object,
                            verification,
                            strategy_engine,
                        },
                        controls,
                        progress,
                        &mut buffer,
                    )
                    .await?;
                    metadata_loss.record(&target_child, lost);
                }
                TransferSourceKind::SymbolicLink { .. } => {
                    let lost = copy_symbolic_link_to_target(
                        &source_child,
                        &target_child,
                        &source_object,
                        controls,
                    )
                    .await?;
                    metadata_loss.record(&target_child, lost);
                }
            }
        }
    }

    Ok(metadata_loss)
}

async fn prepare_copy_target(
    from: &Path,
    to: &Path,
    source_metadata: &std::fs::Metadata,
    conflict_strategy: TransferConflictStrategy,
) -> Result<Option<PathBuf>, FileError> {
    if from == to && conflict_strategy != TransferConflictStrategy::KeepBoth {
        return Ok(None);
    }

    let Some(target_metadata) = transfer_target_metadata_if_exists(to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?
    else {
        return Ok(Some(to.to_path_buf()));
    };

    match conflict_strategy {
        TransferConflictStrategy::Fail => Err(FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source: already_exists_error(),
        }),
        TransferConflictStrategy::Replace => {
            ensure_replace_target_does_not_contain_source_path(from, to, &target_metadata)?;
            remove_copy_target(from, to, &target_metadata).await?;
            Ok(Some(to.to_path_buf()))
        }
        TransferConflictStrategy::Skip => Ok(None),
        TransferConflictStrategy::KeepBoth => {
            first_available_entry_path(to, EntryNameKind::of_metadata(source_metadata))
                .await
                .map(Some)
        }
        TransferConflictStrategy::Merge => {
            if source_metadata.is_dir() && target_metadata.is_dir() {
                Ok(Some(to.to_path_buf()))
            } else {
                Ok(None)
            }
        }
    }
}

async fn ensure_copy_directory_target(from: &Path, to: &Path) -> Result<bool, FileError> {
    if transfer_target_metadata_if_exists(to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?
        .is_some()
    {
        return Ok(false);
    }

    fs::create_dir(to).await.map_err(|source| FileError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })?;

    Ok(true)
}

fn nested_copy_conflict_strategy(
    conflict_strategy: TransferConflictStrategy,
) -> TransferConflictStrategy {
    if conflict_strategy == TransferConflictStrategy::Merge {
        TransferConflictStrategy::Skip
    } else {
        conflict_strategy
    }
}

async fn remove_copy_target(
    from: &Path,
    to: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), FileError> {
    let result = if metadata.is_dir() {
        fs::remove_dir_all(to).await
    } else {
        fs::remove_file(to).await
    };

    result.map_err(|source| FileError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })
}
