//! 文件操作编排：任务队列、可恢复传输驱动、日志接入、结算与执行。
//! 界面包只负责发起操作、展示进度与结果，不实现这些逻辑。

mod outcome;
mod progress;

pub use outcome::{
    completed_migrations_cross_directory_tree_boundary,
    completed_migrations_have_source_in_directory_tree, completed_migrations_leave_directory_tree,
    completed_migrations_touch_directory_tree, path_after_completed_migrations,
    CompletedPathMigration, CompletedTransfer, FileOperationCompletion,
    FileOperationHistoryEligibility, FileOperationOutcome, KeptNewFolder, RenameRejection,
};
pub use progress::{
    FileOperationProgress, FileOperationProgressUpdate, TransferEntrySnapshot,
    TransferEntrySnapshotState,
};
