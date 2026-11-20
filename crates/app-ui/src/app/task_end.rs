//! 队列任务结束的唯一收口：完成、失败、取消、入队后被拒、开始前被取消、恢复中断都从这里
//! 交给撤销历史。撤销重放的任务交给重放（它可能接着放下一项），其余的记成普通历史。

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::model::Message;
use crate::operation_history::{HistoryDirection, TaskEnd};

impl FileBrowser {
    pub(super) fn accept_task_end(&mut self, task_id: u64, end: TaskEnd<'_>) -> Task<Message> {
        if !self.operation_history.is_replaying(task_id) {
            self.operation_history.record(end);
            return Task::none();
        }
        let queue = &mut self.operation_queue;
        let rejection = self
            .operation_history
            .accept_replay_end(task_id, end, &mut |operation| queue.enqueue(operation));
        self.after_history_enqueue(rejection)
    }

    pub(super) fn undo_file_operation(&mut self) -> Task<Message> {
        self.replay_history(HistoryDirection::Undo)
    }

    pub(super) fn redo_file_operation(&mut self) -> Task<Message> {
        self.replay_history(HistoryDirection::Redo)
    }

    fn replay_history(&mut self, direction: HistoryDirection) -> Task<Message> {
        self.context_menu = None;
        self.retract_global_error(GlobalErrorSource::FileOperationEnqueue);
        let queue = &mut self.operation_queue;
        let rejection = self
            .operation_history
            .request(direction, &mut |operation| queue.enqueue(operation));
        self.after_history_enqueue(rejection)
    }

    /// 历史往队列里放了操作之后：入队被拒就报出来，否则让队列接着写库、启动。
    fn after_history_enqueue(&mut self, rejection: Option<String>) -> Task<Message> {
        if let Some(error) = rejection {
            self.show_global_error_from(GlobalErrorSource::FileOperationEnqueue, error);
        }
        Task::batch([
            self.continue_file_operation_persistence(),
            self.launch_ready_driver(),
        ])
    }
}
