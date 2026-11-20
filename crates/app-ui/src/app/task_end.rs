//! 队列任务结束的唯一收口：完成、失败、取消、入队后被拒、开始前被取消、恢复中断都从这里
//! 交给任务的主人：撤销重放的交给重放（它可能接着放下一项），动作里的交给那次运行
//! （它可能接着做下一步），其余的是用户直接做的，记成普通历史。

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::model::Message;
use crate::operation_history::{HistoryDirection, TaskEnd};

/// 一个队列任务是谁放进来的。只有用户直接做的才有完成后的界面收尾（进入重命名、选中副本、
/// 弹各自的完成通知）；重放和动作的任务由它们自己收尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskOwner {
    User,
    HistoryReplay,
    Action,
}

impl FileBrowser {
    pub(super) fn task_owner(&self, task_id: u64) -> TaskOwner {
        if self.operation_history.is_replaying(task_id) {
            TaskOwner::HistoryReplay
        } else if self.action_runs.owns(task_id) {
            TaskOwner::Action
        } else {
            TaskOwner::User
        }
    }

    pub(super) fn accept_task_end(&mut self, task_id: u64, end: TaskEnd<'_>) -> Task<Message> {
        match self.task_owner(task_id) {
            TaskOwner::User => {
                self.operation_history.record(end);
                Task::none()
            }
            TaskOwner::Action => self.accept_action_task_end(task_id, end),
            TaskOwner::HistoryReplay => {
                let queue = &mut self.operation_queue;
                let rejection =
                    self.operation_history
                        .accept_replay_end(task_id, end, &mut |operation| queue.enqueue(operation));
                self.after_history_enqueue(rejection)
            }
        }
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
