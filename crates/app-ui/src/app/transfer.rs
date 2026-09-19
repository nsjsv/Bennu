//! 本地文件传输状态机：常驻 LocalSend 服务、二维码下载会话、LocalSend
//! 直推与接收确认 Modal。
//!
//! 职责边界（design.md 阶段 B）：
//! - 常驻服务在 update 上下文同步启动（iced tokio executor 保证 tokio 上下文，
//!   见 design.md A0 结论）；`ServiceHandle` 常驻不主动关停，设置变更时重启。
//! - 二维码下载服务是一次性会话：服务句柄随事件转发流存活，窗口关闭通过
//!   `CancellationToken` 收尾；完成/超时由服务自停。
//! - 信任判定回调读共享指纹快照；持久化仍归 UserPreferences。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iced::futures::{SinkExt, StreamExt};
use local_send::{
    DeviceInfo, FileMetadata, LocalSendError, LocalSendService, QrDownloadServer, QrEvent, QrItem,
    QrSource, SendProgress, ServiceEvent,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::transfer_service::service_config;
use super::FileBrowser;
use crate::config::{default_transfer_device_alias, TrustedTransferDevice};
use crate::model::Message;
use crate::view::transfer_qr::qr_image_handle;

/// 传输子消息：`Message::Transfer` 的载荷。
#[derive(Debug, Clone)]
pub(crate) enum TransferMessage {
    // ---- 发送：二维码下载会话（R2-R4） ----
    /// 右键「发送到手机」：以当前选中集合开启一次性下载会话。
    SendToPhoneRequested,
    /// 下载服务就绪：携带本机全部候选地址的下载链接，窗口先展示地址列表。
    QrSessionStarted {
        urls: Vec<String>,
    },
    /// 用户点选某个地址：生成该地址对应的二维码。
    QrAddressSelected(usize),
    /// 二维码视图返回地址列表（重新选择手机可达的地址）。
    QrAddressListRequested,
    QrSessionFailed(String),
    QrEvent(QrEvent),
    // ---- 发送：LocalSend 直推（R5） ----
    SendToDevicePressed(DeviceInfo),
    DirectSendProgress {
        fingerprint: String,
        progress: SendProgress,
    },
    DirectSendFinished {
        fingerprint: String,
        outcome: Result<(), String>,
    },
    // ---- 接收确认（R7） ----
    IncomingRememberToggled(bool),
    IncomingConfirmed {
        accept: bool,
    },
    // ---- 常驻服务事件流（R6） ----
    ServiceEvent(ServiceEvent),
    /// 事件流积压丢弃（broadcast Lagged）：整体刷新设备表。
    DevicesRefreshRequested,
    /// 设备列表手动刷新按钮：清空快照并立即触发一轮子网扫描。
    RefreshDevicesRequested,
    // ---- 设置（R9） ----
    DownloadDirChooserPressed,
    DownloadDirChosen(Result<Option<PathBuf>, String>),
    DeviceAliasInputChanged(String),
    DeviceAliasCommitted,
    TrustedDeviceRemoved(String),
}

/// 常驻 LocalSend 服务运行时状态。
pub(crate) struct LocalSendRuntime {
    pub(crate) handle: local_send::ServiceHandle,
    /// 事件订阅重挂时的 resubscribe 种子（订阅侧派生独立接收端）。
    pub(crate) events: broadcast::Receiver<ServiceEvent>,
    /// 订阅身份代：服务重启后强制重挂事件流，旧流随旧服务关闭而终结。
    pub(crate) generation: u64,
    /// 共享信任指纹快照：`is_trusted` 回调读这里，设置页/确认勾选写这里。
    pub(crate) trusted: Arc<parking_lot::RwLock<HashSet<String>>>,
    /// 发现的设备快照（fingerprint 去重，UI 列表直接渲染）。
    pub(crate) devices: Vec<DeviceInfo>,
}

/// 二维码下载会话状态（与传输窗口同生命周期）。
pub(crate) struct TransferSessionState {
    pub(crate) phase: TransferSessionPhase,
    pub(crate) connected: bool,
    pub(crate) bytes_sent: u64,
    pub(crate) speed_bytes_per_second: u64,
    /// 会话终态：服务传完自停（Finished）或空闲超时（TimedOut）。
    pub(crate) finished: Option<QrEvent>,
    last_progress: Option<(u64, Instant)>,
    /// 窗口关闭→流收尾的取消令牌。
    pub(crate) cancel: CancellationToken,
    /// 当前直推显示槽（单槽：新直推顶替旧显示，后台任务继续跑完）。
    pub(crate) direct_send: Option<DirectSendState>,
}

#[derive(Debug, Clone)]
pub(crate) enum TransferSessionPhase {
    Preparing,
    /// `selected`/`qr` 为 `None` 时显示本机地址列表；点选地址后生成对应二维码。
    Ready {
        urls: Vec<String>,
        selected: Option<usize>,
        qr: Option<iced::widget::image::Handle>,
    },
    Failed(String),
}

/// LocalSend 直推进度快照（R5：设备列表点选直推）。
#[derive(Debug, Clone)]
pub(crate) struct DirectSendState {
    pub(crate) fingerprint: String,
    pub(crate) alias: String,
    pub(crate) file_name: String,
    pub(crate) file_index: usize,
    pub(crate) total_files: usize,
    pub(crate) bytes_sent: u64,
    pub(crate) file_size: u64,
    /// 终态：Ok 为全部完成，Err 为失败原因（保留最后一次进度展示）。
    pub(crate) outcome: Option<Result<(), String>>,
}

/// 接收确认 Modal 状态（R7：未信任设备首次发送）。
#[derive(Debug, Clone)]
pub(crate) struct IncomingTransferConfirmation {
    pub(crate) device: DeviceInfo,
    pub(crate) session_id: String,
    pub(crate) files: Vec<FileMetadata>,
    pub(crate) remember_device: bool,
}

impl FileBrowser {
    /// 启动（或设置变更后重启）常驻 LocalSend 服务。
    ///
    /// update 上下文已进入 tokio runtime（A0 结论）；单测无 runtime 时跳过。
    pub(super) fn start_or_restart_local_send_service(&mut self) {
        // 单测环境没有 tokio runtime：常驻服务只在真实运行时启动。
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let generation = self
            .transfer_runtime
            .as_ref()
            .map_or(0, |runtime| runtime.generation + 1);
        if let Some(previous) = self.transfer_runtime.take() {
            previous.handle.shutdown();
        }
        // 旧服务的挂起确认随重启作废：Modal 不能指向死会话。
        self.incoming_transfer = None;
        let trusted = Arc::new(parking_lot::RwLock::new(trusted_fingerprint_set(
            &self.user_config.transfer_trusted_devices,
        )));
        match LocalSendService::start(service_config(&self.user_config, Arc::clone(&trusted))) {
            Ok((handle, events)) => {
                self.transfer_runtime = Some(LocalSendRuntime {
                    handle,
                    events,
                    generation,
                    trusted,
                    devices: Vec::new(),
                });
            }
            Err(error) => {
                // 启动失败（如无法构造 HTTP 客户端）按一次性全局错误展示。
                self.show_global_error(transfer_service_start_failed(&error));
            }
        }
    }

    /// 右键「发送到手机」入口：以选中集合开启二维码下载会话并打开窗口。
    pub(super) fn begin_qr_transfer_session(&mut self) -> iced::Task<Message> {
        let paths = self.selected_paths_for_operation();
        if paths.is_empty() {
            // 无操作路径：静默返回（error-handling 规范）。
            return iced::Task::none();
        }
        // 一次新的独立请求先终结旧会话（与 Preview 同语义）。
        if let Some(previous) = self.transfer_session.take() {
            previous.cancel.cancel();
        }
        let cancel = CancellationToken::new();
        self.transfer_session = Some(TransferSessionState {
            phase: TransferSessionPhase::Preparing,
            connected: false,
            bytes_sent: 0,
            speed_bytes_per_second: 0,
            finished: None,
            last_progress: None,
            cancel: cancel.clone(),
            direct_send: None,
        });
        let items = qr_items_from_paths(paths);
        iced::Task::batch([
            self.prepare_transfer_window_open(),
            self.ensure_transfer_window(),
            qr_transfer_session_command(items, cancel),
        ])
    }

    pub(super) fn handle_transfer_message(
        &mut self,
        message: TransferMessage,
    ) -> iced::Task<Message> {
        match message {
            TransferMessage::SendToPhoneRequested => self.begin_qr_transfer_session(),
            TransferMessage::QrSessionStarted { urls } => {
                if let Some(session) = &mut self.transfer_session {
                    // 默认展示地址列表，二维码等用户点选地址后再生成。
                    session.phase = TransferSessionPhase::Ready {
                        urls,
                        selected: None,
                        qr: None,
                    };
                }
                iced::Task::none()
            }
            TransferMessage::QrAddressSelected(index) => {
                if let Some(session) = &mut self.transfer_session {
                    if let TransferSessionPhase::Ready { urls, selected, qr } = &mut session.phase {
                        // 点选即生成：fast_qr 矩阵计算在微秒级，无需异步。
                        if let Some(url) = urls.get(index) {
                            *selected = Some(index);
                            *qr = Some(qr_image_handle(url));
                        }
                    }
                }
                iced::Task::none()
            }
            TransferMessage::QrAddressListRequested => {
                if let Some(session) = &mut self.transfer_session {
                    if let TransferSessionPhase::Ready { selected, qr, .. } = &mut session.phase {
                        *selected = None;
                        *qr = None;
                    }
                }
                iced::Task::none()
            }
            TransferMessage::QrSessionFailed(error) => {
                if let Some(session) = &mut self.transfer_session {
                    session.phase = TransferSessionPhase::Failed(error);
                }
                iced::Task::none()
            }
            TransferMessage::QrEvent(event) => {
                self.accept_qr_event(event);
                iced::Task::none()
            }
            TransferMessage::SendToDevicePressed(device) => self.begin_direct_send(device),
            TransferMessage::DirectSendProgress {
                fingerprint,
                progress,
            } => {
                self.accept_direct_send_progress(fingerprint, progress);
                iced::Task::none()
            }
            TransferMessage::DirectSendFinished {
                fingerprint,
                outcome,
            } => {
                self.accept_direct_send_finished(fingerprint, outcome);
                iced::Task::none()
            }
            TransferMessage::IncomingRememberToggled(remember) => {
                if let Some(incoming) = &mut self.incoming_transfer {
                    incoming.remember_device = remember;
                }
                iced::Task::none()
            }
            TransferMessage::IncomingConfirmed { accept } => self.confirm_incoming_transfer(accept),
            TransferMessage::ServiceEvent(event) => self.accept_local_send_service_event(event),
            TransferMessage::DevicesRefreshRequested => {
                if let Some(runtime) = self.transfer_runtime.as_mut() {
                    runtime.devices.clear();
                }
                iced::Task::none()
            }
            TransferMessage::RefreshDevicesRequested => {
                if let Some(runtime) = self.transfer_runtime.as_mut() {
                    runtime.devices.clear();
                    runtime.handle.refresh_devices();
                }
                iced::Task::none()
            }
            TransferMessage::DownloadDirChooserPressed => download_dir_chooser_command(),
            TransferMessage::DownloadDirChosen(outcome) => {
                self.accept_transfer_download_dir(outcome)
            }
            TransferMessage::DeviceAliasInputChanged(value) => {
                self.transfer_device_alias_input = value;
                iced::Task::none()
            }
            TransferMessage::DeviceAliasCommitted => self.commit_transfer_device_alias(),
            TransferMessage::TrustedDeviceRemoved(fingerprint) => {
                self.remove_trusted_transfer_device(fingerprint)
            }
        }
    }

    fn accept_qr_event(&mut self, event: QrEvent) {
        let Some(session) = self.transfer_session.as_mut() else {
            return;
        };
        match event {
            QrEvent::Connected => session.connected = true,
            QrEvent::Progress {
                bytes_sent,
                active: _,
            } => {
                // 进度事件 ~1s 节流：直接差分估算速度即可。
                let now = Instant::now();
                if let Some((last_bytes, last_at)) = session.last_progress {
                    let elapsed = now.duration_since(last_at).as_secs_f64();
                    if elapsed > 0.05 && bytes_sent >= last_bytes {
                        session.speed_bytes_per_second =
                            ((bytes_sent - last_bytes) as f64 / elapsed) as u64;
                    }
                }
                session.last_progress = Some((bytes_sent, now));
                session.bytes_sent = bytes_sent;
            }
            QrEvent::Finished | QrEvent::TimedOut => {
                // 服务已自停；窗口保留终态直到用户关闭。
                session.finished = Some(event);
            }
        }
    }

    /// 设备列表点选直推（R5）。
    fn begin_direct_send(&mut self, device: DeviceInfo) -> iced::Task<Message> {
        let paths = self.selected_paths_for_operation();
        if paths.is_empty() {
            return iced::Task::none();
        }
        let display_alias = self.effective_transfer_device_alias();
        let Some(session) = self.transfer_session.as_mut() else {
            return iced::Task::none();
        };
        session.direct_send = Some(DirectSendState {
            fingerprint: device.fingerprint.clone(),
            alias: device.alias.clone(),
            file_name: String::new(),
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            outcome: None,
        });
        direct_send_command(device, paths, display_alias)
    }

    fn accept_direct_send_progress(&mut self, fingerprint: String, progress: SendProgress) {
        let Some(session) = self.transfer_session.as_mut() else {
            return;
        };
        let Some(state) = session.direct_send.as_mut() else {
            return;
        };
        // 迟到的旧直推进度（已被更新的直推顶替）直接丢弃。
        if state.fingerprint != fingerprint || state.outcome.is_some() {
            return;
        }
        state.file_name = progress.file_name;
        state.file_index = progress.file_index;
        state.total_files = progress.total_files;
        state.bytes_sent = progress.bytes_sent;
        state.file_size = progress.file_size;
    }

    fn accept_direct_send_finished(&mut self, fingerprint: String, outcome: Result<(), String>) {
        let Some(session) = self.transfer_session.as_mut() else {
            return;
        };
        let Some(state) = session.direct_send.as_mut() else {
            return;
        };
        if state.fingerprint != fingerprint {
            return;
        }
        state.outcome = Some(outcome);
    }

    /// 接收确认回写：accept 走服务确认 + 记住设备持久化；reject 释放会话。
    fn confirm_incoming_transfer(&mut self, accept: bool) -> iced::Task<Message> {
        let Some(incoming) = self.incoming_transfer.take() else {
            return iced::Task::none();
        };
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return iced::Task::none();
        };
        if accept {
            // remember=true 时服务把设备补进内存信任集（覆盖持久化落盘前的窗口期）。
            runtime
                .handle
                .accept_session(&incoming.session_id, incoming.remember_device);
            if incoming.remember_device
                && !self
                    .user_config
                    .transfer_trusted_devices
                    .iter()
                    .any(|device| device.fingerprint == incoming.device.fingerprint)
            {
                self.user_config
                    .transfer_trusted_devices
                    .push(TrustedTransferDevice {
                        fingerprint: incoming.device.fingerprint.clone(),
                        alias: incoming.device.alias.clone(),
                        added_at: chrono::Utc::now().to_rfc3339(),
                    });
                runtime
                    .trusted
                    .write()
                    .insert(incoming.device.fingerprint.clone());
                return self.persist_user_preferences_command();
            }
            return iced::Task::none();
        }
        runtime.handle.reject_session(&incoming.session_id);
        iced::Task::none()
    }

    /// Escape/外部点击对 Modal 的清理语义：按拒绝收敛（dismiss_floating 调用）。
    pub(super) fn reject_pending_incoming_transfer(&mut self) -> iced::Task<Message> {
        let Some(incoming) = self.incoming_transfer.take() else {
            return iced::Task::none();
        };
        if let Some(runtime) = self.transfer_runtime.as_ref() {
            runtime.handle.reject_session(&incoming.session_id);
        }
        iced::Task::none()
    }

    fn accept_local_send_service_event(&mut self, event: ServiceEvent) -> iced::Task<Message> {
        match event {
            ServiceEvent::DeviceSeen(device) => {
                if let Some(runtime) = self.transfer_runtime.as_mut() {
                    if let Some(existing) = runtime
                        .devices
                        .iter_mut()
                        .find(|existing| existing.fingerprint == device.fingerprint)
                    {
                        *existing = device;
                    } else {
                        runtime.devices.push(device);
                    }
                }
            }
            ServiceEvent::DeviceLost(device) => {
                if let Some(runtime) = self.transfer_runtime.as_mut() {
                    runtime
                        .devices
                        .retain(|existing| existing.fingerprint != device.fingerprint);
                }
            }
            ServiceEvent::IncomingRequest {
                device,
                session_id,
                files,
            } => return self.begin_incoming_transfer_confirmation(device, session_id, files),
            ServiceEvent::RequestAccepted { .. } | ServiceEvent::RequestRejected { .. } => {
                // 确认动作由本 UI 发起，回执无需呈现。
            }
            ServiceEvent::ReceiveProgress { .. } => {
                // 第一期接收进度不进 UI（R8 以完成通知为准）。
            }
            ServiceEvent::ReceiveFinished {
                saved_paths,
                failed,
                ..
            } => {
                return self.notify_transfer_receive_finished(saved_paths, failed);
            }
            ServiceEvent::ServiceError(message) => {
                // 服务级错误（端口占用等）只报一次：全局 Toast（N2/design）。
                self.show_global_error(message);
            }
        }
        iced::Task::none()
    }

    /// 未信任设备的接收请求：清理主窗口同层输入状态后弹出确认 Modal。
    fn begin_incoming_transfer_confirmation(
        &mut self,
        device: DeviceInfo,
        session_id: String,
        files: Vec<FileMetadata>,
    ) -> iced::Task<Message> {
        self.context_menu = None;
        self.open_with = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        let rename_command = self.commit_rename_if_active();
        self.incoming_transfer = Some(IncomingTransferConfirmation {
            device,
            session_id,
            files,
            remember_device: false,
        });
        rename_command
    }

    /// 接收完成桌面通知：仅在应用整体后台时发布（与文件任务通知同一门控）。
    fn notify_transfer_receive_finished(
        &mut self,
        saved_paths: Vec<PathBuf>,
        failed: Vec<(String, String)>,
    ) -> iced::Task<Message> {
        // 前台不通知；空会话（无成功也无失败）同样静默。
        if self.system_focused_window.is_some() || (saved_paths.is_empty() && failed.is_empty()) {
            return iced::Task::none();
        }
        let summary = crate::localization::translate_current("Files received");
        let body =
            crate::localization::transfer_receive_finished_body(saved_paths.len(), failed.len());
        crate::commands::publish_desktop_notification_command(summary, body)
    }

    /// 设置页生效别名（配置值优先，缺省 hostname）。
    pub(crate) fn effective_transfer_device_alias(&self) -> String {
        self.user_config
            .transfer_device_alias
            .clone()
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or_else(default_transfer_device_alias)
    }

    fn accept_transfer_download_dir(
        &mut self,
        outcome: Result<Option<PathBuf>, String>,
    ) -> iced::Task<Message> {
        match outcome {
            Ok(Some(dir)) => {
                self.user_config.transfer_download_dir = Some(dir);
                // 接收目录在服务启动时确定：改目录必须重启常驻服务。
                self.start_or_restart_local_send_service();
                self.persist_user_preferences_command()
            }
            Ok(None) => iced::Task::none(),
            Err(error) => {
                self.show_global_error(error);
                iced::Task::none()
            }
        }
    }

    fn commit_transfer_device_alias(&mut self) -> iced::Task<Message> {
        let trimmed = self.transfer_device_alias_input.trim().to_owned();
        let configured = if trimmed.is_empty() {
            // 清空输入 = 回退 hostname 默认。
            None
        } else if Some(&trimmed) == self.user_config.transfer_device_alias.as_ref() {
            return iced::Task::none();
        } else {
            Some(trimmed)
        };
        self.user_config.transfer_device_alias = configured;
        self.start_or_restart_local_send_service();
        self.persist_user_preferences_command()
    }

    fn remove_trusted_transfer_device(&mut self, fingerprint: String) -> iced::Task<Message> {
        self.user_config
            .transfer_trusted_devices
            .retain(|device| device.fingerprint != fingerprint);
        if let Some(runtime) = self.transfer_runtime.as_ref() {
            runtime.trusted.write().remove(&fingerprint);
        }
        // 移除信任后再次接收需要重新确认（PRD A5）。
        self.persist_user_preferences_command()
    }
}

fn trusted_fingerprint_set(devices: &[TrustedTransferDevice]) -> HashSet<String> {
    devices
        .iter()
        .filter(|device| !device.fingerprint.is_empty())
        .map(|device| device.fingerprint.clone())
        .collect()
}

/// 选中集合 → 下载会话条目：目录走 zip 流式，文件直接流式。
fn qr_items_from_paths(paths: Vec<PathBuf>) -> Vec<QrItem> {
    paths
        .into_iter()
        .map(|path| {
            let display_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            let source = if path.is_dir() {
                QrSource::Dir(path)
            } else {
                QrSource::File(path)
            };
            QrItem {
                display_name,
                source,
            }
        })
        .collect()
}

/// 二维码下载会话：服务句柄随流存活，窗口关闭走 cancel 收尾。
fn qr_transfer_session_command(
    items: Vec<QrItem>,
    cancel: CancellationToken,
) -> iced::Task<Message> {
    iced::Task::stream(
        iced::stream::channel(32, async move |mut output| {
            let server = tokio::select! {
                _ = cancel.cancelled() => return,
                server = QrDownloadServer::start(items) => server,
            };
            let handle = match server {
                Ok(handle) => handle,
                Err(error) => {
                    let _ = output
                        .send(TransferMessage::QrSessionFailed(error.to_string()))
                        .await;
                    return;
                }
            };
            let _ = output
                .send(TransferMessage::QrSessionStarted {
                    urls: handle.urls().to_vec(),
                })
                .await;
            let mut events = handle.events();
            loop {
                tokio::select! {
                _ = cancel.cancelled() => {
                    handle.shutdown();
                    return;
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        // 终态事件后服务自停，流随之结束（句柄 drop 兜底关停）。
                        let done = matches!(event, QrEvent::Finished | QrEvent::TimedOut);
                        let _ = output.send(TransferMessage::QrEvent(event)).await;
                        if done {
                            return;
                        }
                    }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        })
        .map(Message::Transfer),
    )
}

/// LocalSend 直推：进度经 mpsc 进入同一事件流，终态单独回执。
fn direct_send_command(
    target: DeviceInfo,
    items: Vec<PathBuf>,
    display_alias: String,
) -> iced::Task<Message> {
    let fingerprint = target.fingerprint.clone();
    iced::Task::stream(
        iced::stream::channel(16, async move |mut output| {
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<SendProgress>(16);
            // 进度通道满时 send_to_device 内部丢进度，不会阻塞传输本身。
            let mut send_task = tokio::spawn(async move {
                local_send::send_to_device_with_progress(
                    &target,
                    items,
                    &display_alias,
                    progress_tx,
                )
                .await
            });
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
                        let outcome = match result {
                            Ok(Ok(())) => Ok(()),
                            Ok(Err(error)) => Err(error.to_string()),
                            Err(error) => Err(error.to_string()),
                        };
                        let _ = output
                            .send(TransferMessage::DirectSendFinished {
                                fingerprint,
                                outcome,
                            })
                            .await;
                        return;
                    }
                }
            }
            let outcome = match send_task.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = output
                .send(TransferMessage::DirectSendFinished {
                    fingerprint,
                    outcome,
                })
                .await;
        })
        .map(Message::Transfer),
    )
}

/// 设置页接收目录选择（ashpd 目录选择器，复用搜索路径选择先例）。
fn download_dir_chooser_command() -> iced::Task<Message> {
    iced::Task::perform(choose_download_directory(), |outcome| {
        Message::Transfer(TransferMessage::DownloadDirChosen(outcome))
    })
}

async fn choose_download_directory() -> Result<Option<PathBuf>, String> {
    let title = crate::localization::translate_current("Choose receive directory");
    let accept_label = crate::localization::translate_current("Select");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .directory(true)
        .multiple(false)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    selected
        .uris()
        .first()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| "the selected location is not a local directory".to_owned())
        })
        .transpose()
}

/// 服务启动失败的用户可见文案（保留底层错误）。
fn transfer_service_start_failed(error: &LocalSendError) -> String {
    crate::localization::translate_current(&format!("Could not start transfer service: {error}"))
}

#[cfg(test)]
mod tests;
