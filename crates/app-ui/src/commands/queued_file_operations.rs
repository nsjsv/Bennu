use std::any::TypeId;
use std::ffi::OsString;
use std::future::Future;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use file_core::{
    create_archive_with_controls_and_progress, create_directory, create_empty_file,
    delete_path_permanently, delete_trash_entry, extract_archive_with_controls_and_progress,
    is_direct_move_segment_candidate, is_transfer_target_available,
    persist_recoverable_source_manifest_with_controls, prepare_direct_move_intent_segment,
    rename_path, restore_trash_entry, run_direct_move_batch_to_durable_renamed,
    run_recoverable_transfer, trash_path_with_restore_entry_and_cancellation,
    ArchiveCreationRequest, ArchiveExtractionRequest, CopyProgress, DirectMoveBatchRecord,
    DirectMoveIntentBatchRecord, FileError, FileOperationControls, FileOperationVerification,
    FileTransferOptions, RecoverableTransferError, RecoverableTransferOperation,
    RecoverableTransferOutcome, TransferConflictStrategy, TransferJournal, TransferJournalError,
    TransferJournalMutation, TransferJournalRecord, TransferWorkKey, TrashRestoreEntry,
};
use file_operation_store::TaskQueueStore;
use iced::advanced::subscription::{self, EventStream, Hasher, Recipe};
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::stream::BoxStream;
use iced::futures::SinkExt;
use iced::Subscription;

use crate::model::Message;
use crate::operation_history::{
    CompletedTransfer, FileOperationCompletion, FileOperationHistoryEligibility,
    FileOperationOutcome,
};
use crate::operation_progress::FileOperationProgressUpdate;
use crate::operation_queue::{
    QueuedFileOperation, QueuedTransfer, RunningFileOperation, NEW_DIRECTORY_NAME, NEW_FILE_NAME,
};

use super::batch_rename_operation::run_queued_batch_rename;
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

pub(crate) fn file_operation_subscription(task: RunningFileOperation) -> Subscription<Message> {
    subscription::from_recipe(FileOperationRecipe { task })
}

struct FileOperationRecipe {
    task: RunningFileOperation,
}

impl Recipe for FileOperationRecipe {
    type Output = Message;

    fn hash(&self, state: &mut Hasher) {
        TypeId::of::<Self>().hash(state);
        self.task.id.hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
        let RunningFileOperation {
            id: task_id,
            stored_id,
            operation,
            controls,
            store,
        } = self.task;

        Box::pin(iced::stream::channel(
            FILE_OPERATION_CHANNEL_SIZE,
            async move |mut output| {
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
                    .send(Message::FileOperationFinished(task_id, result))
                    .await;
                iced::futures::future::pending().await
            },
        ))
    }
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
            run_queued_rename(path, new_name, controls, task_id, output).await
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
            }
        }
        QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources } => {
            run_queued_gather_selection_into_new_folder(
                directory,
                sources,
                controls,
                task_id,
                output,
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

async fn run_queued_rename(
    path: PathBuf,
    new_name: String,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    send_file_operation_progress(output, task_id, FileOperationProgressUpdate::Indeterminate).await;
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    let target = rename_path(path.clone(), OsString::from(new_name))
        .await
        .map_err(|error| error.to_string())?;
    send_file_operation_progress(
        output,
        task_id,
        FileOperationProgressUpdate::IndeterminateItems {
            completed: 1,
            total: 1,
        },
    )
    .await;
    Ok(FileOperationOutcome::Rename {
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
    let path = parent.join(NEW_DIRECTORY_NAME);
    create_directory(&path)
        .await
        .map_err(|error| error.to_string())?;
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
    let path = parent.join(NEW_FILE_NAME);
    create_empty_file(&path)
        .await
        .map_err(|error| error.to_string())?;
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

async fn run_queued_gather_selection_into_new_folder(
    directory: PathBuf,
    sources: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    controls
        .wait_until_running()
        .await
        .map_err(|error| error.to_string())?;
    // 入队侧已起好名;极端竞争下名字被占时按同一命名规则就地重选,
    // 让操作完成而不是带着误导性的失败退出。
    let directory = available_gathered_folder_directory(directory).await?;
    create_directory(&directory)
        .await
        .map_err(|error| error.to_string())?;

    let mut moved = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        let name = source
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .ok_or_else(|| format!("missing file name for {}", source.display()))?;
        let target = directory.join(name);
        // 源与目标同在一个父目录树内,rename 原子完成且必同盘。
        tokio::fs::rename(source, &target)
            .await
            .map_err(|error| error.to_string())?;
        moved.push(CompletedTransfer {
            source: source.clone(),
            target,
        });
        send_file_operation_progress(
            output,
            task_id,
            FileOperationProgressUpdate::IndeterminateItems {
                completed: index + 1,
                total: sources.len(),
            },
        )
        .await;
    }
    Ok(FileOperationOutcome::GatheredIntoNewFolder {
        directory,
        moved,
    })
}

async fn run_queued_ungather_new_folder(
    directory: PathBuf,
    restore_targets: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let total = restore_targets.len();
    for (index, restore_target) in restore_targets.iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        let name = restore_target
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .ok_or_else(|| format!("missing file name for {}", restore_target.display()))?;
        tokio::fs::rename(directory.join(name), restore_target)
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
    // 只接受空目录;若撤销前用户往里放了新内容,操作显式失败而不是吞掉它们。
    tokio::fs::remove_dir(&directory)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::NoHistory)
}

async fn available_gathered_folder_directory(directory: PathBuf) -> Result<PathBuf, String> {
    if is_transfer_target_available(&directory)
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(directory);
    }
    let parent = directory
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("missing parent for {}", directory.display()))?;
    for name in crate::model::suffixed_name_candidates(
        std::ffi::OsStr::new(crate::model::GATHERED_FOLDER_BASE_NAME),
        "",
        false,
    ) {
        let candidate = parent.join(name);
        if is_transfer_target_available(&candidate)
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(candidate);
        }
    }
    Ok(directory)
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
    for (index, path) in paths.iter().cloned().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        match trash_path_with_restore_entry_and_cancellation(&path, controls.cancellation_token())
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
    for (index, entry) in entries.iter().cloned().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        let restored_path = restore_trash_entry(entry, TransferConflictStrategy::KeepBoth)
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
    for (index, entry) in entries.into_iter().enumerate() {
        controls
            .wait_until_running()
            .await
            .map_err(|error| error.to_string())?;
        delete_trash_entry(entry)
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

mod recoverable;

#[cfg(test)]
mod archive_progress_tests;

use recoverable::{
    run_queued_transfers, send_archive_creation_progress, send_archive_extraction_progress,
    send_file_operation_progress,
};
