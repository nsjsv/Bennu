//! 接收任务：接受了的一次接收进任务面板，按文件累计进度，可以在这台电脑上取消。
//! 还有面板与设备行共用的取消、清除入口。

use iced::Task;
use local_send::{FileMetadata, ReceiveReport};

use super::super::FileBrowser;
use super::history::NewRecord;
use super::tasks::{ReceivePhase, ReceiveSource, ReceiveTask, TransferKind};
use crate::model::Message;

impl FileBrowser {
    /// 收下了一次接收（LocalSend 设备被自动收下或用户确认、手机网页开始发）：进面板。
    pub(super) fn accept_receive_started(
        &mut self,
        session_id: String,
        sender: String,
        source: ReceiveSource,
        files: Vec<FileMetadata>,
    ) {
        let panel_open = self.operation_queue.is_panel_open();
        self.transfer_tasks.push(
            TransferKind::Receive(ReceiveTask::new(session_id, sender, source, files)),
            panel_open,
        );
    }

    pub(super) fn accept_receive_progress(
        &mut self,
        session_id: &str,
        file_id: String,
        received: u64,
    ) {
        if let Some(TransferKind::Receive(receive)) = self
            .transfer_tasks
            .receive_mut(session_id)
            .map(|task| &mut task.kind)
        {
            receive.record_progress(file_id, received);
        }
    }

    /// 会话结束（收完、对方取消、本机取消）：任务面板、传输记录、后台时的桌面通知。
    pub(super) fn finish_receive(
        &mut self,
        session_id: &str,
        report: &ReceiveReport,
    ) -> Task<Message> {
        let history = self.accept_receive_finished(session_id, report);
        let notification =
            self.notify_transfer_receive_finished(report.saved_paths(), report.failures());
        Task::batch([history, notification])
    }

    /// 取消了的显示"已取消"，其余显示结果；每个文件记一条传输记录。
    pub(super) fn accept_receive_finished(
        &mut self,
        session_id: &str,
        report: &ReceiveReport,
    ) -> Task<Message> {
        let panel_open = self.operation_queue.is_panel_open();
        let Some(task) = self.transfer_tasks.receive_mut(session_id) else {
            return Task::none();
        };
        let id = task.id;
        let TransferKind::Receive(receive) = &mut task.kind else {
            return Task::none();
        };
        let saved = report.saved_paths().len();
        receive.phase = if report.canceled() || receive.phase == ReceivePhase::Canceling {
            ReceivePhase::Canceled { saved }
        } else {
            ReceivePhase::Finished {
                saved,
                failed: report.failures(),
            }
        };
        let records = NewRecord::per_received_file(&receive.sender, report);
        self.transfer_tasks.mark_changed(id, panel_open);
        self.record_history(records)
    }

    /// 面板"取消"、设备行"取消"的唯一入口。
    pub(super) fn cancel_transfer_task(&mut self, id: u64) -> Task<Message> {
        let panel_open = self.operation_queue.is_panel_open();
        match self.transfer_tasks.get_mut(id).map(|task| &mut task.kind) {
            Some(TransferKind::Send(_)) => {
                let Some(history) = self.cancel_send(id) else {
                    return Task::none();
                };
                self.transfer_tasks.mark_changed(id, panel_open);
                history
            }
            Some(TransferKind::Receive(receive)) if receive.phase == ReceivePhase::Receiving => {
                // 结果（含记录）等服务结束会话、发 `ReceiveFinished` 再说。
                receive.phase = ReceivePhase::Canceling;
                match &receive.source {
                    ReceiveSource::LocalSend => {
                        if let Some(runtime) = &self.transfer_runtime {
                            runtime.handle.cancel_receive(&receive.session_id);
                        }
                    }
                    ReceiveSource::Link(control) => control.cancel_session(&receive.session_id),
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// 发送方撤回了还在等确认的请求：收起确认弹窗。
    pub(super) fn accept_request_withdrawn(&mut self, session_id: &str) -> Task<Message> {
        if self
            .incoming_transfer
            .as_ref()
            .is_some_and(|incoming| incoming.session_id == session_id)
        {
            self.incoming_transfer = None;
        }
        Task::none()
    }
}
