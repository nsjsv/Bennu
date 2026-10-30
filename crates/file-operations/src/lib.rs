//! 文件操作编排：任务队列、可恢复传输驱动、日志接入、结算与执行。
//! 界面包只负责发起操作、展示进度与结果，不实现这些逻辑。

// 执行器的步骤函数各自显式接收任务上下文（控制句柄、恢复日志、事件通道、进度累加器）。
// 这条放行随代码从 app-ui 原样搬来；打包成参数结构只会换个地方列出同样的字段。
#![allow(clippy::too_many_arguments)]

// 队列里的状态文案、任务标题直接写 tr!/tr_in!。
#[macro_use]
extern crate bennu_localization;

mod events;
mod executor;
mod outcome;
mod progress;
mod queue;

pub use events::{
    DurableDirectMoveCommit, FileOperationEvent, FileOperationEventSender,
    FILE_OPERATION_EVENT_CAPACITY,
};
pub use executor::run_file_operation;
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
