//! LocalSend 直推：每台设备一份最近一次直推的状态。
//!
//! "发送到手机"窗口与 LocalSend 窗口的设备行都读这里，所以状态按设备（证书
//! 指纹）记，不挂在某个窗口的会话上。同一台设备同时只有一次直推：进度与结果
//! 按指纹找到的就是这一次，不需要会话号。
//!
//! 一次发给几台时文件只准备一遍（展开文件夹、算 sha256）：准备中的一批记在
//! `batches`，好了再分给等它的每台设备。对方要 PIN 时停在 `NeedsPin`，输完用
//! 同一份准备结果重发（同官方：先不带 PIN，401 才问）。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use iced::futures::{SinkExt, StreamExt};
use local_send::{
    DeviceInfo, FileFailure, LocalSendError, PrepareProgress, PreparedFiles, SendProgress,
    SendReport,
};

use super::super::FileBrowser;
use super::TransferMessage;
use crate::model::{sanitized_application_log_detail, Message};

/// 全部直推：每台设备（证书指纹）的状态，与还在准备的批次。
#[derive(Debug, Default)]
pub(crate) struct DirectSends {
    pub(crate) states: HashMap<String, DirectSendState>,
    /// 准备中的批次 → 进度；准备完就移走。
    batches: HashMap<u64, PrepareProgress>,
    next_batch: u64,
}

/// 某台设备最近一次直推；在本次运行里保留，下一次发给同一台时替换。
#[derive(Debug, Clone)]
pub(crate) struct DirectSendState {
    pub(crate) file_index: usize,
    pub(crate) total_files: usize,
    pub(crate) bytes_sent: u64,
    pub(crate) file_size: u64,
    pub(crate) phase: DirectSendPhase,
    target: DeviceInfo,
    files: SendFiles,
}

/// 这台设备要发的文件：还在准备（哪一批），或已准备好（几台共用）。
#[derive(Debug, Clone)]
enum SendFiles {
    Preparing(u64),
    Ready(Arc<PreparedFiles>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendPhase {
    /// 展开文件夹、算校验。
    Checking,
    Sending,
    /// 对方要 PIN：设备行里展开输入框。`wrong` = 上一次输的不对。
    NeedsPin {
        input: String,
        wrong: bool,
    },
    /// 发完了；`failed` 是没发成的文件（含准备时就读不了的），点"详情"展开。
    Finished {
        sent: usize,
        failed: Vec<FileFailure>,
        details_open: bool,
    },
    Canceled,
    Failed(DirectSendFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendFailure {
    Declined,
    TooManyPinAttempts,
    NothingToSend,
    Other(String),
}

/// 直推结束时的结果：`LocalSendError` 不能 Clone，在命令边界映射成它进消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendOutcome {
    Sent(SendReport),
    PinRequired,
    WrongPin,
    Failed(DirectSendFailure),
}

impl DirectSendState {
    /// 测试里没有常驻服务、发不出去：直接造一份"正在发"的状态。
    #[cfg(test)]
    pub(crate) fn sending_to(target: DeviceInfo) -> Self {
        Self {
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            phase: DirectSendPhase::Sending,
            target,
            files: SendFiles::Ready(Arc::new(PreparedFiles::unreadable(String::new()))),
        }
    }

    /// 用户能不能对这台设备点"发送"：在准备、在发或等 PIN 时都不能。
    fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            DirectSendPhase::Checking | DirectSendPhase::Sending | DirectSendPhase::NeedsPin { .. }
        )
    }
}

impl FileBrowser {
    pub(crate) fn direct_send_to(&self, device: &DeviceInfo) -> Option<&DirectSendState> {
        self.direct_sends.states.get(&device.fingerprint)
    }

    /// 同一台设备同时只有一次直推：设备行的按钮与发送入口都按它判断。
    pub(crate) fn can_send_to(&self, device: &DeviceInfo) -> bool {
        !self
            .direct_send_to(device)
            .is_some_and(DirectSendState::is_busy)
    }

    /// 准备进度（0–100）：这台设备在等的那一批。
    pub(crate) fn checking_percent(&self, state: &DirectSendState) -> u64 {
        let SendFiles::Preparing(batch) = state.files else {
            return 100;
        };
        self.direct_sends
            .batches
            .get(&batch)
            .filter(|progress| progress.bytes_total > 0)
            .map_or(0, |progress| {
                progress.bytes_done * 100 / progress.bytes_total
            })
    }

    /// 所有"发送"都走这里：跳过正忙的设备；几台一起发时文件只准备一遍。
    pub(super) fn begin_direct_sends(
        &mut self,
        devices: Vec<DeviceInfo>,
        paths: Vec<PathBuf>,
    ) -> iced::Task<Message> {
        let devices: Vec<_> = devices
            .into_iter()
            .filter(|device| self.can_send_to(device))
            .collect();
        // 直推要出示的设备身份归常驻服务；设备列表本身也只在服务运行时存在。
        if paths.is_empty() || devices.is_empty() || self.transfer_runtime.is_none() {
            return iced::Task::none();
        }
        let batch = self.direct_sends.next_batch;
        self.direct_sends.next_batch += 1;
        self.direct_sends.batches.insert(
            batch,
            PrepareProgress {
                bytes_done: 0,
                bytes_total: 0,
            },
        );
        for target in devices {
            self.direct_sends.states.insert(
                target.fingerprint.clone(),
                DirectSendState {
                    file_index: 0,
                    total_files: 0,
                    bytes_sent: 0,
                    file_size: 0,
                    phase: DirectSendPhase::Checking,
                    target,
                    files: SendFiles::Preparing(batch),
                },
            );
        }
        prepare_command(batch, paths)
    }

    pub(super) fn accept_batch_progress(&mut self, batch: u64, progress: PrepareProgress) {
        if let Some(current) = self.direct_sends.batches.get_mut(&batch) {
            *current = progress;
        }
    }

    /// 一批准备好了：分给还在等它的每台设备，各自开始发。
    pub(super) fn accept_batch_prepared(
        &mut self,
        batch: u64,
        files: Arc<PreparedFiles>,
    ) -> iced::Task<Message> {
        self.direct_sends.batches.remove(&batch);
        let waiting: Vec<String> = self
            .direct_sends
            .states
            .iter_mut()
            .filter(|(_, state)| matches!(state.files, SendFiles::Preparing(id) if id == batch))
            .map(|(fingerprint, state)| {
                state.files = SendFiles::Ready(Arc::clone(&files));
                fingerprint.clone()
            })
            .collect();
        let launches: Vec<_> = waiting
            .iter()
            .map(|fingerprint| self.launch_direct_send(fingerprint, None))
            .collect();
        iced::Task::batch(launches)
    }

    /// 用这台设备已准备好的文件发一次；要 PIN 时带上。
    fn launch_direct_send(
        &mut self,
        fingerprint: &str,
        pin: Option<String>,
    ) -> iced::Task<Message> {
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return iced::Task::none();
        };
        let Some(state) = self.direct_sends.states.get_mut(fingerprint) else {
            return iced::Task::none();
        };
        let SendFiles::Ready(files) = &state.files else {
            return iced::Task::none();
        };
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<SendProgress>(16);
        let send =
            runtime
                .handle
                .send_to_device(&state.target, Arc::clone(files), pin, progress_tx);
        state.phase = DirectSendPhase::Sending;
        state.file_index = 0;
        state.total_files = 0;
        state.bytes_sent = 0;
        state.file_size = 0;
        direct_send_command(
            fingerprint.to_owned(),
            state.target.alias.clone(),
            send,
            progress_rx,
        )
    }

    pub(super) fn edit_direct_send_pin(&mut self, fingerprint: &str, value: String) {
        if let Some(DirectSendPhase::NeedsPin { input, .. }) = self
            .direct_sends
            .states
            .get_mut(fingerprint)
            .map(|state| &mut state.phase)
        {
            *input = value;
        }
    }

    /// 带着输的 PIN 把同一批文件再发一次；空的不发。
    pub(super) fn submit_direct_send_pin(&mut self, fingerprint: &str) -> iced::Task<Message> {
        let pin = match self
            .direct_sends
            .states
            .get(fingerprint)
            .map(|state| &state.phase)
        {
            Some(DirectSendPhase::NeedsPin { input, .. }) if !input.is_empty() => input.clone(),
            _ => return iced::Task::none(),
        };
        self.launch_direct_send(fingerprint, Some(pin))
    }

    pub(super) fn cancel_direct_send_pin(&mut self, fingerprint: &str) {
        if let Some(state) = self.direct_sends.states.get_mut(fingerprint) {
            if matches!(state.phase, DirectSendPhase::NeedsPin { .. }) {
                state.phase = DirectSendPhase::Canceled;
            }
        }
    }

    pub(super) fn toggle_direct_send_details(&mut self, fingerprint: &str) {
        if let Some(DirectSendPhase::Finished { details_open, .. }) = self
            .direct_sends
            .states
            .get_mut(fingerprint)
            .map(|state| &mut state.phase)
        {
            *details_open = !*details_open;
        }
    }

    pub(super) fn accept_direct_send_progress(
        &mut self,
        fingerprint: &str,
        progress: SendProgress,
    ) {
        let Some(state) = self.direct_sends.states.get_mut(fingerprint) else {
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
        if let Some(reason) = logged_failure(&outcome) {
            record_direct_send_failure(alias, &reason);
        }
        let Some(state) = self.direct_sends.states.get_mut(fingerprint) else {
            return;
        };
        state.phase = match outcome {
            DirectSendOutcome::Sent(report) => DirectSendPhase::Finished {
                sent: report.sent,
                failed: report.failed,
                details_open: false,
            },
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

/// 准备一批文件：进度与结果进同一事件流。
fn prepare_command(batch: u64, paths: Vec<PathBuf>) -> iced::Task<Message> {
    iced::Task::stream(
        iced::stream::channel(16, async move |mut output| {
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
            let prepare = local_send::prepare_files(paths, progress_tx);
            tokio::pin!(prepare);
            let files = loop {
                tokio::select! {
                    biased;
                    Some(progress) = progress_rx.recv() => {
                        let _ = output
                            .send(TransferMessage::BatchProgress { batch, progress })
                            .await;
                    }
                    files = &mut prepare => break files,
                }
            };
            let _ = output
                .send(TransferMessage::BatchPrepared {
                    batch,
                    files: Arc::new(files),
                })
                .await;
        })
        .map(Message::Transfer),
    )
}

/// LocalSend 直推：进度经 mpsc 进入同一事件流，终态单独回执。
fn direct_send_command(
    fingerprint: String,
    alias: String,
    send: impl Future<Output = Result<SendReport, LocalSendError>> + Send + 'static,
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
    result: Result<Result<SendReport, LocalSendError>, tokio::task::JoinError>,
) -> DirectSendOutcome {
    let failure = match result {
        Ok(Ok(report)) => return DirectSendOutcome::Sent(report),
        Ok(Err(LocalSendError::PinRequired)) => return DirectSendOutcome::PinRequired,
        Ok(Err(LocalSendError::WrongPin)) => return DirectSendOutcome::WrongPin,
        Ok(Err(LocalSendError::Declined)) => DirectSendFailure::Declined,
        Ok(Err(LocalSendError::TooManyPinAttempts)) => DirectSendFailure::TooManyPinAttempts,
        Ok(Err(LocalSendError::NothingToSend)) => DirectSendFailure::NothingToSend,
        Ok(Err(error)) => DirectSendFailure::Other(error.to_string()),
        Err(error) => DirectSendFailure::Other(error.to_string()),
    };
    DirectSendOutcome::Failed(failure)
}

/// 要进日志的原因（英文，Logs 页不走翻译）；没失败返回 None。
fn logged_failure(outcome: &DirectSendOutcome) -> Option<String> {
    match outcome {
        DirectSendOutcome::Sent(report) if report.failed.is_empty() => None,
        DirectSendOutcome::Sent(report) => Some(
            report
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.file_name, failure.reason))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        DirectSendOutcome::PinRequired | DirectSendOutcome::WrongPin => None,
        DirectSendOutcome::Failed(DirectSendFailure::Declined) => {
            Some("the device declined the files".to_owned())
        }
        DirectSendOutcome::Failed(DirectSendFailure::TooManyPinAttempts) => {
            Some("too many wrong PINs".to_owned())
        }
        DirectSendOutcome::Failed(DirectSendFailure::NothingToSend) => {
            Some("nothing to send".to_owned())
        }
        DirectSendOutcome::Failed(DirectSendFailure::Other(reason)) => Some(reason.clone()),
    }
}

fn record_direct_send_failure(alias: &str, reason: &str) {
    tracing::error!(
        target: "app_ui::transfer",
        event = "localsend_direct_send_failed",
        error = %sanitized_application_log_detail(reason),
        "LocalSend direct send to {} failed",
        sanitized_application_log_detail(alias)
    );
}

#[cfg(test)]
mod tests;
