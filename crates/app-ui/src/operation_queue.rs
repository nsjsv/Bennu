use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use file_core::{
    ArchiveCompressionLevel, ArchiveExtractionRequest, ArchiveFormat, ArchivePassword,
    BatchRenameItem, ConversionRequest, FileOperationControls, FileOperationRunState,
    FileOperationVerification, TransferConflictStrategy, TrashRestoreEntry,
};
use file_operation_store::{
    RecoverableTaskRunnerLease, StoredInterruptedRecoverableTask, StoredOperation, StoredTask,
    StoredTaskStatus, TaskQueueStore,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::model::sanitized_application_log_detail;
use crate::operation_progress::{FileOperationProgress, FileOperationProgressUpdate};

mod persistence;
use persistence::{queued_operation_from_stored, queued_operation_to_stored};
mod persistence_worker;
#[cfg(test)]
pub(crate) use persistence_worker::execute_file_operation_persistence;
pub(crate) use persistence_worker::{
    file_operation_persistence_command, FileOperationPersistenceAction,
    FileOperationPersistenceCompletion, FileOperationPersistenceOutcome,
    FileOperationPersistenceRequest, PersistedFileOperation, PersistenceEffect,
    TaskStatePersistence,
};

pub(crate) const NEW_DIRECTORY_NAME: &str = "New Folder";
pub(crate) const NEW_FILE_NAME: &str = "New File";

const LOCAL_TASK_ID_START: u64 = 1 << 63;

#[cfg(test)]
std::thread_local! {
    static RECORDED_FILE_OPERATION_FAILURES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn record_file_operation_failure(task_id: u64, operation: &str, log_error: &str) {
    tracing::error!(
        target: "app_ui::file_operations",
        event = "file_operation_failed",
        task_id,
        operation,
        error = %log_error,
        "file operation failed"
    );
    #[cfg(test)]
    RECORDED_FILE_OPERATION_FAILURES.with(|count| count.set(count.get() + 1));
}

#[derive(Debug, Clone)]
pub(crate) enum QueuedFileOperation {
    Rename {
        path: PathBuf,
        new_name: String,
    },
    BatchRename {
        items: Vec<BatchRenameItem>,
    },
    CreateDirectory {
        parent: PathBuf,
    },
    CreateEmptyFile {
        parent: PathBuf,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    Restore {
        entries: Vec<TrashRestoreEntry>,
    },
    DeleteTrashEntries {
        entries: Vec<TrashRestoreEntry>,
    },
    DeletePermanently {
        paths: Vec<PathBuf>,
    },
    EmptyTrash,
    Copy {
        transfers: Vec<QueuedTransfer>,
        verification: FileOperationVerification,
    },
    Move {
        transfers: Vec<QueuedTransfer>,
        verification: FileOperationVerification,
    },
    CreateArchive {
        sources: Vec<PathBuf>,
        target: PathBuf,
        format: ArchiveFormat,
        compression_level: ArchiveCompressionLevel,
        password: Option<ArchivePassword>,
    },
    ExtractArchive {
        request: ArchiveExtractionRequest,
    },
    Convert {
        requests: Vec<ConversionRequest>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedTransfer {
    pub(crate) source: PathBuf,
    pub(crate) target: PathBuf,
    pub(crate) conflict_strategy: TransferConflictStrategy,
}

impl QueuedTransfer {
    pub(crate) fn new(source: PathBuf, target: PathBuf) -> Self {
        Self {
            source,
            target,
            conflict_strategy: TransferConflictStrategy::Fail,
        }
    }
}

impl QueuedFileOperation {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "Rename",
            Self::BatchRename { .. } => "Batch Rename",
            Self::CreateDirectory { .. } => "New Folder",
            Self::CreateEmptyFile { .. } => "New File",
            Self::Trash { .. } => "Move to Trash",
            Self::Restore { .. } => "Restore",
            Self::DeleteTrashEntries { .. } => "Delete Permanently",
            Self::DeletePermanently { .. } => "Delete Permanently",
            Self::EmptyTrash => "Empty Trash",
            Self::Copy { .. } => "Copy",
            Self::Move { .. } => "Move",
            Self::CreateArchive { .. } => "Create Archive",
            Self::ExtractArchive { .. } => "Extract Archive",
            Self::Convert { .. } => "Convert Format",
        }
    }

    pub(crate) fn created_path(&self) -> Option<PathBuf> {
        match self {
            Self::CreateDirectory { parent } => Some(parent.join(NEW_DIRECTORY_NAME)),
            Self::CreateEmptyFile { parent } => Some(parent.join(NEW_FILE_NAME)),
            _ => None,
        }
    }

    pub(crate) fn supports_pause(&self) -> bool {
        !matches!(
            self,
            Self::CreateArchive { .. } | Self::ExtractArchive { .. } | Self::Convert { .. }
        )
    }

    fn uses_recovery_journal(&self) -> bool {
        matches!(self, Self::Copy { .. } | Self::Move { .. })
    }

    fn to_stored(&self) -> StoredOperation {
        queued_operation_to_stored(self)
    }

    fn from_resumable_stored(operation: StoredOperation) -> Option<Self> {
        queued_operation_from_stored(operation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOperationStatus {
    Pending,
    Running,
    Paused,
    Canceling,
    Failed,
    Completed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOperationExecutionPhase {
    Preparing,
    Executing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostInsertDisposition {
    Continue,
    ShutdownRecoverable(StoredTaskStatus),
    ShutdownTransient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOperationTerminalStatus {
    Completed,
    Failed,
    Canceled,
}

impl FileOperationStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Canceling => "Canceling",
            Self::Failed => "Failed",
            Self::Completed => "Completed",
            Self::Canceled => "Canceled",
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Completed | Self::Canceled)
    }

    fn to_stored(self) -> StoredTaskStatus {
        match self {
            Self::Pending => StoredTaskStatus::Pending,
            Self::Running => StoredTaskStatus::Running,
            Self::Paused => StoredTaskStatus::Paused,
            Self::Canceling => StoredTaskStatus::Canceling,
            Self::Failed => StoredTaskStatus::Failed,
            Self::Completed => StoredTaskStatus::Completed,
            Self::Canceled => StoredTaskStatus::Canceled,
        }
    }
}

pub(crate) enum FileOperationFinish {
    Succeeded,
    SucceededWithWarning(String),
    Canceled,
    Failed(String),
    RecoveryInterrupted(String),
    RecoveryBlocked(String),
}

pub(crate) enum FileOperationEnqueueOutcome {
    Queued { task_id: u64 },
    QueuedWithStorageWarning { task_id: u64, error: String },
    Rejected { error: String },
}

impl FileOperationEnqueueOutcome {
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        match self {
            Self::Queued { .. } => None,
            Self::QueuedWithStorageWarning { error, .. } | Self::Rejected { error } => Some(error),
        }
    }
}

pub(crate) struct FileOperationTask {
    pub(crate) id: u64,
    pub(crate) operation: QueuedFileOperation,
    pub(crate) status: FileOperationStatus,
    pub(crate) progress: FileOperationProgress,
    pub(crate) completion_warning: Option<String>,
    pub(crate) error: Option<String>,
    is_read: bool,
    cancel: CancellationToken,
    _runner_lease: Option<Arc<RecoverableTaskRunnerLease>>,
    run_state_sender: watch::Sender<FileOperationRunState>,
    run_state_receiver: watch::Receiver<FileOperationRunState>,
    pub(crate) stored_id: Option<u64>,
    execution_phase: Option<FileOperationExecutionPhase>,
    post_insert_disposition: PostInsertDisposition,
    terminal_persistence_pending: bool,
    accepted_direct_move_revisions: HashMap<file_core::TransferWorkKey, u64>,
}

impl FileOperationTask {
    pub(crate) fn status_label(&self) -> &'static str {
        if self.status == FileOperationStatus::Running
            && self.execution_phase == Some(FileOperationExecutionPhase::Preparing)
        {
            "Preparing"
        } else {
            self.status.label()
        }
    }
}

pub(crate) struct RunningFileOperation {
    pub(crate) id: u64,
    pub(crate) stored_id: Option<u64>,
    pub(crate) operation: QueuedFileOperation,
    pub(crate) controls: FileOperationControls,
    pub(crate) store: Option<TaskQueueStore>,
}

pub(crate) struct FileOperationShutdownDisposition {
    pub(crate) waiting_for_operation_ids: BTreeSet<u64>,
    pub(crate) interrupted_recoverable_tasks: Vec<StoredInterruptedRecoverableTask>,
    pub(crate) transient_task_ids: Vec<u64>,
    pub(crate) stopping_signal_count: usize,
    pub(crate) journal_read_count: usize,
}

pub(crate) struct FileOperationQueue {
    tasks: Vec<FileOperationTask>,
    next_local_id: u64,
    next_persistence_request_id: u64,
    persistence_requests: VecDeque<FileOperationPersistenceRequest>,
    persistence_in_flight: Option<u64>,
    pending_deletions: BTreeSet<u64>,
    is_panel_open: bool,
    store: Option<TaskQueueStore>,
    #[cfg(test)]
    persist_synchronously_for_tests: bool,
}

impl FileOperationQueue {
    pub(crate) fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_local_id: LOCAL_TASK_ID_START,
            next_persistence_request_id: 1,
            persistence_requests: VecDeque::new(),
            persistence_in_flight: None,
            pending_deletions: BTreeSet::new(),
            is_panel_open: false,
            store: None,
            #[cfg(test)]
            persist_synchronously_for_tests: false,
        }
    }

    pub(crate) fn set_store_and_restore(&mut self, store: TaskQueueStore) -> Option<String> {
        #[cfg(test)]
        {
            self.persist_synchronously_for_tests = true;
        }
        let restore_coordinator = match store.try_acquire_recoverable_restore_coordinator() {
            Ok(Some(restore_coordinator)) => restore_coordinator,
            Ok(None) => {
                self.store = Some(store);
                return None;
            }
            Err(error) => {
                self.store = Some(store);
                return Some(storage_error(error));
            }
        };
        let stored_tasks = match store.read_tasks() {
            Ok(stored_tasks) => stored_tasks,
            Err(error) => {
                self.store = Some(store);
                return Some(storage_error(error));
            }
        };
        self.store = Some(store);
        let mut storage_error = None;

        for stored_task in stored_tasks {
            storage_error =
                combine_storage_errors(storage_error, self.restore_stored_task(stored_task));
        }
        drop(restore_coordinator);

        let storage_error = combine_storage_errors(storage_error, self.start_next());
        #[cfg(test)]
        return combine_storage_errors(storage_error, self.complete_test_persistence());
        #[cfg(not(test))]
        storage_error
    }

    pub(crate) fn task_queue_store(&self) -> Option<&TaskQueueStore> {
        self.store.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_store(&mut self, store: TaskQueueStore) {
        self.store = Some(store);
        self.persist_synchronously_for_tests = true;
    }

    #[cfg(test)]
    pub(crate) fn set_store_with_deferred_persistence(&mut self, store: TaskQueueStore) {
        self.store = Some(store);
        self.persist_synchronously_for_tests = false;
    }

    pub(crate) fn tasks(&self) -> &[FileOperationTask] {
        &self.tasks
    }

    pub(crate) fn is_panel_open(&self) -> bool {
        self.is_panel_open
    }

    pub(crate) fn open_panel(&mut self) {
        self.is_panel_open = true;
        self.mark_all_read();
    }

    pub(crate) fn close_panel(&mut self) {
        self.is_panel_open = false;
    }

    pub(crate) fn unread_count(&self) -> usize {
        self.tasks.iter().filter(|task| !task.is_read).count()
    }

    pub(crate) fn has_unread_warning_task(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| !task.is_read && task.completion_warning.is_some())
    }

    pub(crate) fn has_unread_failed_task(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| !task.is_read && task.status == FileOperationStatus::Failed)
    }

    pub(crate) fn enqueue(
        &mut self,
        operation: QueuedFileOperation,
    ) -> FileOperationEnqueueOutcome {
        let requires_recovery_journal = operation.uses_recovery_journal();
        if self.store.is_none() && requires_recovery_journal {
            return FileOperationEnqueueOutcome::Rejected {
                error:
                    "File operation queue storage is unavailable; copy and move were not started"
                        .to_owned(),
            };
        }
        let id = self.allocate_local_id();
        let (run_state_sender, run_state_receiver) = watch::channel(FileOperationRunState::Running);
        let is_read = self.is_panel_open;
        self.tasks.push(FileOperationTask {
            id,
            operation: operation.clone(),
            status: FileOperationStatus::Pending,
            progress: FileOperationProgress::pending(),
            completion_warning: None,
            error: None,
            is_read,
            cancel: CancellationToken::new(),
            _runner_lease: None,
            run_state_sender,
            run_state_receiver,
            stored_id: None,
            execution_phase: None,
            post_insert_disposition: PostInsertDisposition::Continue,
            terminal_persistence_pending: false,
            accepted_direct_move_revisions: HashMap::new(),
        });

        if let Some(store) = self.store.clone() {
            self.queue_persistence_action(FileOperationPersistenceAction::Insert {
                ui_task_id: id,
                store,
                operation: operation.to_stored(),
                requires_recovery_journal,
            });
        }
        let _ = self.start_next();

        #[cfg(test)]
        let persistence_error = self.complete_test_persistence();
        #[cfg(not(test))]
        let persistence_error = None;

        #[cfg(test)]
        let returned_id = self.tasks.last().map(|task| task.id).unwrap_or(id);
        #[cfg(not(test))]
        let returned_id = id;

        if let Some(error) = persistence_error {
            if requires_recovery_journal
                && self
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .is_some_and(|task| task.status == FileOperationStatus::Failed)
            {
                self.tasks.retain(|task| task.id != id);
                return FileOperationEnqueueOutcome::Rejected { error };
            }
            return FileOperationEnqueueOutcome::QueuedWithStorageWarning {
                task_id: returned_id,
                error,
            };
        }
        FileOperationEnqueueOutcome::Queued {
            task_id: returned_id,
        }
    }

    pub(crate) fn active_subscription(&self) -> Option<RunningFileOperation> {
        self.tasks
            .iter()
            .find(|task| {
                matches!(
                    task.status,
                    FileOperationStatus::Running
                        | FileOperationStatus::Paused
                        | FileOperationStatus::Canceling
                ) && (!task.operation.uses_recovery_journal() || task.stored_id.is_some())
            })
            .map(|task| RunningFileOperation {
                id: task.id,
                stored_id: task.stored_id,
                operation: task.operation.clone(),
                controls: FileOperationControls::new(
                    task.cancel.clone(),
                    task.run_state_receiver.clone(),
                ),
                store: self.store.clone(),
            })
    }

    pub(crate) fn accept_durable_direct_move_commit(
        &mut self,
        id: u64,
        work_key: &file_core::TransferWorkKey,
        source: &std::path::Path,
        intent_revision: u64,
    ) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) else {
            return false;
        };
        if intent_revision == 0
            || !matches!(
                task.status,
                FileOperationStatus::Running
                    | FileOperationStatus::Paused
                    | FileOperationStatus::Canceling
            )
            || task.accepted_direct_move_revisions.contains_key(work_key)
        {
            return false;
        }
        let QueuedFileOperation::Move { transfers, .. } = &task.operation else {
            return false;
        };
        let Some(transfer) = usize::try_from(work_key.transfer_index)
            .ok()
            .and_then(|index| transfers.get(index))
        else {
            return false;
        };
        let expected_source = if work_key.relative_path.as_os_str().is_empty() {
            transfer.source.clone()
        } else {
            transfer.source.join(&work_key.relative_path)
        };
        if expected_source != source {
            return false;
        }
        task.accepted_direct_move_revisions
            .insert(work_key.clone(), intent_revision);
        task.execution_phase = Some(FileOperationExecutionPhase::Executing);
        true
    }

    pub(crate) fn operation(&self, id: u64) -> Option<&QueuedFileOperation> {
        self.tasks
            .iter()
            .find(|task| task.id == id)
            .map(|task| &task.operation)
    }

    pub(crate) fn has_terminal_tasks(&self) -> bool {
        self.tasks.iter().any(|task| task.status.is_terminal())
    }

    pub(crate) fn clear_terminal_task(&mut self, id: u64) -> Option<String> {
        let task_ids = self
            .tasks
            .iter()
            .filter(|task| {
                task.id == id
                    && task.status.is_terminal()
                    && !self.pending_deletions.contains(&task.id)
                    && self.can_clear_terminal_task(task)
            })
            .map(|task| task.id)
            .collect::<BTreeSet<_>>();
        self.clear_terminal_task_ids(&task_ids)
    }

    pub(crate) fn clear_terminal_tasks(&mut self) -> Option<String> {
        let task_ids = self
            .tasks
            .iter()
            .filter(|task| {
                task.status.is_terminal()
                    && !self.pending_deletions.contains(&task.id)
                    && self.can_clear_terminal_task(task)
            })
            .map(|task| task.id)
            .collect::<BTreeSet<_>>();
        self.clear_terminal_task_ids(&task_ids)
    }

    fn can_clear_terminal_task(&self, task: &FileOperationTask) -> bool {
        if task.stored_id.is_some() || self.store.is_none() {
            return !task.terminal_persistence_pending;
        }
        if task.status != FileOperationStatus::Canceled {
            return true;
        }
        self.persistence_in_flight.is_none()
            && !self.persistence_requests.iter().any(|request| {
                matches!(
                    &request.action,
                    FileOperationPersistenceAction::Insert { ui_task_id, .. }
                        if *ui_task_id == task.id
                )
            })
    }

    fn clear_terminal_task_ids(&mut self, task_ids: &BTreeSet<u64>) -> Option<String> {
        if task_ids.is_empty() {
            return None;
        }
        let persisted = self
            .tasks
            .iter()
            .filter(|task| task_ids.contains(&task.id))
            .filter_map(|task| task.stored_id.map(|stored_id| (task.id, stored_id)))
            .collect::<Vec<_>>();
        let persisted_ui_ids = persisted
            .iter()
            .map(|(ui_task_id, _)| *ui_task_id)
            .collect::<BTreeSet<_>>();
        self.tasks
            .retain(|task| !task_ids.contains(&task.id) || persisted_ui_ids.contains(&task.id));
        if persisted.is_empty() {
            return None;
        }

        self.pending_deletions
            .extend(persisted.iter().map(|(ui_task_id, _)| *ui_task_id));
        self.queue_persistence_action(FileOperationPersistenceAction::DeleteTasks {
            store: self
                .store
                .clone()
                .expect("persisted file operation task has an operation store"),
            stored_task_ids: persisted
                .iter()
                .map(|(_, stored_task_id)| *stored_task_id)
                .collect(),
            ui_task_ids: persisted
                .iter()
                .map(|(ui_task_id, _)| *ui_task_id)
                .collect(),
        });
        #[cfg(test)]
        return self.complete_test_persistence();
        #[cfg(not(test))]
        None
    }

    pub(crate) fn update_progress(
        &mut self,
        id: u64,
        update: FileOperationProgressUpdate,
    ) -> Option<String> {
        let position = self.tasks.iter().position(|task| task.id == id)?;
        if !matches!(
            self.tasks[position].status,
            FileOperationStatus::Running | FileOperationStatus::Canceling
        ) {
            return None;
        }
        self.tasks[position].execution_phase = Some(FileOperationExecutionPhase::Executing);
        self.tasks[position].progress.update(update);
        None
    }

    pub(crate) fn finish(
        &mut self,
        id: u64,
        outcome: FileOperationFinish,
    ) -> (Option<FileOperationTerminalStatus>, Option<String>) {
        let terminal_status = self.finish_without_advancing_queue(id, outcome);
        let _ = self.start_next();
        #[cfg(test)]
        let storage_error = self.complete_test_persistence();
        #[cfg(not(test))]
        let storage_error = None;
        (terminal_status, storage_error)
    }

    pub(crate) fn finish_for_application_shutdown(
        &mut self,
        id: u64,
        outcome: FileOperationFinish,
    ) -> (
        Option<FileOperationTerminalStatus>,
        Option<String>,
        Option<u64>,
    ) {
        #[cfg(test)]
        let recoverable_stored_id = self
            .tasks
            .iter()
            .find(|task| task.id == id && task.operation.uses_recovery_journal())
            .and_then(|task| task.stored_id);
        let terminal_status = self.finish_without_advancing_queue(id, outcome);
        #[cfg(test)]
        let storage_error = self.complete_test_persistence();
        #[cfg(not(test))]
        let storage_error = None;
        #[cfg(test)]
        let persisted_terminal_stored_id = storage_error
            .is_none()
            .then_some(recoverable_stored_id)
            .flatten();
        #[cfg(not(test))]
        let persisted_terminal_stored_id = None;
        (terminal_status, storage_error, persisted_terminal_stored_id)
    }

    fn finish_without_advancing_queue(
        &mut self,
        id: u64,
        outcome: FileOperationFinish,
    ) -> Option<FileOperationTerminalStatus> {
        let position = self.tasks.iter().position(|task| task.id == id)?;
        if !matches!(
            self.tasks[position].status,
            FileOperationStatus::Running
                | FileOperationStatus::Paused
                | FileOperationStatus::Canceling
        ) {
            return None;
        }

        let was_canceling = self.tasks[position].status == FileOperationStatus::Canceling;
        let (stored_status, persistence) = match outcome {
            FileOperationFinish::Succeeded => {
                let task = &mut self.tasks[position];
                task.status = FileOperationStatus::Completed;
                task.progress.mark_complete();
                task.completion_warning = None;
                task.error = None;
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::Completed,
                    terminal_persistence(&task.operation),
                )
            }
            FileOperationFinish::SucceededWithWarning(warning) => {
                let task = &mut self.tasks[position];
                task.status = FileOperationStatus::Completed;
                task.progress.mark_complete();
                task.completion_warning = Some(warning);
                task.error = None;
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::Completed,
                    terminal_persistence(&task.operation),
                )
            }
            FileOperationFinish::Canceled => {
                let task = &mut self.tasks[position];
                task.status = FileOperationStatus::Canceled;
                task.error = None;
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::Canceled,
                    terminal_persistence(&task.operation),
                )
            }
            FileOperationFinish::Failed(error)
                if was_canceling && !self.tasks[position].operation.uses_recovery_journal() =>
            {
                let task = &mut self.tasks[position];
                drop(error);
                task.status = FileOperationStatus::Canceled;
                task.error = None;
                task.is_read = self.is_panel_open;
                (StoredTaskStatus::Canceled, TaskStatePersistence::Update)
            }
            FileOperationFinish::Failed(error) => {
                let task = &mut self.tasks[position];
                let log_error = sanitized_application_log_detail(&error);
                record_file_operation_failure(id, task.operation.title(), &log_error);
                task.status = FileOperationStatus::Failed;
                task.error = Some(error);
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::Failed,
                    terminal_persistence(&task.operation),
                )
            }
            FileOperationFinish::RecoveryInterrupted(error) => {
                let task = &mut self.tasks[position];
                let log_error = sanitized_application_log_detail(&error);
                record_file_operation_failure(id, task.operation.title(), &log_error);
                task.status = FileOperationStatus::Failed;
                task.error = Some(error);
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::RecoveryPending,
                    TaskStatePersistence::PreserveRecovery,
                )
            }
            FileOperationFinish::RecoveryBlocked(error) => {
                let task = &mut self.tasks[position];
                let log_error = sanitized_application_log_detail(&error);
                record_file_operation_failure(id, task.operation.title(), &log_error);
                task.status = FileOperationStatus::Failed;
                task.error = Some(error);
                task.is_read = self.is_panel_open;
                (
                    StoredTaskStatus::Failed,
                    TaskStatePersistence::PreserveRecovery,
                )
            }
        };
        self.tasks[position].execution_phase = None;
        if self.tasks[position].operation.uses_recovery_journal()
            && matches!(
                persistence,
                TaskStatePersistence::FinalizeRecovery | TaskStatePersistence::PreserveRecovery
            )
        {
            self.tasks[position].terminal_persistence_pending = true;
        }
        self.queue_task_state(position, stored_status, persistence);
        let terminal_status = match self.tasks[position].status {
            FileOperationStatus::Completed => FileOperationTerminalStatus::Completed,
            FileOperationStatus::Failed => FileOperationTerminalStatus::Failed,
            FileOperationStatus::Canceled => FileOperationTerminalStatus::Canceled,
            FileOperationStatus::Pending
            | FileOperationStatus::Running
            | FileOperationStatus::Paused
            | FileOperationStatus::Canceling => {
                unreachable!("file operation finish did not produce a terminal status");
            }
        };

        Some(terminal_status)
    }

    pub(crate) fn toggle_pause(&mut self, id: u64) -> Option<String> {
        let position = self.tasks.iter().position(|task| task.id == id)?;
        match self.tasks[position].status {
            FileOperationStatus::Running => {
                let task = &mut self.tasks[position];
                let _ = task.run_state_sender.send(FileOperationRunState::Paused);
                task.status = FileOperationStatus::Paused;
                self.queue_task_status(position);
            }
            FileOperationStatus::Paused => {
                let task = &mut self.tasks[position];
                let _ = task.run_state_sender.send(FileOperationRunState::Running);
                task.status = FileOperationStatus::Running;
                self.queue_task_status(position);
            }
            _ => return None,
        }
        #[cfg(test)]
        return self.complete_test_persistence();
        #[cfg(not(test))]
        None
    }

    pub(crate) fn cancel(&mut self, id: u64) -> Option<String> {
        let position = self.tasks.iter().position(|task| task.id == id)?;

        match self.tasks[position].status {
            FileOperationStatus::Pending
                if self.tasks[position].operation.uses_recovery_journal() =>
            {
                let task = &mut self.tasks[position];
                task.cancel.cancel();
                let _ = task.run_state_sender.send(FileOperationRunState::Running);
                task.status = FileOperationStatus::Canceling;
                task.error = None;
                task.is_read = self.is_panel_open;
                self.queue_task_status(position);
            }
            FileOperationStatus::Pending => {
                let task = &mut self.tasks[position];
                task.status = FileOperationStatus::Canceled;
                task.error = None;
                task.is_read = self.is_panel_open;
                self.queue_task_state(
                    position,
                    StoredTaskStatus::Canceled,
                    TaskStatePersistence::Update,
                );
            }
            FileOperationStatus::Running | FileOperationStatus::Paused => {
                let task = &mut self.tasks[position];
                task.cancel.cancel();
                let _ = task.run_state_sender.send(FileOperationRunState::Running);
                task.status = FileOperationStatus::Canceling;
                task.is_read = self.is_panel_open;
                self.queue_task_status(position);
            }
            FileOperationStatus::Canceling
            | FileOperationStatus::Failed
            | FileOperationStatus::Completed
            | FileOperationStatus::Canceled => return None,
        }

        let _ = self.start_next();
        #[cfg(test)]
        return self.complete_test_persistence();
        #[cfg(not(test))]
        None
    }

    pub(crate) fn begin_application_shutdown(&mut self) -> FileOperationShutdownDisposition {
        let mut waiting_for_operation_ids = BTreeSet::new();
        let mut interrupted_recoverable_tasks = Vec::new();
        let mut transient_task_ids = Vec::new();
        let mut stopping_signal_count = 0;

        for task in &mut self.tasks {
            let is_recoverable = task.operation.uses_recovery_journal();
            let is_waiting_for_insert = task.stored_id.is_none()
                && ((is_recoverable
                    && matches!(
                        task.status,
                        FileOperationStatus::Pending | FileOperationStatus::Canceling
                    ))
                    || (!is_recoverable && task.status == FileOperationStatus::Canceled));
            if is_waiting_for_insert {
                match is_recoverable {
                    true => {
                        let shutdown_status = if task.status == FileOperationStatus::Canceling {
                            StoredTaskStatus::Canceling
                        } else {
                            StoredTaskStatus::RecoveryPending
                        };
                        task.post_insert_disposition =
                            PostInsertDisposition::ShutdownRecoverable(shutdown_status);
                    }
                    false => {
                        task.status = FileOperationStatus::Canceled;
                        task.post_insert_disposition = PostInsertDisposition::ShutdownTransient;
                    }
                }
                continue;
            }
            let is_active = matches!(
                task.status,
                FileOperationStatus::Running
                    | FileOperationStatus::Paused
                    | FileOperationStatus::Canceling
            );
            let is_recoverable = task.operation.uses_recovery_journal() && task.stored_id.is_some();
            if is_active {
                waiting_for_operation_ids.insert(task.id);
                let _ = task
                    .run_state_sender
                    .send(FileOperationRunState::ApplicationStopping);
                stopping_signal_count += 1;
                if !is_recoverable {
                    task.cancel.cancel();
                }
            }

            if is_recoverable {
                if is_active || task.terminal_persistence_pending {
                    interrupted_recoverable_tasks.push(StoredInterruptedRecoverableTask {
                        task_id: task.stored_id.expect("recoverable task is persisted"),
                        status: if is_active && task.status == FileOperationStatus::Canceling {
                            StoredTaskStatus::Canceling
                        } else {
                            StoredTaskStatus::RecoveryPending
                        },
                        progress: task.progress.to_stored(),
                        error: Some("Application stopped with recoverable work pending".to_owned()),
                    });
                }
                continue;
            }

            if let Some(stored_id) = task.stored_id {
                transient_task_ids.push(stored_id);
            }
        }
        self.is_panel_open = false;

        FileOperationShutdownDisposition {
            waiting_for_operation_ids,
            interrupted_recoverable_tasks,
            transient_task_ids,
            stopping_signal_count,
            journal_read_count: 0,
        }
    }

    pub(crate) fn release_application_shutdown_ownership(&mut self) {
        self.tasks.clear();
        self.is_panel_open = false;
    }

    pub(crate) fn operation_uses_recovery_journal(&self, id: u64) -> bool {
        self.tasks
            .iter()
            .find(|task| task.id == id)
            .is_some_and(|task| task.operation.uses_recovery_journal())
    }

    pub(crate) fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub(crate) fn is_active_paused(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.status == FileOperationStatus::Paused)
    }

    pub(crate) fn has_active_task(&self) -> bool {
        self.tasks.iter().any(|task| {
            matches!(
                task.status,
                FileOperationStatus::Running
                    | FileOperationStatus::Paused
                    | FileOperationStatus::Canceling
            )
        })
    }

    pub(crate) fn has_active_indeterminate_progress(&self) -> bool {
        self.tasks.iter().any(|task| {
            matches!(
                task.status,
                FileOperationStatus::Running | FileOperationStatus::Canceling
            ) && task.progress.fraction().is_none()
        })
    }

    pub(crate) fn indicator_progress(&self) -> Option<f32> {
        self.tasks
            .iter()
            .find(|task| {
                matches!(
                    task.status,
                    FileOperationStatus::Running
                        | FileOperationStatus::Paused
                        | FileOperationStatus::Canceling
                )
            })
            .and_then(|task| task.progress.fraction())
    }
}

mod runtime;
pub(crate) use runtime::PersistedShutdownFileOperation;

fn storage_error(error: impl std::fmt::Display) -> String {
    format!("File operation queue storage failed: {error}")
}

fn combine_storage_errors(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(error), None) | (None, Some(error)) => Some(error),
        (None, None) => None,
    }
}

fn terminal_persistence(operation: &QueuedFileOperation) -> TaskStatePersistence {
    if operation.uses_recovery_journal() {
        TaskStatePersistence::FinalizeRecovery
    } else {
        TaskStatePersistence::Update
    }
}

fn stored_status_is_terminal(status: StoredTaskStatus) -> bool {
    matches!(
        status,
        StoredTaskStatus::Failed | StoredTaskStatus::Completed | StoredTaskStatus::Canceled
    )
}

#[cfg(test)]
mod progress_tests;
#[cfg(test)]
mod tests;
