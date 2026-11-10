//! LocalSend 直推：每台设备一份最近一次直推的状态。
//!
//! "发送到手机"窗口与 LocalSend 窗口的设备行都读这里，所以状态按设备（证书
//! 指纹）记，不挂在某个窗口的会话上。同一台设备同时只有一次直推：进度与结果
//! 按指纹找到的就是这一次，不需要会话号。

use std::future::Future;
use std::path::PathBuf;

use iced::futures::{SinkExt, StreamExt};
use local_send::{DeviceInfo, LocalSendError, SendProgress};

use super::super::FileBrowser;
use super::TransferMessage;
use crate::model::{sanitized_application_log_detail, Message};

/// 某台设备最近一次直推；在本次运行里保留，下一次发给同一台时替换。
#[derive(Debug, Clone)]
pub(crate) struct DirectSendState {
    pub(crate) file_index: usize,
    pub(crate) total_files: usize,
    pub(crate) bytes_sent: u64,
    pub(crate) file_size: u64,
    /// 终态：Ok 为全部完成，Err 为失败原因（保留最后一次进度展示）。
    pub(crate) outcome: Option<Result<(), String>>,
}

impl DirectSendState {
    pub(super) fn started() -> Self {
        Self {
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            outcome: None,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.outcome.is_none()
    }
}

impl FileBrowser {
    pub(crate) fn direct_send_to(&self, device: &DeviceInfo) -> Option<&DirectSendState> {
        self.direct_sends.get(&device.fingerprint)
    }

    /// 同一台设备同时只有一次直推：设备行的按钮与发送入口都按它判断。
    pub(crate) fn can_send_to(&self, device: &DeviceInfo) -> bool {
        !self
            .direct_send_to(device)
            .is_some_and(DirectSendState::is_running)
    }

    /// 两个窗口的"发送"都走这里：没有文件或这台设备正在发时不做。
    pub(super) fn begin_direct_send(
        &mut self,
        device: DeviceInfo,
        paths: Vec<PathBuf>,
    ) -> iced::Task<Message> {
        if paths.is_empty() || !self.can_send_to(&device) {
            return iced::Task::none();
        }
        // 直推要出示的设备身份归常驻服务；设备列表本身也只在服务运行时存在。
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return iced::Task::none();
        };
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<SendProgress>(16);
        let send = runtime.handle.send_to_device(&device, paths, progress_tx);
        self.direct_sends
            .insert(device.fingerprint.clone(), DirectSendState::started());
        direct_send_command(device.fingerprint, device.alias, send, progress_rx)
    }

    pub(super) fn accept_direct_send_progress(
        &mut self,
        fingerprint: &str,
        progress: SendProgress,
    ) {
        let Some(state) = self.direct_sends.get_mut(fingerprint) else {
            return;
        };
        state.file_index = progress.file_index;
        state.total_files = progress.total_files;
        state.bytes_sent = progress.bytes_sent;
        state.file_size = progress.file_size;
    }

    pub(super) fn accept_direct_send_finished(
        &mut self,
        fingerprint: &str,
        alias: &str,
        outcome: Result<(), String>,
    ) {
        // 失败进日志：设备行只显示到下一次发送，日志是长期记录。
        if let Err(error) = &outcome {
            record_direct_send_failure(alias, error);
        }
        if let Some(state) = self.direct_sends.get_mut(fingerprint) {
            state.outcome = Some(outcome);
        }
    }
}

/// LocalSend 直推：进度经 mpsc 进入同一事件流，终态单独回执。
fn direct_send_command(
    fingerprint: String,
    alias: String,
    send: impl Future<Output = Result<(), LocalSendError>> + Send + 'static,
    mut progress_rx: tokio::sync::mpsc::Receiver<SendProgress>,
) -> iced::Task<Message> {
    iced::Task::stream(
        iced::stream::channel(16, async move |mut output| {
            // 进度通道满时服务内部丢进度，不会阻塞传输本身。
            let mut send_task = tokio::spawn(send);
            loop {
                tokio::select! {
                    biased;
                    progress = progress_rx.recv() => match progress {
                        Some(progress) => {
                            let _ = output
                                .send(TransferMessage::DirectSendProgress {
                                    fingerprint: fingerprint.clone(),
                                    progress,
                                })
                                .await;
                        }
                        // 发送任务结束关闭进度通道：收最终回执。
                        None => break,
                    },
                    result = &mut send_task => {
                        let _ = output
                            .send(TransferMessage::DirectSendFinished {
                                fingerprint,
                                alias,
                                outcome: send_outcome(result),
                            })
                            .await;
                        return;
                    }
                }
            }
            let outcome = send_outcome(send_task.await);
            let _ = output
                .send(TransferMessage::DirectSendFinished {
                    fingerprint,
                    alias,
                    outcome,
                })
                .await;
        })
        .map(Message::Transfer),
    )
}

fn send_outcome(
    result: Result<Result<(), LocalSendError>, tokio::task::JoinError>,
) -> Result<(), String> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

fn record_direct_send_failure(alias: &str, error: &str) {
    tracing::error!(
        target: "app_ui::transfer",
        event = "localsend_direct_send_failed",
        error = %sanitized_application_log_detail(error),
        "LocalSend direct send to {} failed",
        sanitized_application_log_detail(alias)
    );
}
