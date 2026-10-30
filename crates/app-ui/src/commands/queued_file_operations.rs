use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, Instant};

use file_core::{
    create_archive_with_controls_and_progress, create_directory, create_empty_file,
    delete_path_permanently, extract_archive_with_controls_and_progress,
    is_direct_move_segment_candidate, persist_recoverable_source_manifest_with_controls,
    prepare_direct_move_intent_segment, rename_entry, run_direct_move_batch_to_durable_renamed,
    run_recoverable_transfer, ArchiveCreationRequest, ArchiveExtractionRequest, CopyProgress,
    DirectMoveBatchRecord, DirectMoveIntentBatchRecord, FileError, FileOperationControls,
    FileOperationVerification, FileTransferOptions, RecoverableTransferError,
    RecoverableTransferOperation, RecoverableTransferOutcome, TransferConflictStrategy,
    TransferJournal, TransferJournalError, TransferJournalMutation, TransferJournalRecord,
    TransferWorkKey, TrashCommitBatch, TrashRestoreEntry, TrashVerificationBatch,
};
use file_operation_store::TaskQueueStore;
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;

use crate::localization::translate_current;
use crate::model::Message;
use crate::operation_history::{
    CompletedTransfer, FileOperationCompletion, FileOperationHistoryEligibility,
    FileOperationOutcome, RenameRejection,
};
use crate::operation_progress::FileOperationProgressUpdate;
use crate::operation_queue::{
    QueuedFileOperation, QueuedTransfer, RunningFileOperation, NEW_DIRECTORY_NAME, NEW_FILE_NAME,
};

use self::entry_gathering::{
    run_queued_create_directories, run_queued_gather_selection_into_new_folder,
    run_queued_ungather_new_folder,
};
use self::extract_members::run_queued_extract_archive_members;
use super::batch_rename_operation::{rename_failure_text, run_queued_batch_rename};
use super::convert_operation::run_queued_convert;

const FILE_OPERATION_CHANNEL_SIZE: usize = 32;
const BYTE_PROGRESS_UI_INTERVAL: Duration = crate::ui_pacing::PROGRESS_UI_INTERVAL;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableDirectMoveCommit {
    pub(crate) work_key: TransferWorkKey,
    pub(crate) source: PathBuf,
    pub(crate) target: PathBuf,
    pub(crate) checkpoint_revision: u64,
}

#[cfg(test)]
impl DurableDirectMoveCommit {
    pub(crate) fn for_test(
        work_key: TransferWorkKey,
        source: PathBuf,
        target: PathBuf,
        checkpoint_revision: u64,
    ) -> Self {
        Self {
            work_key,
            source,
            target,
            checkpoint_revision,
        }
    }
}

fn should_send_byte_progress(last_sent_at: Option<Instant>, now: Instant) -> bool {
    match last_sent_at {
        Some(last_sent_at) => now.duration_since(last_sent_at) >= BYTE_PROGRESS_UI_INTERVAL,
        None => true,
    }
}

/// 驱动者是 `Task::stream`:一次性托付给执行器跑到流结束,不进订阅 hash 名册,
/// 不会被 subscription() 重评估静默回收。驱动者死亡由队列监督检测并换代重启,
/// 见 `operation_queue::supervision`。
pub(crate) fn file_operation_driver_task(task: RunningFileOperation) -> iced::Task<Message> {
    iced::Task::stream(iced::stream::channel(
        FILE_OPERATION_CHANNEL_SIZE,
        async move |mut output| {
            let RunningFileOperation {
                id: task_id,
                stored_id,
                operation,
                controls,
                store,
                generation,
            } = task;
            let result = run_queued_file_operation(
                operation,
                controls,
                store,
                stored_id,
                task_id,
                &mut output,
            )
            .await;
            let _ = output
                .send(Message::FileOperationFinished(task_id, generation, result))
                .await;
        },
    ))
}

async fn run_queued_file_operation(
    operation: QueuedFileOperation,
    controls: FileOperationControls,
    store: Option<TaskQueueStore>,
    stored_task_id: Option<u64>,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> FileOperationCompletion {
    let result = match operation {
        QueuedFileOperation::Rename { path, new_name } => {
            return run_queued_rename(path, new_name, controls, task_id, output).await;
        }
        QueuedFileOperation::BatchRename { items } => {
            send_file_operation_progress(
                output,
                task_id,
                FileOperationProgressUpdate::Indeterminate,
            )
            .await;
            let total_items = items.len();
            match run_queued_batch_rename(items, controls).await {
                Ok(outcome) => {
                    send_file_operation_progress(
                        output,
                        task_id,
                        FileOperationProgressUpdate::IndeterminateItems {
                            completed: total_items,
                            total: total_items,
                        },
                    )
                    .await;
                    Ok(outcome)
                }
                Err(error) => Err(error),
            }
        }
        QueuedFileOperation::CreateDirectory { parent } => {
            run_queued_create_directory(parent, controls, task_id, output).await
        }
        QueuedFileOperation::CreateDirectories {
            parent,
            names,
            gather_sources,
        } => {
            run_queued_create_directories(parent, names, gather_sources, controls, task_id, output)
                .await
        }
        QueuedFileOperation::CreateEmptyFile { parent } => {
            run_queued_create_empty_file(parent, controls, task_id, output).await
        }
        QueuedFileOperation::Trash { paths } => {
            run_queued_trash(paths, controls, task_id, output).await
        }
        QueuedFileOperation::Restore { entries } => {
            run_queued_restore(entries, controls, task_id, output).await
        }
        QueuedFileOperation::DeleteTrashEntries { entries } => {
            run_queued_delete_trash_entries(entries, controls, task_id, output).await
        }
        QueuedFileOperation::DeletePermanently { paths } => {
            run_queued_delete_permanently(paths, controls, task_id, output).await
        }
        QueuedFileOperation::EmptyTrash => run_queued_empty_trash(controls, task_id, output).await,
        QueuedFileOperation::Copy {
            transfers,
            verification,
        } => {
            return {
                run_queued_transfers(
                    transfers,
                    controls,
                    stored_task_id.expect("recoverable copy has a persisted task id"),
                    task_id,
                    output,
                    store,
                    QueuedTransferMode::Copy,
                    verification,
                )
                .await
            }
        }
        QueuedFileOperation::Duplicate {
            transfers,
            verification,
        } => {
            return {
                // 副本走复制管线;完成后由 UI 按 duplicate_selection_targets 选中。
                run_queued_transfers(
                    transfers,
                    controls,
                    stored_task_id.expect("recoverable duplicate has a persisted task id"),
                    task_id,
                    output,
                    store,
                    QueuedTransferMode::Copy,
                    verification,
                )
                .await
            };
        }
        QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources } => {
            run_queued_gather_selection_into_new_folder(
                directory, sources, controls, task_id, output,
            )
            .await
        }
        QueuedFileOperation::UngatherNewFolder {
            directory,
            restore_targets,
        } => {
            run_queued_ungather_new_folder(directory, restore_targets, controls, task_id, output)
                .await
        }
        QueuedFileOperation::CreateSymbolicLinks { links } => {
            run_queued_create_symbolic_links(links, controls, task_id, output).await
        }
        QueuedFileOperation::Move {
            transfers,
            verification,
        } => {
            return {
                run_queued_transfers(
                    transfers,
                    controls,
                    stored_task_id.expect("recoverable move has a persisted task id"),
                    task_id,
                    output,
                    store,
                    QueuedTransferMode::Move,
                    verification,
                )
                .await
            }
        }
        QueuedFileOperation::CreateArchive {
            sources,
            target,
            format,
            compression_level,
            password,
        } => {
            run_queued_create_archive(
                sources,
                target,
                format,
                compression_level,
                password,
                controls,
                task_id,
                output,
            )
            .await
        }
        QueuedFileOperation::ExtractArchive { request } => {
            run_queued_extract_archive(request, controls, task_id, output).await
        }
        QueuedFileOperation::ExtractArchiveMembers {
            sources,
            destination,
            password,
        } => {
            run_queued_extract_archive_members(
                sources,
                destination,
                password,
                controls,
                task_id,
                output,
            )
            .await
        }
        QueuedFileOperation::Convert { requests } => {
            run_queued_convert(requests, controls, task_id, output).await
        }
    };

    FileOperationCompletion::from_result(result)
}

async fn run_queued_extract_archive(
    request: ArchiveExtractionRequest,
    controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    send_file_operation_progress(output, task_id, FileOperationProgressUpdate::Indeterminate).await;
    let (progress_sender, mut progress_receiver) = tokio::sync::watch::channel(None);
    let mut extraction = Box::pin(extract_archive_with_controls_and_progress(
        request,
        controls,
        move |progress| {
            progress_sender.send_replace(Some(progress));
        },
    ));
    let mut progress_open = true;
    let mut last_progress_sent = None;
    let mut last_progress_sent_at = None;

    loop {
        tokio::select! {
            changed = progress_receiver.changed(), if progress_open => {
                match changed {
                    Ok(()) => {
                        let progress = *progress_receiver.borrow_and_update();
                        let now = Instant::now();
                        if should_send_byte_progress(last_progress_sent_at, now) {
                            if let Some(progress) = progress {
                                send_archive_extraction_progress(output, task_id, progress).await;
                                last_progress_sent = Some(progress);
                                last_progress_sent_at = Some(now);
                            }
                        }
                    }
                    Err(_) => progress_open = false,
                }
            }
            outcome = &mut extraction => {
                let latest_progress = *progress_receiver.borrow_and_update();
                if let Some(progress) = latest_progress.filter(|progress| Some(*progress) != last_progress_sent) {
                    send_archive_extraction_progress(output, task_id, progress).await;
                }
                return outcome
                    .map(|_| FileOperationOutcome::NoHistory)
                    .map_err(|error| error.to_string());
            }
        }
    }
}

async fn run_queued_create_archive(
    sources: Vec<PathBuf>,
    target: PathBuf,
    format: file_core::ArchiveFormat,
    compression_level: file_core::ArchiveCompressionLevel,
    password: Option<file_core::ArchivePassword>,
    controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    send_file_operation_progress(output, task_id, FileOperationProgressUpdate::Indeterminate).await;

    let (progress_sender, mut progress_receiver) = tokio::sync::watch::channel(None);
    let mut archive = Box::pin(create_archive_with_controls_and_progress(
        ArchiveCreationRequest {
            sources,
            target,
            format,
            compression_level,
            password,
        },
        controls,
        move |progress| {
            progress_sender.send_replace(Some(progress));
        },
    ));
    let mut progress_open = true;
    let mut last_progress_sent = None;
    let mut last_progress_sent_at = None;

    loop {
        tokio::select! {
            changed = progress_receiver.changed(), if progress_open => {
                match changed {
                    Ok(()) => {
                        let progress = *progress_receiver.borrow_and_update();
                        let now = Instant::now();
                        if should_send_byte_progress(last_progress_sent_at, now) {
                            if let Some(progress) = progress {
                                send_archive_creation_progress(output, task_id, progress).await;
                                last_progress_sent = Some(progress);
                                last_progress_sent_at = Some(now);
                            }
                        }
                    }
                    Err(_) => progress_open = false,
                }
            }
            outcome = &mut archive => {
                let latest_progress = *progress_receiver.borrow_and_update();
                if let Some(progress) = latest_progress.filter(|progress| Some(*progress) != last_progress_sent) {
                    send_archive_creation_progress(output, task_id, progress).await;
                }
                return outcome
                    .map(|_| FileOperationOutcome::NoHistory)
                    .map_err(|error| error.to_string());
            }
        }
    }
}

/// 名字被占用、不合法或文件系统不能安全改名时结果是"被拒"而不是失败：
/// 界面据此让用户换个名字。
async fn run_queued_rename(
    path: PathBuf,
    new_name: OsString,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> FileOperationCompletion {
    send_file_operation_progress(output, task_id, FileOperationProgressUpdate::Indeterminate).await;
    if let Err(error) = controls.wait_until_running().await {
        return FileOperationCompletion::from_result(Err(error.to_string()));
    }
    let target = match rename_entry(path.clone(), new_name.clone()).await {
        Ok(target) => target,
        Err(error) => {
            return match RenameRejection::from_error(&error) {
                Some(reason) => FileOperationCompletion::RenameRejected {
                    path,
                    attempted_name: new_name,
                    reason,
                },
                None => FileOperationCompletion::from_result(Err(rename_failure_text(
                    &error,
                    crate::localization::current_language(),
                ))),
            };
        }
    };
    send_file_operation_progress(
        output,
        task_id,
        FileOperationProgressUpdate::IndeterminateItems {
            completed: 1,
            total: 1,
        },
    )
    .await;
    FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
        from: path,
        to: target,
    })
}

async fn run_queued_create_directory(
    parent: PathBuf,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    let path = create_new_entry(
        parent,
        &translate_current(NEW_DIRECTORY_NAME),
        NewEntryKind::Directory,
    )
    .await?;
    send_file_operation_progress(
        output,
        task_id,
        FileOperationProgressUpdate::IndeterminateItems {
            completed: 1,
            total: 1,
        },
    )
    .await;
    Ok(FileOperationOutcome::CreateDirectory { path })
}

async fn run_queued_create_empty_file(
    parent: PathBuf,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    let path = create_new_entry(
        parent,
        &translate_current(NEW_FILE_NAME),
        NewEntryKind::EmptyFile,
    )
    .await?;
    send_file_operation_progress(
        output,
        task_id,
        FileOperationProgressUpdate::IndeterminateItems {
            completed: 1,
            total: 1,
        },
    )
    .await;
    Ok(FileOperationOutcome::CreateEmptyFile { path })
}

/// 「New...」菜单新建条目的唯一名落地:基名跟随界面语言,候选名按共享
/// 命名规则生成(基名、基名 2、基名 3…),由创建操作的 AlreadyExists
/// 原子裁决逐个后移——与「收纳新文件夹」执行侧的就地重选同一套命名规则。
async fn create_new_entry(
    parent: PathBuf,
    base_name: &str,
    kind: NewEntryKind,
) -> Result<PathBuf, String> {
    let mut name_taken_error = None;
    for name in crate::model::suffixed_name_candidates(std::ffi::OsStr::new(base_name), "", false) {
        let path = parent.join(name);
        let result = match kind {
            NewEntryKind::Directory => create_directory(&path).await,
            NewEntryKind::EmptyFile => create_empty_file(&path).await,
        };
        match result {
            Ok(path) => return Ok(path),
            Err(error) if create_entry_name_taken(&error) => {
                name_taken_error = Some(error.to_string());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(name_taken_error
        .unwrap_or_else(|| format!("no available name for new entry in {}", parent.display())))
}

/// 候选名被占用的错误形态:create_dir / create_new 的 AlreadyExists 就是
/// 占用判定本身,先探测再创建反而引入 TOCTOU。
fn create_entry_name_taken(error: &FileError) -> bool {
    match error {
        FileError::CreateDirectory { source, .. } | FileError::CreateFile { source, .. } => {
            source.kind() == std::io::ErrorKind::AlreadyExists
        }
        _ => false,
    }
}

enum NewEntryKind {
    Directory,
    EmptyFile,
}

async fn run_queued_create_symbolic_links(
    links: Vec<crate::operation_queue::SymbolicLinkCreation>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = links.len();
    for (index, link) in links.iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::symlink(&link.target_path, &link.link_path)
            .await
            .map_err(|error| error.to_string())?;
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total,
            },
        )
        .await;
    }
    Ok(FileOperationOutcome::NoHistory)
}

async fn run_queued_trash(
    paths: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = paths.len();
    let mut entries = Vec::new();
    let mut tracked_paths = Vec::new();
    let mut tracking_warnings = Vec::new();
    let mut commit_batch = TrashCommitBatch::new();
    for (index, path) in paths.iter().cloned().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        match commit_batch
            .commit(&path, controls.cancellation_token())
            .await
            .map_err(|error| error.to_string())?
        {
            file_core::TrashCommitOutcome::Tracked(entry) => {
                tracked_paths.push(entry.original_path.clone());
                entries.push(*entry);
            }
            file_core::TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
                tracking_warnings.push(warning);
            }
        }
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total,
            },
        )
        .await;
    }

    Ok(FileOperationOutcome::Trash {
        paths: tracked_paths,
        entries,
        tracking_warnings,
    })
}

async fn run_queued_restore(
    entries: Vec<TrashRestoreEntry>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = entries.len();
    let mut restored_paths = Vec::with_capacity(total);
    let batch = TrashVerificationBatch::new();
    for (index, entry) in entries.iter().cloned().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        let restored_path = batch
            .restore_entry(entry, TransferConflictStrategy::KeepBoth)
            .await
            .map_err(|error| error.to_string())?;
        restored_paths.push(restored_path);
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total,
            },
        )
        .await;
    }
    Ok(FileOperationOutcome::Restore {
        entries,
        restored_paths,
    })
}

async fn run_queued_delete_trash_entries(
    entries: Vec<TrashRestoreEntry>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = entries.len();
    let batch = TrashVerificationBatch::new();
    for (index, entry) in entries.into_iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        batch
            .delete_entry(entry)
            .await
            .map_err(|error| error.to_string())?;
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total,
            },
        )
        .await;
    }
    Ok(FileOperationOutcome::NoHistory)
}

async fn run_queued_delete_permanently(
    paths: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = paths.len();
    for (index, path) in paths.into_iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        delete_path_permanently(path)
            .await
            .map_err(|error| error.to_string())?;
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total,
            },
        )
        .await;
    }
    Ok(FileOperationOutcome::NoHistory)
}

async fn run_queued_empty_trash(
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    send_file_operation_progress(output, task_id, FileOperationProgressUpdate::Indeterminate).await;
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    file_core::empty_trash_with_cancellation(controls.cancellation_token())
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::NoHistory)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueuedTransferMode {
    Copy,
    Move,
}

impl QueuedTransferMode {
    fn operation(self) -> RecoverableTransferOperation {
        match self {
            Self::Copy => RecoverableTransferOperation::Copy,
            Self::Move => RecoverableTransferOperation::Move,
        }
    }
}

#[cfg(test)]
mod create_new_entry_tests {
    use super::*;

    #[tokio::test]
    async fn create_new_entry_skips_taken_candidates() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("新建文件夹")).unwrap();

        let created = create_new_entry(
            directory.path().to_path_buf(),
            "新建文件夹",
            NewEntryKind::Directory,
        )
        .await
        .unwrap();

        assert_eq!(created, directory.path().join("新建文件夹 2"));
        assert!(created.is_dir());
    }

    #[tokio::test]
    async fn create_new_entry_creates_base_name_when_free() {
        let directory = tempfile::tempdir().unwrap();

        let created = create_new_entry(
            directory.path().to_path_buf(),
            "新建文件",
            NewEntryKind::EmptyFile,
        )
        .await
        .unwrap();

        assert_eq!(created, directory.path().join("新建文件"));
        assert!(created.is_file());
    }
}

mod entry_gathering;
mod extract_members;
mod recoverable;

#[cfg(test)]
mod archive_progress_tests;

use recoverable::{
    run_queued_transfers, send_archive_creation_progress, send_archive_extraction_progress,
    send_file_operation_progress,
};
