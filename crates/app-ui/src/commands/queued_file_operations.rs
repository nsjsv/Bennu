use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, Instant};

use bennu_state::TaskQueueStore;
use file_core::{
    create_archive_with_controls_and_progress, create_directory_at_available_name,
    create_empty_file_at_available_name, create_symbolic_links, delete_path_permanently,
    extract_archive_with_controls_and_progress, is_direct_move_segment_candidate,
    persist_recoverable_source_manifest_with_controls, prepare_direct_move_intent_segment,
    rename_entry, run_direct_move_batch_to_durable_renamed, run_recoverable_transfer,
    ArchiveCreationRequest, ArchiveExtractionRequest, CopyProgress, DirectMoveBatchRecord,
    DirectMoveIntentBatchRecord, FileError, FileOperationControls, FileOperationVerification,
    FileTransferOptions, RecoverableTransferError, RecoverableTransferOperation,
    RecoverableTransferOutcome, SymbolicLinkRequest, TransferConflictStrategy, TransferJournal,
    TransferJournalError, TransferJournalMutation, TransferJournalRecord, TransferWorkKey,
    TrashRestoreEntry, TrashVerificationBatch,
};
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;

use crate::model::Message;
use crate::operation_queue::{
    new_directory_name, new_file_name, QueuedFileOperation, QueuedTransfer, RunningFileOperation,
};
use file_operations::FileOperationProgressUpdate;
use file_operations::{
    CompletedTransfer, FileOperationCompletion, FileOperationHistoryEligibility,
    FileOperationOutcome, RenameRejection,
};

use self::entry_gathering::{
    run_queued_create_directories, run_queued_gather_selection_into_new_folder,
    run_queued_ungather_new_folder,
};
use self::extract_members::run_queued_extract_archive_members;
use self::step_pacer::QueuedStepPacer;
use super::batch_rename_operation::{rename_failure_text, run_queued_batch_rename};
use super::convert_operation::run_queued_convert;

const FILE_OPERATION_CHANNEL_SIZE: usize = 32;
const BYTE_PROGRESS_UI_INTERVAL: Duration = file_core::PROGRESS_REPORT_INTERVAL;

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
            return run_queued_trash(paths, controls, task_id, output).await;
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
            created_directories,
        } => {
            run_queued_ungather_new_folder(
                directory,
                restore_targets,
                created_directories,
                controls,
                task_id,
                output,
            )
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
                    bennu_localization::current_language(),
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
    // 被占用时 file-core 原子创建并续编号（基名 2、基名 3…）。
    let path = create_directory_at_available_name(parent.join(new_directory_name()))
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
    let path = create_empty_file_at_available_name(parent.join(new_file_name()))
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

async fn run_queued_create_symbolic_links(
    links: Vec<crate::operation_queue::SymbolicLinkCreation>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> Result<FileOperationOutcome, String> {
    let requests = links
        .into_iter()
        .map(|link| SymbolicLinkRequest {
            preferred_link: link.link_path,
            target: link.target_path,
            name_kind: link.name_kind,
        })
        .collect();
    let mut pacer = QueuedStepPacer::new(&mut controls, output, task_id);
    // 建成的链接不进撤销历史、也不选中，实际路径（可能已续编号）由目录刷新呈现。
    create_symbolic_links(requests, &mut pacer)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileOperationOutcome::NoHistory)
}

async fn run_queued_trash(
    paths: Vec<PathBuf>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender<Message>,
) -> FileOperationCompletion {
    let total = paths.len();
    let mut entries = Vec::new();
    let mut untrashable = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        let trashed = match controls.wait_until_running().await {
            Ok(()) => file_core::trash_path(&path, controls.cancellation_token()).await,
            Err(error) => Err(error),
        };
        match trashed {
            Ok(entry) => entries.push(entry),
            Err(FileError::TrashUnavailable { reason, .. }) => {
                tracing::warn!(
                    target: "app_ui::file_operations",
                    event = "trash_unavailable",
                    path = %path.display(),
                    %reason,
                    "item cannot be moved to Trash; asking to delete it permanently"
                );
                untrashable.push(path);
            }
            // 取消也走这里：队列按"正在取消"把它记成已取消。已放入的照样能撤销。
            Err(error) => {
                return FileOperationCompletion::Failed {
                    error: error.to_string(),
                    partial_outcome: FileOperationOutcome::Trash {
                        entries,
                        untrashable,
                    },
                }
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

    FileOperationCompletion::Succeeded(FileOperationOutcome::Trash {
        entries,
        untrashable,
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

mod entry_gathering;
mod extract_members;
mod recoverable;
mod step_pacer;

#[cfg(test)]
mod archive_progress_tests;

use recoverable::{
    run_queued_transfers, send_archive_creation_progress, send_archive_extraction_progress,
    send_file_operation_progress,
};
