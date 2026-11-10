//! LocalSend 直推：每台设备一份最近一次直推的状态。
//!
//! "发送到手机"窗口与 LocalSend 窗口的设备行都读这里，所以状态按设备（证书
//! 指纹）记，不挂在某个窗口的会话上。同一台设备同时只有一次直推：进度与结果
//! 按指纹找到的就是这一次，不需要会话号。对方要 PIN 时这一次停在 `NeedsPin`，
//! 输完用同一批文件重发（同官方：先不带 PIN，401 才问）。

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
    pub(crate) phase: DirectSendPhase,
    /// 发给谁、发哪些：输了 PIN 用同一批重发，不读当时的待发列表。
    target: DeviceInfo,
    paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendPhase {
    Sending,
    /// 对方要 PIN：设备行里展开输入框。`wrong` = 上一次输的不对。
    NeedsPin {
        input: String,
        wrong: bool,
    },
    Sent,
    Canceled,
    Failed(DirectSendFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendFailure {
    Declined,
    TooManyPinAttempts,
    Other(String),
}

/// 直推结束时的结果：`LocalSendError` 不能 Clone，在命令边界映射成它进消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendOutcome {
    Sent,
    PinRequired,
    WrongPin,
    Failed(DirectSendFailure),
}

impl DirectSendState {
    /// 测试里没有常驻服务、发不出去：直接造一份"正在发"的状态。
    #[cfg(test)]
    pub(crate) fn sending_to(target: DeviceInfo, paths: Vec<PathBuf>) -> Self {
        Self {
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            phase: DirectSendPhase::Sending,
            target,
            paths,
        }
    }

    /// 用户能不能对这台设备点主"发送"：在发或等 PIN 时都不能。
    fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            DirectSendPhase::Sending | DirectSendPhase::NeedsPin { .. }
        )
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
            .is_some_and(DirectSendState::is_busy)
    }

    /// 两个窗口的"发送"都走这里：没有文件或这台设备正忙时不做。
    pub(super) fn begin_direct_send(
        &mut self,
        device: DeviceInfo,
        paths: Vec<PathBuf>,
    ) -> iced::Task<Message> {
        if paths.is_empty() || !self.can_send_to(&device) {
            return iced::Task::none();
        }
        self.start_direct_send(device, paths, None)
    }

    fn start_direct_send(
        &mut self,
        device: DeviceInfo,
        paths: Vec<PathBuf>,
        pin: Option<String>,
    ) -> iced::Task<Message> {
        // 直推要出示的设备身份归常驻服务；设备列表本身也只在服务运行时存在。
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return iced::Task::none();
        };
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<SendProgress>(16);
        let send = runtime
            .handle
            .send_to_device(&device, paths.clone(), pin, progress_tx);
        let command = direct_send_command(
            device.fingerprint.clone(),
            device.alias.clone(),
            send,
            progress_rx,
        );
        self.direct_sends.insert(
            device.fingerprint.clone(),
            DirectSendState {
                file_index: 0,
                total_files: 0,
                bytes_sent: 0,
                file_size: 0,
                phase: DirectSendPhase::Sending,
                target: device,
                paths,
            },
        );
        command
    }

    pub(super) fn edit_direct_send_pin(&mut self, fingerprint: &str, value: String) {
        if let Some(DirectSendPhase::NeedsPin { input, .. }) = self
            .direct_sends
            .get_mut(fingerprint)
            .map(|state| &mut state.phase)
        {
            *input = value;
        }
    }

    /// 带着输的 PIN 把同一批文件再发一次；空的不发。
    pub(super) fn submit_direct_send_pin(&mut self, fingerprint: &str) -> iced::Task<Message> {
        let Some(state) = self.direct_sends.get(fingerprint) else {
            return iced::Task::none();
        };
        let DirectSendPhase::NeedsPin { input, .. } = &state.phase else {
            return iced::Task::none();
        };
        if input.is_empty() {
            return iced::Task::none();
        }
        let (target, paths, pin) = (state.target.clone(), state.paths.clone(), input.clone());
        self.start_direct_send(target, paths, Some(pin))
    }

    pub(super) fn cancel_direct_send_pin(&mut self, fingerprint: &str) {
        if let Some(state) = self.direct_sends.get_mut(fingerprint) {
            if matches!(state.phase, DirectSendPhase::NeedsPin { .. }) {
                state.phase = DirectSendPhase::Canceled;
            }
        }
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
        outcome: DirectSendOutcome,
    ) {
        // 失败进日志：设备行只显示到下一次发送，日志是长期记录。要 PIN 不是失败。
        if let DirectSendOutcome::Failed(failure) = &outcome {
            record_direct_send_failure(alias, failure);
        }
        let Some(state) = self.direct_sends.get_mut(fingerprint) else {
            return;
        };
        state.phase = match outcome {
            DirectSendOutcome::Sent => DirectSendPhase::Sent,
            DirectSendOutcome::PinRequired => DirectSendPhase::NeedsPin {
                input: String::new(),
                wrong: false,
            },
            DirectSendOutcome::WrongPin => DirectSendPhase::NeedsPin {
                input: String::new(),
                wrong: true,
            },
            DirectSendOutcome::Failed(failure) => DirectSendPhase::Failed(failure),
        };
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
) -> DirectSendOutcome {
    match result {
        Ok(Ok(())) => DirectSendOutcome::Sent,
        Ok(Err(LocalSendError::PinRequired)) => DirectSendOutcome::PinRequired,
        Ok(Err(LocalSendError::WrongPin)) => DirectSendOutcome::WrongPin,
        Ok(Err(LocalSendError::Declined)) => DirectSendOutcome::Failed(DirectSendFailure::Declined),
        Ok(Err(LocalSendError::TooManyPinAttempts)) => {
            DirectSendOutcome::Failed(DirectSendFailure::TooManyPinAttempts)
        }
        Ok(Err(error)) => DirectSendOutcome::Failed(DirectSendFailure::Other(error.to_string())),
        Err(error) => DirectSendOutcome::Failed(DirectSendFailure::Other(error.to_string())),
    }
}

/// 日志写英文原因（Logs 页不走翻译）；界面文案在设备行里按 `DirectSendFailure` 生成。
fn record_direct_send_failure(alias: &str, failure: &DirectSendFailure) {
    let reason = match failure {
        DirectSendFailure::Declined => "the device declined the files",
        DirectSendFailure::TooManyPinAttempts => "too many wrong PINs",
        DirectSendFailure::Other(reason) => reason,
    };
    tracing::error!(
        target: "app_ui::transfer",
        event = "localsend_direct_send_failed",
        error = %sanitized_application_log_detail(reason),
        "LocalSend direct send to {} failed",
        sanitized_application_log_detail(alias)
    );
}
