//! 接收任务：接受了的一次接收进任务面板，按文件累计进度，可以在这台电脑上取消。
//! 还有面板与设备行共用的取消、清除入口。

use iced::Task;
use local_send::{DeviceInfo, FileMetadata};

use super::super::FileBrowser;
use super::tasks::{ReceivePhase, ReceiveTask, TransferKind};
use crate::model::Message;

impl FileBrowser {
    /// 服务收下了一次接收（自动收下或用户确认）：进面板。
    pub(super) fn accept_receive_started(
        &mut self,
        session_id: String,
        device: &DeviceInfo,
        files: Vec<FileMetadata>,
    ) {
        let sender = self.transfer_device_name(device);
        let panel_open = self.operation_queue.is_panel_open();
        self.transfer_tasks.push(
            TransferKind::Receive(ReceiveTask::new(session_id, sender, files)),
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

    /// 会话结束（收完、对方取消、本机取消）：本机取消的显示"已取消"，其余显示结果。
    pub(super) fn accept_receive_finished(
        &mut self,
        session_id: &str,
        saved: usize,
        failed: Vec<(String, String)>,
    ) {
        let panel_open = self.operation_queue.is_panel_open();
        let Some(task) = self.transfer_tasks.receive_mut(session_id) else {
            return;
        };
        let id = task.id;
        if let TransferKind::Receive(receive) = &mut task.kind {
            receive.phase = match receive.phase {
                ReceivePhase::Canceling => ReceivePhase::Canceled { saved },
                _ => ReceivePhase::Finished { saved, failed },
            };
        }
        self.transfer_tasks.mark_changed(id, panel_open);
    }

    /// 面板"取消"、设备行"取消"的唯一入口。
    pub(super) fn cancel_transfer_task(&mut self, id: u64) {
        let panel_open = self.operation_queue.is_panel_open();
        let ended_now = match self.transfer_tasks.get_mut(id).map(|task| &mut task.kind) {
            Some(TransferKind::Send(_)) => self.cancel_send(id),
            Some(TransferKind::Receive(receive)) if receive.phase == ReceivePhase::Receiving => {
                receive.phase = ReceivePhase::Canceling;
                if let Some(runtime) = &self.transfer_runtime {
                    runtime.handle.cancel_receive(&receive.session_id);
                }
                false
            }
            _ => false,
        };
        if ended_now {
            self.transfer_tasks.mark_changed(id, panel_open);
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
