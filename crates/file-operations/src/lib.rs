//! 文件操作编排：任务队列、可恢复传输驱动、日志接入、结算与执行。
//! 界面包只负责发起操作、展示进度与结果，不实现这些逻辑。

// 队列里的状态文案、任务标题直接写 tr!/tr_in!。
#[macro_use]
extern crate bennu_localization;

mod outcome;
mod progress;
mod queue;

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
// 队列的公开面就是它自己声明为 pub 的那些项（模块私有细节仍是 pub(super)）。
pub use queue::*;
