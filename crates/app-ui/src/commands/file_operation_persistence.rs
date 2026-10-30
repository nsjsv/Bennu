use file_operations::{run_file_operation_persistence, FileOperationPersistenceRequest};
use iced::Task;

use crate::model::Message;

/// 队列的写库请求在后台线程执行，回执经 `Message` 交回 `accept_persistence_outcome`。
pub(crate) fn file_operation_persistence_command(
    request: FileOperationPersistenceRequest,
) -> Task<Message> {
    Task::perform(
        run_file_operation_persistence(request),
        Message::FileOperationPersistenceFinished,
    )
}
