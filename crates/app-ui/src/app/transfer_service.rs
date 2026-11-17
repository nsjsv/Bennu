//! 常驻 LocalSend 服务的装配层：事件订阅、看设备列表的凭据与服务配置。
//!
//! 从 transfer.rs 拆出的服务接线职责：状态机只消费这里的产物，
//! 不直接接触协议装配细节。

use iced::futures::SinkExt;
use local_send::{AcceptPolicy, DeviceListWatch, ReceiverConfig, ServiceConfig, ServiceEvent};
use tokio::sync::broadcast;
use tracing::warn;

use super::transfer::{service_favorites, ReceiverRole, TransferMessage};
use crate::config::{default_transfer_device_alias, TransferAcceptMode};
use crate::model::Message;

/// 设备身份文件（证书 + 私钥，权限 600），放在应用数据目录下。
const LOCAL_SEND_IDENTITY_FILE_NAME: &str = "localsend-identity.pem";

/// 常驻服务事件订阅键：仅以 generation 参与去重，种子接收端不参与比较。
pub(crate) struct LocalSendEventKey {
    generation: u64,
    seed: broadcast::Receiver<ServiceEvent>,
}

impl PartialEq for LocalSendEventKey {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl Eq for LocalSendEventKey {}

impl std::hash::Hash for LocalSendEventKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.generation.hash(state);
    }
}

pub(crate) fn local_send_event_subscription(
    generation: u64,
    seed: broadcast::Receiver<ServiceEvent>,
) -> iced::Subscription<Message> {
    iced::Subscription::run_with(
        LocalSendEventKey { generation, seed },
        local_send_event_stream,
    )
}

fn local_send_event_stream(
    key: &LocalSendEventKey,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let mut events = key.seed.resubscribe();
    iced::stream::channel(128, async move |mut output| loop {
        match events.recv().await {
            Ok(event) => {
                let _ = output
                    .send(Message::Transfer(TransferMessage::ServiceEvent(event)))
                    .await;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let _ = output
                    .send(Message::Transfer(TransferMessage::DevicesRefreshRequested))
                    .await;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    })
}

/// 设备列表显示着时挂上：订阅拿着看列表的凭据，服务只在有凭据时主动找设备。窗口关了
/// 订阅被丢掉，凭据随之放掉；服务换代时按 generation 重挂到新服务上。
struct DeviceListWatchKey {
    generation: u64,
    watch: DeviceListWatch,
}

impl PartialEq for DeviceListWatchKey {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl Eq for DeviceListWatchKey {}

impl std::hash::Hash for DeviceListWatchKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.generation.hash(state);
    }
}

pub(crate) fn device_list_watch_subscription(
    generation: u64,
    watch: DeviceListWatch,
) -> iced::Subscription<Message> {
    iced::Subscription::run_with(
        DeviceListWatchKey { generation, watch },
        device_list_watch_stream,
    )
}

/// 什么都不发，只在活着期间拿着凭据。
fn device_list_watch_stream(
    key: &DeviceListWatchKey,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let lease = key.watch.lease();
    iced::stream::channel(1, async move |_output| {
        let _lease = lease;
        std::future::pending::<()>().await;
    })
}

/// 接收保存目录：用户配置优先，缺省 ~/Downloads；尽力创建。
pub(crate) fn transfer_download_dir(user_config: &crate::config::UserConfig) -> std::path::PathBuf {
    let dir = user_config
        .transfer_download_dir
        .clone()
        .unwrap_or_else(|| file_core::app_dirs::home_dir().join("Downloads"));
    // 目录不存在时尽力创建；失败由每次写入的 per-file 错误暴露。
    if let Err(error) = std::fs::create_dir_all(&dir) {
        warn!(event = "transfer_download_dir_create_failed", %error, "create download dir failed");
    }
    dir
}

/// 组装常驻服务配置：本机设备信息、收藏设备；这个进程负责接收且接收开着时，再带上接收
/// 目录、确认档位与 PIN。
pub(crate) fn service_config(
    user_config: &crate::config::UserConfig,
    role: &ReceiverRole,
) -> ServiceConfig {
    let receiver =
        (role.is_receiving() && user_config.transfer_receive_enabled).then(|| ReceiverConfig {
            download_dir: transfer_download_dir(user_config),
            accept: match user_config.transfer_accept_mode {
                TransferAcceptMode::AskEveryone => AcceptPolicy::AskEveryone,
                TransferAcceptMode::AcceptFavorites => AcceptPolicy::AcceptFavorites,
                TransferAcceptMode::AcceptEveryone => AcceptPolicy::AcceptEveryone,
            },
            pin: user_config.transfer_receive_pin.clone(),
        });
    ServiceConfig {
        alias: user_config
            .transfer_device_alias
            .clone()
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or_else(default_transfer_device_alias),
        // LocalSend 官方设备类型词表（mobile/desktop/web/headless/server）的桌面端取值。
        device_type: Some("desktop".to_owned()),
        port: user_config.transfer_port,
        identity_path: file_core::app_dirs::app_data_dir().join(LOCAL_SEND_IDENTITY_FILE_NAME),
        receiver,
        favorites: service_favorites(&user_config.transfer_favorite_devices),
    }
}
