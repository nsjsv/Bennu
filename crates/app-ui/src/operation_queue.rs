use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use bennu_state::{
    RecoverableTaskRunnerLease, StateDatabase, StoredInterruptedRecoverableTask, StoredOperation,
    StoredTask, StoredTaskStatus, TaskQueueStore,
};
use file_core::{
    ArchiveCompressionLevel, ArchiveExtractionRequest, ArchiveFormat, ArchivePassword,
    BatchRenameItem, ConversionRequest, EntryNameKind, FileOperationControls,
    FileOperationRunState, FileOperationVerification, TransferConflictStrategy, TrashRestoreEntry,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::model::sanitized_application_log_detail;
use file_operations::{
    FileOperationProgress, FileOperationProgressUpdate, TransferEntrySnapshot,
    TransferEntrySnapshotState,
};

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

/// 新建、收纳时的首选名，跟界面语言（同 GNOME、KDE）；被占用时 file-core 续编号。
pub(crate) fn new_directory_name() -> String {
    tr!("New Folder")
}

pub(crate) fn new_file_name() -> String {
    tr!("New File")
}

const LOCAL_TASK_ID_START: u64 = 1 << 63;

fn record_file_operation_failure(task_id: u64, operation: &str, log_error: &str) {
    tracing::error!(
        target: "app_ui::file_operations",
        event = "file_operation_failed",
        task_id,
        operation,
        error = %log_error,
        "file operation failed"
    );
}

#[derive(Debug, Clone)]
pub(crate) enum QueuedFileOperation {
    Rename {
        path: PathBuf,
        new_name: OsString,
    },
    BatchRename {
        items: Vec<BatchRenameItem>,
    },
    CreateDirectory {
        parent: PathBuf,
    },
    /// 高级新建文件夹：批量创建多个目录（名字由弹窗预生成），
    /// 非空 gather_sources 在创建完成后整批移入第一个目录。
    CreateDirectories {
        parent: PathBuf,
        names: Vec<String>,
        gather_sources: Vec<PathBuf>,
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
    /// 右键「复制副本」:与 Copy 同一条恢复管线,仅多一个
    /// 「完成后选中新副本」的收尾语义(见 selects_completed_copies)。
    Duplicate {
        transfers: Vec<QueuedTransfer>,
        verification: FileOperationVerification,
    },
    Move {
        transfers: Vec<QueuedTransfer>,
        verification: FileOperationVerification,
    },
    /// 用选中项新建文件夹:目录已由入队侧按命名规则起好名,
    /// 选中条目随之移入;撤销时整批还原(见 UngatherNewFolder)。
    GatherSelectionIntoNewFolder {
        directory: PathBuf,
        sources: Vec<PathBuf>,
    },
    /// 收纳与"高级新建并移入"的撤销:条目从 `directory` 移回原位,再逆序删除
    /// `created_directories`(按创建顺序)中已空的目录。
    UngatherNewFolder {
        directory: PathBuf,
        restore_targets: Vec<PathBuf>,
        created_directories: Vec<PathBuf>,
    },
    CreateSymbolicLinks {
        links: Vec<SymbolicLinkCreation>,
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
    /// 包内成员复制/拖出到真实目录:按成员提取落地,不覆盖既有内容。
    /// 密码只存在内存里供本次重试使用,不参与持久化。
    ExtractArchiveMembers {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        password: Option<ArchivePassword>,
    },
    Convert {
        requests: Vec<ConversionRequest>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolicLinkCreation {
    /// 首选链接路径；被占用时执行端按"名字 2"续编号，按 `name_kind` 决定是否拆扩展名。
    pub(crate) link_path: PathBuf,
    pub(crate) name_kind: EntryNameKind,
    /// 符号链接目标一律存绝对路径。
    pub(crate) target_path: PathBuf,
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
    /// 会改动回收站内容的批量操作。运行期间 UI 挂起 watcher 触发的回收站
    /// 重扫,任务终结后统一补刷一次。
    pub(crate) fn changes_trash(&self) -> bool {
        matches!(
            self,
            Self::Trash { .. }
                | Self::Restore { .. }
                | Self::DeleteTrashEntries { .. }
                | Self::EmptyTrash
        )
    }

    pub(crate) fn title(&self) -> String {
        self.title_in(bennu_localization::current_language())
    }

    pub(crate) fn title_in(&self, language: crate::config::UiLanguage) -> String {
        match self {
            Self::Rename { .. } => tr_in!(language, "Rename"),
            Self::BatchRename { .. } => tr_in!(language, "Batch Rename"),
            Self::CreateDirectory { .. } => tr_in!(language, "New Folder"),
            Self::CreateDirectories { .. } => tr_in!(language, "New Folders"),
            Self::CreateEmptyFile { .. } => tr_in!(language, "New File"),
            Self::Trash { .. } => tr_in!(language, "Move to Trash"),
            Self::Restore { .. } => tr_in!(language, "Restore"),
            Self::DeleteTrashEntries { .. } => tr_in!(language, "Delete Permanently"),
            Self::DeletePermanently { .. } => tr_in!(language, "Delete Permanently"),
            Self::EmptyTrash => tr_in!(language, "Empty Trash"),
            Self::Copy { .. } => tr_in!(language, "Copy"),
            Self::Duplicate { .. } => tr_in!(language, "Duplicate"),
            Self::Move { .. } => tr_in!(language, "Move"),
            Self::GatherSelectionIntoNewFolder { .. } | Self::UngatherNewFolder { .. } => {
                tr_in!(language, "New Folder with Selection")
            }
            Self::CreateSymbolicLinks { .. } => tr_in!(language, "Create Symbolic Link"),
            Self::CreateArchive { .. } => tr_in!(language, "Create Archive"),
            Self::ExtractArchive { .. } | Self::ExtractArchiveMembers { .. } => {
                tr_in!(language, "Extract Archive")
            }
            Self::Convert { .. } => tr_in!(language, "Convert Format"),
        }
    }

    pub(crate) fn created_path(&self) -> Option<PathBuf> {
        match self {
            Self::CreateDirectory { parent } => Some(parent.join(new_directory_name())),
            // 预览里第一个目录就是后续移入/重命名态的落点。
            Self::CreateDirectories { parent, names, .. } => {
                names.first().map(|name| parent.join(name))
            }
            Self::CreateEmptyFile { parent } => Some(parent.join(new_file_name())),
            // 收纳文件夹在入队侧已按命名规则确定路径,完成后照常进入重命名态。
            Self::GatherSelectionIntoNewFolder { directory, .. } => Some(directory.clone()),
            _ => None,
        }
    }

    /// 「复制副本」完成后整批选中新副本;其余操作无此语义。副本名可能被复制
    /// 引擎续编号,选中按完成结果里的实际目标,不按这里的首选目标。
    pub(crate) fn selects_completed_copies(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }

    pub(crate) fn supports_pause(&self) -> bool {
        !matches!(
            self,
            Self::CreateArchive { .. } | Self::ExtractArchive { .. } | Self::Convert { .. }
        )
    }

    fn uses_recovery_journal(&self) -> bool {
        matches!(
            self,
            Self::Copy { .. } | Self::Duplicate { .. } | Self::Move { .. }
        )
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
    pub(crate) fn label(self) -> String {
        match self {
            Self::Pending => tr!("Pending"),
            Self::Running => tr!("Running"),
            Self::Paused => tr!("Paused"),
            Self::Canceling => tr!("Canceling"),
            Self::Failed => tr!("Failed"),
            Self::Completed => tr!("Completed"),
            Self::Canceled => tr!("Canceled"),
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

/// 入队只决定任务能否开始;写库结果经 `accept_persistence_outcome` 的回执返回。
pub(crate) enum FileOperationEnqueueOutcome {
    Queued { task_id: u64 },
    Rejected { error: String },
}

pub(crate) struct FileOperationTask {
    pub(crate) id: u64,
    pub(crate) operation: QueuedFileOperation,
    pub(crate) status: FileOperationStatus,
    pub(crate) progress: FileOperationProgress,
    /// 传入条目的逐条目快照(占位行单一事实源)。入队即初始化:
    /// Copy/Duplicate 全量 Queued(占位立即出现),Move 置空——同盘移动
    /// 走瞬时 rename 全程无字节进度,跨盘 move 从首条复制进度起出现。
    pub(crate) transfer_progress: Vec<TransferEntrySnapshot>,
    /// 任务入队时刻;占位行在 Modified 排序下的缺省键(≈最新)。
    pub(crate) enqueued_at: std::time::SystemTime,
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
    /// 当前驱动者代数;监督换代时递增,旧代驱动者的迟到终态凭此被拒。
    pub(crate) driver_generation: u64,
    /// 是否已签发驱动者(Task::stream)。监督换代时清零等待重新签发。
    pub(crate) driver_running: bool,
    /// 最近一次该任务驱动者的生命信号(任意进度/终态消息)。
    pub(crate) last_driver_signal: Option<std::time::Instant>,
}

impl FileOperationTask {
    pub(crate) fn status_label(&self) -> String {
        if self.status == FileOperationStatus::Running
            && self.execution_phase == Some(FileOperationExecutionPhase::Preparing)
        {
            tr!("Preparing")
        } else {
            self.status.label()
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.status,
            FileOperationStatus::Pending
                | FileOperationStatus::Running
                | FileOperationStatus::Paused
                | FileOperationStatus::Canceling
        )
    }
}

pub(crate) struct RunningFileOperation {
    pub(crate) id: u64,
    pub(crate) stored_id: Option<u64>,
    pub(crate) operation: QueuedFileOperation,
    pub(crate) controls: FileOperationControls,
    pub(crate) store: Option<TaskQueueStore>,
    pub(crate) generation: u64,
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
    /// 下次允许扫 store 认领外来任务的时刻(监督节流)。
    next_store_scan: Option<std::time::Instant>,
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
            next_store_scan: None,
        }
    }

    pub(crate) fn set_store_and_restore(&mut self, store: TaskQueueStore) -> Option<String> {
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

        self.start_next();
        storage_error
    }

    /// 进程的状态库句柄：设置、浏览会话与关机提交各自从它派生存储，不再各开一份连接。
    pub(crate) fn state_database(&self) -> Option<StateDatabase> {
        self.store.as_ref().map(|store| store.database().clone())
    }

    pub(crate) fn tasks(&self) -> &[FileOperationTask] {
        &self.tasks
    }

    /// 是否存在仍在运行(未到终态)的回收站变更类批量任务。
    pub(crate) fn has_running_trash_change_operation(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.is_running() && task.operation.changes_trash())
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
            transfer_progress: initial_transfer_progress(&operation),
            enqueued_at: std::time::SystemTime::now(),
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
            driver_generation: 0,
            driver_running: false,
            last_driver_signal: None,
        });

        if let Some(store) = self.store.clone() {
            self.queue_persistence_action(FileOperationPersistenceAction::Insert {
                ui_task_id: id,
                store,
                operation: operation.to_stored(),
                requires_recovery_journal,
            });
        }
        self.start_next();

        // 写库结果经 accept_persistence_outcome 的回执返回:初始写库失败时
        // 可恢复任务在那里被移除并报告 rejected_task_id。
        FileOperationEnqueueOutcome::Queued { task_id: id }
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
                generation: task.driver_generation,
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

    pub(crate) fn clear_terminal_task(&mut self, id: u64) {
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

    pub(crate) fn clear_terminal_tasks(&mut self) {
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

    fn clear_terminal_task_ids(&mut self, task_ids: &BTreeSet<u64>) {
        if task_ids.is_empty() {
            return;
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
            return;
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
    }

    pub(crate) fn update_progress(
        &mut self,
        id: u64,
        update: FileOperationProgressUpdate,
        transfer_snapshots: Vec<TransferEntrySnapshot>,
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
        // 快照流只由恢复式传输的后台发送;空向量表示本消息不携带逐条目
        // 语义(直移 rename 结算/非传输操作),保留已有快照不回退。
        if !transfer_snapshots.is_empty() {
            self.tasks[position].transfer_progress = transfer_snapshots;
        }
        None
    }

    pub(crate) fn finish(
        &mut self,
        id: u64,
        outcome: FileOperationFinish,
    ) -> Option<FileOperationTerminalStatus> {
        let terminal_status = self.finish_without_advancing_queue(id, outcome);
        self.start_next();
        terminal_status
    }

    /// 关机期间不再启动下一个任务;终态写库完成后由回执的
    /// `persisted_recoverable_terminal_stored_id` 推进关机流程。
    pub(crate) fn finish_for_application_shutdown(
        &mut self,
        id: u64,
        outcome: FileOperationFinish,
    ) {
        self.finish_without_advancing_queue(id, outcome);
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
                record_file_operation_failure(id, &task.operation.title(), &log_error);
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
                record_file_operation_failure(id, &task.operation.title(), &log_error);
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
                record_file_operation_failure(id, &task.operation.title(), &log_error);
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
        // 任务到终态,驱动者不复存在;标记清零,与监督的存活判断保持一致。
        self.tasks[position].driver_running = false;
        self.tasks[position].last_driver_signal = None;
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

    pub(crate) fn toggle_pause(&mut self, id: u64) {
        let Some(position) = self.tasks.iter().position(|task| task.id == id) else {
            return;
        };
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
                // 暂停期间驱动者合法静默;恢复后重计信号超时,防误判死亡。
                task.last_driver_signal = Some(std::time::Instant::now());
                self.queue_task_status(position);
            }
            _ => {}
        }
    }

    pub(crate) fn cancel(&mut self, id: u64) {
        let Some(position) = self.tasks.iter().position(|task| task.id == id) else {
            return;
        };

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
            | FileOperationStatus::Canceled => return,
        }

        self.start_next();
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
mod supervision;
pub(crate) use supervision::SUPERVISION_INTERVAL;

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

/// 入队/恢复时的占位初值:Copy/Duplicate 全部条目以 Queued 快照出现
/// (占位立即可见,空环),Move 置空留待运行期按实际复制进度出现。
/// is_directory 在边界处对源做一次 metadata 判定;恢复时源可能已不存在,
/// 判定失败按文件图标处理,首条进度快照会用清单事实替换。
fn initial_transfer_progress(operation: &QueuedFileOperation) -> Vec<TransferEntrySnapshot> {
    let transfers = match operation {
        QueuedFileOperation::Copy { transfers, .. }
        | QueuedFileOperation::Duplicate { transfers, .. } => transfers,
        _ => return Vec::new(),
    };
    transfers
        .iter()
        .map(|transfer| TransferEntrySnapshot {
            target: transfer.target.clone(),
            is_directory: std::fs::metadata(&transfer.source)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false),
            completed_bytes: None,
            total_bytes: None,
            state: TransferEntrySnapshotState::Queued,
        })
        .collect()
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
pub(crate) mod test_persistence;
#[cfg(test)]
mod tests;
