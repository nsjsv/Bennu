//! Bennu 的应用状态存储：一个进程、一个 `state.sqlite`。
//!
//! 数据库句柄 `StateDatabase` 每进程只开一次、只建一次表；任务队列、用户设置、浏览会话
//! 各有一个存储类型，从同一个句柄派生，互不知道对方的表。

mod browser_session;
mod database;
mod error;
mod preferences_payload;
mod recoverable_transfer;
mod shutdown;
mod stored_path;
mod task_queue;
mod user_settings;

pub use browser_session::{
    BrowserSessionStore, StoredBrowserPane, StoredBrowserPaneLayout, StoredBrowserSession,
    StoredBrowserTab, StoredBrowserViewMode, StoredColumnBrowserViewport, StoredColumnViewport,
    StoredSplitAxis,
};
pub use database::StateDatabase;
pub use error::{StoreError, StoreResult};
pub use preferences_payload::{
    UnreadablePreferences, UnreadablePreferencesBackup, UserPreferencesRead, USER_PREFERENCES_KEY,
};
pub use recoverable_transfer::{
    ClaimedRecoverableTask, RecoverableRestoreCoordinatorLease, RecoverableTaskRunnerLease,
    StoredFileIdentity, StoredFileObjectKind, StoredFileOperationVerification,
    StoredManifestCheckpointBatchUpdate, StoredManifestEntry, StoredMergeChildCompletion,
    StoredTransferCheckpoint, StoredTransferCheckpointKind, StoredTransferCheckpointSwap,
    StoredTransferConflictStrategy, StoredTransferJournalEntry, StoredTransferOperation,
    StoredTransferRecoverySnapshot, StoredTransferWorkKey, TransferManifestCheckpointUpdate,
    TRANSFER_JOURNAL_VERSION,
};
pub use shutdown::{StoredApplicationShutdown, StoredBrowserSessionShutdown};
pub use stored_path::{StoredFileName, StoredPath};
pub use task_queue::{
    StoredArchiveCompressionLevel, StoredArchiveFormat, StoredBatchRenameItem,
    StoredInterruptedRecoverableTask, StoredOperation, StoredProgress, StoredSymbolicLinkCreation,
    StoredTask, StoredTaskStatus, StoredTransfer, StoredTrashEntry, TaskQueueStore,
};
pub use user_settings::UserSettingsStore;

pub(crate) use database::current_time_ms;
pub(crate) use task_queue::sqlite_id_to_u64;

#[cfg(test)]
mod tests;
