use std::future::Future;

use file_core::{EntryStepPacer, FileError, FileOperationControls};
use iced::futures::channel::mpsc::Sender as IcedSender;

use super::send_file_operation_progress;
use crate::model::Message;
use file_operations::FileOperationProgressUpdate;

/// file-core 多条目操作的每一步前等任务暂停结束、遇取消报错；每一步后把条目进度发给任务行。
pub(super) struct QueuedStepPacer<'a> {
    controls: &'a mut FileOperationControls,
    output: &'a mut IcedSender<Message>,
    task_id: u64,
}

impl<'a> QueuedStepPacer<'a> {
    pub(super) fn new(
        controls: &'a mut FileOperationControls,
        output: &'a mut IcedSender<Message>,
        task_id: u64,
    ) -> Self {
        Self {
            controls,
            output,
            task_id,
        }
    }
}

impl EntryStepPacer for QueuedStepPacer<'_> {
    fn before_step(&mut self) -> impl Future<Output = Result<(), FileError>> + Send {
        self.controls.wait_until_running()
    }

    fn step_finished(&mut self, completed: usize, total: usize) -> impl Future<Output = ()> + Send {
        send_file_operation_progress(
            self.output,
            self.task_id,
            FileOperationProgressUpdate::IndeterminateItems { completed, total },
        )
    }
}
