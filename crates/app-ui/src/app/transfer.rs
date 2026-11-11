//! 本地文件传输状态机：常驻 LocalSend 服务、二维码下载会话、LocalSend
//! 直推与接收确认 Modal。
//!
//! 职责边界（design.md 阶段 B）：
//! - 常驻服务在 update 上下文同步启动（iced tokio executor 保证 tokio 上下文，
//!   见 design.md A0 结论）；`ServiceHandle` 常驻不主动关停，设置变更时重启。
//! - 二维码下载服务是一次性会话：服务句柄随事件转发流存活，窗口关闭通过
//!   `CancellationToken` 收尾；完成/超时由服务自停。
//! - 收藏设备（接收时算信任）只存在 UserConfig 里，改动经 `favorites.rs` 推给服务。

use std::path::PathBuf;
use std::time::Instant;

use iced::futures::{SinkExt, StreamExt};
use local_send::{
    DeviceInfo, FileMetadata, LocalSendError, LocalSendService, QrDownloadServer, QrEvent, QrItem,
    QrSource, ReceiverFailure, SendProgress, ServiceEvent,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::transfer_service::service_config;
use super::FileBrowser;
use crate::config::TransferAcceptMode;
use crate::model::Message;

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
    /// 某次发送（任务 id）的进度与结果。
    DirectSendProgress {
        task: u64,
        progress: SendProgress,
    },
    DirectSendFinished {
        task: u64,
        alias: String,
        outcome: DirectSendOutcome,
    },
    /// 一批要发的文件准备中（展开文件夹、算校验）。
    BatchProgress {
        batch: u64,
        progress: local_send::PrepareProgress,
    },
    /// None = 准备被取消了。
    BatchPrepared {
        batch: u64,
        files: Option<std::sync::Arc<local_send::PreparedFiles>>,
    },
    DirectSendDetailsToggled(String),
    DeviceChecked {
        owner: DeviceListOwner,
        fingerprint: String,
        checked: bool,
    },
    SendToCheckedPressed(DeviceListOwner),
    CheckedCleared(DeviceListOwner),
    /// 设备行里的 PIN 输入（按证书指纹找这台设备的这次直推）。
    DirectSendPinChanged {
        fingerprint: String,
        value: String,
    },
    DirectSendPinSubmitted(String),
    /// 设备行的"取消"（含 PIN 输入框旁的）：取消这台设备最新的那次发送。
    DirectSendCancelPressed(String),
    /// 任务面板里 LocalSend 行的"取消"、"清除"（任务 id）。
    TaskCancelRequested(u64),
    TaskClearRequested(u64),
    // ---- LocalSend 窗口"设备 | 记录"与传输记录 ----
    LocalSendPageSelected(LocalSendPage),
    HistoryLoaded(Result<Vec<HistoryRecord>, String>),
    /// 写库没成的原因（只记日志：内存里的记录照样在）；None = 写成了。
    HistorySaved(Option<String>),
    HistoryClearPressed,
    HistoryRecordOpened(String),
    // ---- 接收确认（R7） ----
    IncomingAddToFavoritesToggled(bool),
    IncomingConfirmed {
        accept: bool,
    },
    // ---- 收到的文字（主窗口弹窗，当前显示的是队首那条） ----
    IncomingMessageCopied,
    IncomingMessageLinkOpened,
    /// 浏览器没打开链接的原因；None = 打开了。
    IncomingMessageLinkFailed(Option<String>),
    IncomingMessageClosed,
    // ---- 常驻服务事件流（R6） ----
    ServiceEvent(ServiceEvent),
    /// 事件流积压丢弃（broadcast Lagged）：整体刷新设备表。
    DevicesRefreshRequested,
    /// 设备列表手动刷新按钮：清空快照并立即触发一轮子网扫描。
    RefreshDevicesRequested,
    // ---- 设置（R9） ----
    DownloadDirInputChanged(String),
    DownloadDirCommitted,
    DownloadDirChooserPressed,
    DownloadDirChosen(Result<Option<PathBuf>, String>),
    DeviceAliasInputChanged(String),
    DeviceAliasCommitted,
    ReceiveEnabledToggled,
    AcceptModeSelected(TransferAcceptMode),
    ReceivePinToggled,
    ReceivePinInputChanged(String),
    ReceivePinCommitted,
    PortInputChanged(String),
    PortCommitted,
    // ---- LocalSend 窗口 ----
    /// 侧栏入口：打开或聚焦窗口。
    LocalSendWindowRequested,
    ChooserPressed(ChooserTarget),
    LocalSendPathsChosen(Result<Vec<PathBuf>, String>),
    StagedSizeMeasured {
        path: PathBuf,
        size: StagedSize,
    },
    StagedItemRemoved(PathBuf),
    StagedItemsCleared,
    StagedTabSelected(StagedTab),
    StagedTextEdited(iced::widget::text_editor::Action),
    StagedTextCleared,
    /// LocalSend 窗口点了一台设备：按住 Ctrl 是多选，否则把要发的东西直接发给它。
    LocalSendDevicePressed(DeviceInfo),
    // ---- 收藏设备 ----
    /// 附近设备行的 ☆。
    FavoritePressed(DeviceInfo),
    /// 收藏行的 ★、设置页"删除"。
    UnfavoritePressed(String),
    FavoriteAliasInputChanged {
        fingerprint: String,
        value: String,
    },
    FavoriteAliasCommitted(String),
    AddressForm(AddressFormMessage),
    LinkReceive(LinkReceiveMessage),
}

/// 常驻 LocalSend 服务运行时状态。
pub(crate) struct LocalSendRuntime {
    pub(crate) handle: local_send::ServiceHandle,
    /// 事件订阅重挂时的 resubscribe 种子（订阅侧派生独立接收端）。
    pub(crate) events: broadcast::Receiver<ServiceEvent>,
    /// 订阅身份代：服务重启后强制重挂事件流，旧流随旧服务关闭而终结。
    pub(crate) generation: u64,
    /// 发现的设备快照（fingerprint 去重，UI 列表直接渲染）。
    pub(crate) devices: Vec<DeviceInfo>,
    /// 本服务的接收端没开起来或已停止的原因：随服务换代重置。
    pub(crate) receiver_failure: Option<ReceiverFailure>,
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
    /// 打开窗口时那批文件：二维码分享它们，点设备"发送"也发它们。
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) enum TransferSessionPhase {
    Preparing,
    Ready(QrLinks),
    Failed(String),
}

/// 接收确认 Modal 状态：不是收藏的设备发来文件。
#[derive(Debug, Clone)]
pub(crate) struct IncomingTransferConfirmation {
    pub(crate) device: DeviceInfo,
    pub(crate) session_id: String,
    pub(crate) files: Vec<FileMetadata>,
    pub(crate) add_to_favorites: bool,
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
        let previous = self.transfer_runtime.take();
        let generation = previous
            .as_ref()
            .map_or(0, |runtime| runtime.generation + 1);
        // 旧服务的挂起确认随重启作废：Modal 不能指向死会话。
        self.incoming_transfer = None;
        let config = service_config(&self.user_config);
        // 重启交给服务：旧接收端放掉端口后新接收端才绑定。
        let started = match previous {
            Some(previous) => previous.handle.restart(config),
            None => LocalSendService::start(config),
        };
        match started {
            Ok((handle, events)) => {
                self.transfer_runtime = Some(LocalSendRuntime {
                    handle,
                    events,
                    generation,
                    devices: Vec::new(),
                    receiver_failure: None,
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
            paths: paths.clone(),
        });
        let items = qr_items_from_paths(paths);
        iced::Task::batch([
            self.prepare_auxiliary_window_open(),
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
                    session.phase = TransferSessionPhase::Ready(QrLinks::new(urls));
                }
                iced::Task::none()
            }
            TransferMessage::QrAddressSelected(index) => {
                if let Some(TransferSessionPhase::Ready(links)) = self
                    .transfer_session
                    .as_mut()
                    .map(|session| &mut session.phase)
                {
                    links.choose(index);
                }
                iced::Task::none()
            }
            TransferMessage::QrAddressListRequested => {
                if let Some(TransferSessionPhase::Ready(links)) = self
                    .transfer_session
                    .as_mut()
                    .map(|session| &mut session.phase)
                {
                    links.show_addresses();
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
            TransferMessage::SendToDevicePressed(device) => {
                let Some(session) = &self.transfer_session else {
                    return iced::Task::none();
                };
                let paths = session.paths.clone();
                self.begin_direct_sends(vec![device], Outgoing::Files(paths))
            }
            TransferMessage::DirectSendProgress { task, progress } => {
                self.accept_direct_send_progress(task, progress);
                iced::Task::none()
            }
            TransferMessage::DirectSendFinished {
                task,
                alias,
                outcome,
            } => self.accept_direct_send_finished(task, &alias, outcome),
            TransferMessage::BatchProgress { batch, progress } => {
                self.accept_batch_progress(batch, progress);
                iced::Task::none()
            }
            TransferMessage::BatchPrepared { batch, files } => {
                self.accept_batch_prepared(batch, files)
            }
            TransferMessage::DirectSendDetailsToggled(fingerprint) => {
                self.toggle_direct_send_details(&fingerprint);
                iced::Task::none()
            }
            TransferMessage::DeviceChecked {
                owner,
                fingerprint,
                checked,
            } => {
                self.check_device(owner, fingerprint, checked);
                iced::Task::none()
            }
            TransferMessage::SendToCheckedPressed(owner) => self.send_to_checked_devices(owner),
            TransferMessage::CheckedCleared(owner) => {
                self.checked_devices.clear(owner);
                iced::Task::none()
            }
            TransferMessage::DirectSendPinChanged { fingerprint, value } => {
                self.edit_direct_send_pin(&fingerprint, value);
                iced::Task::none()
            }
            TransferMessage::DirectSendPinSubmitted(fingerprint) => {
                self.submit_direct_send_pin(&fingerprint)
            }
            TransferMessage::DirectSendCancelPressed(fingerprint) => {
                self.cancel_latest_send(&fingerprint)
            }
            TransferMessage::TaskCancelRequested(task) => self.cancel_transfer_task(task),
            TransferMessage::LocalSendPageSelected(page) => {
                self.local_send_page = page;
                iced::Task::none()
            }
            TransferMessage::HistoryLoaded(outcome) => {
                self.accept_history_loaded(outcome);
                iced::Task::none()
            }
            TransferMessage::HistorySaved(error) => {
                if let Some(error) = error {
                    tracing::warn!(
                        target: "app_ui::transfer",
                        event = "localsend_history_save_failed",
                        %error,
                        "could not save the LocalSend transfer history"
                    );
                }
                iced::Task::none()
            }
            TransferMessage::HistoryClearPressed => self.clear_transfer_history(),
            TransferMessage::HistoryRecordOpened(id) => self.open_history_record(&id),
            TransferMessage::TaskClearRequested(task) => {
                self.transfer_tasks.clear(task);
                iced::Task::none()
            }
            TransferMessage::IncomingAddToFavoritesToggled(add) => {
                if let Some(incoming) = &mut self.incoming_transfer {
                    incoming.add_to_favorites = add;
                }
                iced::Task::none()
            }
            TransferMessage::IncomingConfirmed { accept } => self.confirm_incoming_transfer(accept),
            TransferMessage::IncomingMessageCopied => self.copy_incoming_message(),
            TransferMessage::IncomingMessageLinkOpened => self.open_incoming_message_link(),
            TransferMessage::IncomingMessageLinkFailed(error) => {
                if let Some(error) = error {
                    self.show_global_error(error);
                }
                iced::Task::none()
            }
            TransferMessage::IncomingMessageClosed => {
                self.close_incoming_message();
                iced::Task::none()
            }
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
            TransferMessage::DownloadDirInputChanged(value) => {
                self.edit_transfer_download_dir(value);
                iced::Task::none()
            }
            TransferMessage::DownloadDirCommitted => self.commit_transfer_download_dir(),
            TransferMessage::DownloadDirChooserPressed => settings::download_dir_chooser_command(),
            TransferMessage::DownloadDirChosen(outcome) => {
                self.accept_transfer_download_dir(outcome)
            }
            TransferMessage::DeviceAliasInputChanged(value) => {
                self.transfer_device_alias_input = value;
                iced::Task::none()
            }
            TransferMessage::DeviceAliasCommitted => self.commit_transfer_device_alias(),
            TransferMessage::ReceiveEnabledToggled => self.toggle_transfer_receive(),
            TransferMessage::PortInputChanged(value) => {
                self.edit_transfer_port(value);
                iced::Task::none()
            }
            TransferMessage::PortCommitted => self.commit_transfer_port(),
            TransferMessage::AcceptModeSelected(mode) => self.select_transfer_accept_mode(mode),
            TransferMessage::ReceivePinToggled => self.toggle_receive_pin(),
            TransferMessage::ReceivePinInputChanged(value) => {
                self.edit_receive_pin(value);
                iced::Task::none()
            }
            TransferMessage::ReceivePinCommitted => self.commit_receive_pin(),
            TransferMessage::LocalSendWindowRequested => self.open_local_send_window(),
            TransferMessage::ChooserPressed(target) => local_send_window::chooser_command(target),
            TransferMessage::LocalSendPathsChosen(Ok(paths)) => self.stage_local_send_paths(paths),
            TransferMessage::LocalSendPathsChosen(Err(error)) => {
                self.show_global_error(error);
                iced::Task::none()
            }
            TransferMessage::StagedSizeMeasured { path, size } => {
                self.accept_staged_size(&path, size);
                iced::Task::none()
            }
            TransferMessage::StagedItemRemoved(path) => {
                self.remove_staged_item(&path);
                iced::Task::none()
            }
            TransferMessage::StagedItemsCleared => {
                self.clear_staged_items();
                iced::Task::none()
            }
            TransferMessage::StagedTabSelected(tab) => {
                self.local_send_tab = tab;
                iced::Task::none()
            }
            TransferMessage::StagedTextEdited(action) => {
                self.edit_staged_text(action);
                iced::Task::none()
            }
            TransferMessage::StagedTextCleared => {
                self.local_send_text = iced::widget::text_editor::Content::new();
                iced::Task::none()
            }
            TransferMessage::LocalSendDevicePressed(device) => self.press_local_send_device(device),
            TransferMessage::FavoritePressed(device) => self.favor_device(&device, None),
            TransferMessage::UnfavoritePressed(fingerprint) => self.unfavor_device(&fingerprint),
            TransferMessage::FavoriteAliasInputChanged { fingerprint, value } => {
                self.edit_favorite_alias(fingerprint, value);
                iced::Task::none()
            }
            TransferMessage::FavoriteAliasCommitted(fingerprint) => {
                self.commit_favorite_alias(&fingerprint)
            }
            TransferMessage::AddressForm(message) => self.handle_address_form(message),
            TransferMessage::LinkReceive(message) => self.handle_link_receive(message),
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

    /// 接收确认回写：要加入收藏时先换掉服务的收藏快照，再放行这次请求。
    fn confirm_incoming_transfer(&mut self, accept: bool) -> iced::Task<Message> {
        let Some(incoming) = self.incoming_transfer.take() else {
            return iced::Task::none();
        };
        let persist = if accept && incoming.add_to_favorites {
            self.favor_device(&incoming.device, None)
        } else {
            iced::Task::none()
        };
        let Some(runtime) = self.transfer_runtime.as_ref() else {
            return persist;
        };
        if accept {
            runtime.handle.accept_session(&incoming.session_id);
        } else {
            runtime.handle.reject_session(&incoming.session_id);
        }
        persist
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
                let favorite_refresh = self.refresh_favorite_from(&device);
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
                return favorite_refresh;
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
            ServiceEvent::RequestAccepted {
                session_id,
                device,
                files,
            } => {
                let sender = self.transfer_device_name(&device);
                self.accept_receive_started(session_id, sender, ReceiveSource::LocalSend, files);
            }
            ServiceEvent::RequestRejected { session_id } => {
                return self.accept_request_withdrawn(&session_id);
            }
            ServiceEvent::ReceiveProgress {
                session_id,
                file_id,
                received,
            } => self.accept_receive_progress(&session_id, file_id, received),
            ServiceEvent::ReceiveFinished { session_id, report } => {
                return self.finish_receive(&session_id, &report);
            }
            ServiceEvent::MessageReceived { device, text } => {
                return self.accept_incoming_message(device, text);
            }
            ServiceEvent::ReceiverFailed(failure) => {
                // 弹一次；设置页从运行时读原因常驻显示。
                self.show_global_error(settings::receiver_failure_message(&failure));
                if let Some(runtime) = self.transfer_runtime.as_mut() {
                    runtime.receiver_failure = Some(failure);
                }
            }
        }
        iced::Task::none()
    }

    /// 传输弹窗（接收确认、收到的文字）要盖在主窗口上：先清掉主窗口同层的输入状态
    /// （菜单、拖拽、框选、地址栏编辑），进行中的重命名照常提交。
    fn prepare_transfer_modal(&mut self) -> iced::Task<Message> {
        self.context_menu = None;
        self.open_with = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        self.commit_rename_if_active()
    }

    /// 不是收藏的设备发来文件：清理主窗口同层输入状态后弹出确认 Modal。
    fn begin_incoming_transfer_confirmation(
        &mut self,
        device: DeviceInfo,
        session_id: String,
        files: Vec<FileMetadata>,
    ) -> iced::Task<Message> {
        let rename_command = self.prepare_transfer_modal();
        self.incoming_transfer = Some(IncomingTransferConfirmation {
            device,
            session_id,
            files,
            add_to_favorites: false,
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
        let summary = tr!("Files received");
        let body = if failed.is_empty() {
            tr!("Received %1 file(s).", saved_paths.len())
        } else {
            tr!(
                "Received %1 file(s), %2 failed.",
                saved_paths.len(),
                failed.len()
            )
        };
        crate::commands::publish_desktop_notification_command(summary, body)
    }
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

/// 服务启动失败的用户可见文案（保留底层错误）。
fn transfer_service_start_failed(error: &LocalSendError) -> String {
    tr!("Could not start transfer service: %1", error)
}

mod device_checks;
mod direct_send;
mod favorites;
mod history;
mod incoming;
mod link_receive;
mod local_send_window;
mod messages;
mod qr_links;
mod receive_tasks;
mod settings;
mod tasks;

pub(crate) use device_checks::{CheckedDevices, DeviceListOwner};
pub(crate) use direct_send::{
    DirectSendFailure, DirectSendOutcome, DirectSendPhase, DirectSendState, Outgoing,
};
pub(crate) use favorites::{
    service_favorites, AddressFormMessage, FavoriteAddressForm, FavoriteRow,
};
pub(crate) use history::{Direction, HistoryItem, HistoryRecord, HistoryResult, TransferHistory};
pub(crate) use incoming::{incoming_entries, IncomingEntry};
pub(crate) use link_receive::{LinkReceiveMessage, LinkReceivePhase, LinkReceiving};
pub(crate) use local_send_window::LocalSendPage;
pub(crate) use local_send_window::{ChooserTarget, StagedItem, StagedSize, StagedTab};
pub(crate) use messages::{message_link, IncomingMessage, MessageTitle};
pub(crate) use qr_links::QrLinks;
pub(crate) use tasks::{
    ReceivePhase, ReceiveSource, ReceiveTask, TransferKind, TransferTask, TransferTasks,
};

pub(crate) use settings::receiver_failure_message;

#[cfg(test)]
pub(crate) mod tests;
