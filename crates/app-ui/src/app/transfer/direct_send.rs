//! LocalSend 直推：每次发给一台设备是一个任务（`tasks.rs`），进任务面板；设备行显示
//! 这台设备最新的那次。同一台设备同时只有一次直推（`can_send_to`）。
//!
//! 一次发给几台时文件只准备一遍（展开文件夹、算 sha256）：准备中的一批记在
//! `batches`，好了再分给等它的每台设备。文字不用准备。对方要 PIN 时停在
//! `NeedsPin`，输完用同一份东西重发（同官方：先不带 PIN，401 才问）。
//!
//! 取消（面板与设备行同一个入口 `cancel_transfer_task`）：校验中 = 这台不发了（等
//! 这批的都不发了才停下准备）；发送中 = 撤回请求或断掉并通知对方，结果回来是
//! `Canceled`。

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use iced::futures::{SinkExt, StreamExt};
use local_send::{
    CancellationToken, DeviceInfo, FileFailure, LocalSendError, PrepareProgress, PreparedFiles,
    SendProgress, SendReport,
};

use super::super::FileBrowser;
use super::tasks::{Batch, SpeedMeter, TransferKind};
use super::TransferMessage;
use crate::model::{sanitized_application_log_detail, Message};

/// 一次发给一台设备。
#[derive(Debug)]
pub(crate) struct DirectSendState {
    pub(crate) target: DeviceInfo,
    /// 面板标题用：第一个条目的名字与一共几个条目（用户选的，文件夹算一个）。
    pub(crate) first_item: String,
    pub(crate) item_count: usize,
    pub(crate) phase: DirectSendPhase,
    pub(crate) file_index: usize,
    pub(crate) total_files: usize,
    pub(crate) bytes_sent: u64,
    pub(crate) file_size: u64,
    /// 整批：已传 / 总量（发送端第一次报进度前都是 0）。
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: u64,
    pub(crate) speed: SpeedMeter,
    payload: Payload,
    cancel: CancellationToken,
}

/// 这台设备要发的东西：文件还在准备（哪一批）或已准备好（几台共用），或一段文字。
#[derive(Debug, Clone)]
enum Payload {
    Preparing(u64),
    Files(Arc<PreparedFiles>),
    Text(String),
}

/// 用户要发出去的：一批文件（路径），或一段文字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outgoing {
    Files(Vec<PathBuf>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendPhase {
    /// 展开文件夹、算校验。
    Checking,
    /// 等对方接受或正在传（看 `bytes_total`）。
    Sending,
    /// 对方要 PIN：设备行里展开输入框。`wrong` = 上一次输的不对。
    NeedsPin {
        input: String,
        wrong: bool,
    },
    /// 点了取消，等发送停下。
    Canceling,
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
    Canceled,
    Failed(DirectSendFailure),
}

impl DirectSendState {
    fn new(target: DeviceInfo, outgoing: &Outgoing, payload: Payload) -> Self {
        let (first_item, item_count) = match outgoing {
            Outgoing::Files(paths) => (
                paths
                    .first()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                paths.len(),
            ),
            Outgoing::Text(_) => (String::new(), 1),
        };
        Self {
            target,
            first_item,
            item_count,
            phase: DirectSendPhase::Checking,
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            bytes_done: 0,
            bytes_total: 0,
            speed: SpeedMeter::default(),
            payload,
            cancel: CancellationToken::new(),
        }
    }

    /// 测试里没有常驻服务、发不出去：直接造一份"正在发"的文件发送。
    #[cfg(test)]
    pub(crate) fn sending_to(target: DeviceInfo) -> Self {
        let outgoing = Outgoing::Files(vec![PathBuf::from("/tmp/one.txt")]);
        let files = Arc::new(PreparedFiles::unreadable(String::new()));
        Self {
            phase: DirectSendPhase::Sending,
            ..Self::new(target, &outgoing, Payload::Files(files))
        }
    }

    /// 测试里造一份"在校验"的发送：等第 `batch` 批。
    #[cfg(test)]
    pub(crate) fn checking_for(target: DeviceInfo, batch: u64) -> Self {
        let outgoing = Outgoing::Files(vec![PathBuf::from("/tmp/one.txt")]);
        Self::new(target, &outgoing, Payload::Preparing(batch))
    }

    /// 测试里造一份"在发文字"的发送。
    #[cfg(test)]
    pub(crate) fn sending_text_to(target: DeviceInfo) -> Self {
        let outgoing = Outgoing::Text("hi".to_owned());
        Self {
            phase: DirectSendPhase::Sending,
            ..Self::new(target, &outgoing, Payload::Text("hi".to_owned()))
        }
    }

    /// 用户能不能对这台设备点"发送"：在准备、在发、在取消或等 PIN 时都不能。
    pub(super) fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            DirectSendPhase::Checking
                | DirectSendPhase::Sending
                | DirectSendPhase::Canceling
                | DirectSendPhase::NeedsPin { .. }
        )
    }

    pub(super) fn is_text(&self) -> bool {
        matches!(self.payload, Payload::Text(_))
    }

    pub(super) fn batch(&self) -> Option<u64> {
        match self.payload {
            Payload::Preparing(batch) => Some(batch),
            _ => None,
        }
    }
}

impl FileBrowser {
    pub(crate) fn direct_send_to(&self, device: &DeviceInfo) -> Option<&DirectSendState> {
        self.transfer_tasks
            .latest_send_to(&device.fingerprint)
            .map(|(_, send)| send)
    }

    /// 同一台设备同时只有一次直推：设备行的按钮与发送入口都按它判断。
    pub(crate) fn can_send_to(&self, device: &DeviceInfo) -> bool {
        !self
            .direct_send_to(device)
            .is_some_and(DirectSendState::is_busy)
    }

    /// 准备进度（0–100）：这次发送在等的那一批。
    pub(crate) fn checking_percent(&self, state: &DirectSendState) -> u64 {
        state
            .batch()
            .and_then(|batch| self.transfer_tasks.batches.get(&batch))
            .filter(|batch| batch.progress.bytes_total > 0)
            .map_or(0, |batch| {
                batch.progress.bytes_done * 100 / batch.progress.bytes_total
            })
    }

    /// 所有"发送"都走这里：跳过正忙的设备；几台一起发时文件只准备一遍。没东西可发
    /// （没有文件、文字是空白）时不做。
    pub(super) fn begin_direct_sends(
        &mut self,
        devices: Vec<DeviceInfo>,
        outgoing: Outgoing,
    ) -> iced::Task<Message> {
        let devices: Vec<_> = devices
            .into_iter()
            .filter(|device| self.can_send_to(device))
            .collect();
        // 直推要出示的设备身份归常驻服务；设备列表本身也只在服务运行时存在。
        if devices.is_empty() || self.transfer_runtime.is_none() {
            return iced::Task::none();
        }
        let (payload, command) = match &outgoing {
            Outgoing::Files(paths) if paths.is_empty() => return iced::Task::none(),
            Outgoing::Text(text) if text.trim().is_empty() => return iced::Task::none(),
            Outgoing::Files(paths) => {
                let batch = self.transfer_tasks.next_batch();
                let cancel = CancellationToken::new();
                self.transfer_tasks.batches.insert(
                    batch,
                    Batch {
                        progress: PrepareProgress {
                            bytes_done: 0,
                            bytes_total: 0,
                        },
                        cancel: cancel.clone(),
                    },
                );
                let command = prepare_command(batch, paths.clone(), cancel);
                (Payload::Preparing(batch), command)
            }
            Outgoing::Text(text) => (Payload::Text(text.clone()), iced::Task::none()),
        };
        let panel_open = self.operation_queue.is_panel_open();
        let ids: Vec<u64> = devices
            .into_iter()
            .map(|target| {
                let send = DirectSendState::new(target, &outgoing, payload.clone());
                self.transfer_tasks
                    .push(TransferKind::Send(send), panel_open)
            })
            .collect();
        // 文字不用准备，直接发；文件等这一批准备好（`accept_batch_prepared`）再发。
        let launches: Vec<_> = ids
            .into_iter()
            .map(|id| self.launch_direct_send(id, None))
            .collect();
        iced::Task::batch(launches.into_iter().chain([command]))
    }

    pub(super) fn accept_batch_progress(&mut self, batch: u64, progress: PrepareProgress) {
        if let Some(current) = self.transfer_tasks.batches.get_mut(&batch) {
            current.progress = progress;
        }
    }

    /// 一批准备好了：分给还在等它的每台设备，各自开始发。None = 准备被取消了（等它的
    /// 都取消了），没有设备在等。
    pub(super) fn accept_batch_prepared(
        &mut self,
        batch: u64,
        files: Option<Arc<PreparedFiles>>,
    ) -> iced::Task<Message> {
        self.transfer_tasks.batches.remove(&batch);
        let Some(files) = files else {
            return iced::Task::none();
        };
        let waiting: Vec<u64> = self
            .transfer_tasks
            .sends_mut()
            .filter(|(_, send)| send.batch() == Some(batch) && send.is_busy())
            .map(|(id, send)| {
                send.payload = Payload::Files(Arc::clone(&files));
                id
            })
            .collect();
        let launches: Vec<_> = waiting
            .into_iter()
            .map(|id| self.launch_direct_send(id, None))
            .collect();
        iced::Task::batch(launches)
    }

    /// 把这次的东西发一次（文件还在准备时不发）；要 PIN 时带上。
    fn launch_direct_send(&mut self, id: u64, pin: Option<String>) -> iced::Task<Message> {
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return iced::Task::none();
        };
        let Some(state) = self.transfer_tasks.send_mut(id) else {
            return iced::Task::none();
        };
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<SendProgress>(16);
        let cancel = state.cancel.clone();
        let send: Pin<Box<dyn Future<Output = Result<SendReport, LocalSendError>> + Send>> =
            match &state.payload {
                Payload::Preparing(_) => return iced::Task::none(),
                Payload::Files(files) => Box::pin(runtime.handle.send_to_device(
                    &state.target,
                    Arc::clone(files),
                    pin,
                    progress_tx,
                    cancel,
                )),
                Payload::Text(text) => {
                    let send = runtime
                        .handle
                        .send_text(&state.target, text.clone(), pin, cancel);
                    // 文字没有进度：通道随 `progress_tx` 一起关掉，命令只等结果。
                    drop(progress_tx);
                    Box::pin(async move {
                        send.await.map(|()| SendReport {
                            sent: 1,
                            failed: Vec::new(),
                        })
                    })
                }
            };
        state.phase = DirectSendPhase::Sending;
        state.file_index = 0;
        state.total_files = 0;
        state.bytes_sent = 0;
        state.file_size = 0;
        state.bytes_done = 0;
        state.bytes_total = 0;
        state.speed = SpeedMeter::default();
        direct_send_command(id, state.target.alias.clone(), send, progress_rx)
    }

    #[cfg(test)]
    pub(crate) fn begin_text_for_test(&mut self, fingerprint: &str) -> u64 {
        let target = crate::app::transfer::tests::sample_device(fingerprint);
        self.transfer_tasks.push(
            TransferKind::Send(DirectSendState::sending_text_to(target)),
            false,
        )
    }

    /// 设备行上的操作（PIN、详情、取消）都对这台设备最新的那次发送。
    fn latest_send_id(&self, fingerprint: &str) -> Option<u64> {
        self.transfer_tasks
            .latest_send_to(fingerprint)
            .map(|(id, _)| id)
    }

    fn latest_send_mut(&mut self, fingerprint: &str) -> Option<&mut DirectSendState> {
        let id = self.latest_send_id(fingerprint)?;
        self.transfer_tasks.send_mut(id)
    }

    pub(super) fn edit_direct_send_pin(&mut self, fingerprint: &str, value: String) {
        if let Some(DirectSendPhase::NeedsPin { input, .. }) = self
            .latest_send_mut(fingerprint)
            .map(|state| &mut state.phase)
        {
            *input = value;
        }
    }

    /// 带着输的 PIN 把同一份东西再发一次；空的不发。
    pub(super) fn submit_direct_send_pin(&mut self, fingerprint: &str) -> iced::Task<Message> {
        let Some(id) = self.latest_send_id(fingerprint) else {
            return iced::Task::none();
        };
        let pin = match self.transfer_tasks.send_mut(id).map(|state| &state.phase) {
            Some(DirectSendPhase::NeedsPin { input, .. }) if !input.is_empty() => input.clone(),
            _ => return iced::Task::none(),
        };
        self.launch_direct_send(id, Some(pin))
    }

    pub(super) fn toggle_direct_send_details(&mut self, fingerprint: &str) {
        if let Some(DirectSendPhase::Finished { details_open, .. }) = self
            .latest_send_mut(fingerprint)
            .map(|state| &mut state.phase)
        {
            *details_open = !*details_open;
        }
    }

    /// 设备行的"取消"与 PIN 输入框的"取消"：与面板同一个入口。
    pub(super) fn cancel_latest_send(&mut self, fingerprint: &str) {
        if let Some(id) = self.latest_send_id(fingerprint) {
            self.cancel_transfer_task(id);
        }
    }

    /// 取消一次发送；返回是否已经结束（不用等发送停下）。
    pub(super) fn cancel_send(&mut self, id: u64) -> bool {
        let Some(state) = self.transfer_tasks.send_mut(id) else {
            return false;
        };
        match state.phase {
            // 还在校验：这台不发了；等这批的都不发了才停下准备。
            DirectSendPhase::Checking => {
                state.phase = DirectSendPhase::Canceled;
                let batch = state.batch();
                let still_waiting = self
                    .transfer_tasks
                    .sends_mut()
                    .any(|(_, send)| send.batch() == batch && send.is_busy());
                if let (Some(batch), false) = (batch, still_waiting) {
                    if let Some(batch) = self.transfer_tasks.batches.get(&batch) {
                        batch.cancel.cancel();
                    }
                }
                true
            }
            DirectSendPhase::NeedsPin { .. } => {
                state.phase = DirectSendPhase::Canceled;
                true
            }
            DirectSendPhase::Sending => {
                state.phase = DirectSendPhase::Canceling;
                state.cancel.cancel();
                false
            }
            _ => false,
        }
    }

    pub(super) fn accept_direct_send_progress(&mut self, id: u64, progress: SendProgress) {
        let Some(state) = self.transfer_tasks.send_mut(id) else {
            return;
        };
        state.file_index = progress.file_index;
        state.total_files = progress.total_files;
        state.bytes_sent = progress.bytes_sent;
        state.file_size = progress.file_size;
        state.bytes_done = progress.bytes_done;
        state.bytes_total = progress.bytes_total;
        state.speed.sample(progress.bytes_done, Instant::now());
    }

    pub(super) fn accept_direct_send_finished(
        &mut self,
        id: u64,
        alias: &str,
        outcome: DirectSendOutcome,
    ) {
        // 失败进日志：设备行只显示到下一次发送，日志是长期记录。要 PIN、取消不是失败。
        if let Some(reason) = logged_failure(&outcome) {
            record_direct_send_failure(alias, &reason);
        }
        let panel_open = self.operation_queue.is_panel_open();
        let Some(state) = self.transfer_tasks.send_mut(id) else {
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
            DirectSendOutcome::Canceled => DirectSendPhase::Canceled,
            DirectSendOutcome::Failed(failure) => DirectSendPhase::Failed(failure),
        };
        self.transfer_tasks.mark_changed(id, panel_open);
    }
}

/// 准备一批文件：进度与结果进同一事件流。
fn prepare_command(
    batch: u64,
    paths: Vec<PathBuf>,
    cancel: CancellationToken,
) -> iced::Task<Message> {
    iced::Task::stream(
        iced::stream::channel(16, async move |mut output| {
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
            let prepare = local_send::prepare_files(paths, progress_tx, cancel);
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
                    files: files.map(Arc::new),
                })
                .await;
        })
        .map(Message::Transfer),
    )
}

/// LocalSend 直推：进度经 mpsc 进入同一事件流，终态单独回执。
fn direct_send_command(
    task: u64,
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
                                .send(TransferMessage::DirectSendProgress { task, progress })
                                .await;
                        }
                        // 发送任务结束关闭进度通道：收最终回执。
                        None => break,
                    },
                    result = &mut send_task => {
                        let _ = output
                            .send(TransferMessage::DirectSendFinished {
                                task,
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
                    task,
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
        Ok(Err(LocalSendError::Canceled)) => return DirectSendOutcome::Canceled,
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
        DirectSendOutcome::PinRequired
        | DirectSendOutcome::WrongPin
        | DirectSendOutcome::Canceled => None,
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
