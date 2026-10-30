//! 常驻 LocalSend 服务的装配层：事件订阅与服务配置。
//!
//! 从 transfer.rs 拆出的服务接线职责：状态机只消费这里的产物，
//! 不直接接触协议装配细节。

use std::collections::HashSet;
use std::sync::Arc;

use iced::futures::SinkExt;
use local_send::{DeviceInfo, ServiceConfig, ServiceEvent, TrustPredicate};
use tokio::sync::broadcast;
use tracing::warn;

use super::transfer::TransferMessage;
use crate::config::default_transfer_device_alias;
use crate::model::Message;

/// LocalSend 协议固定端口（PRD R6）。
pub(crate) const LOCAL_SEND_PORT: u16 = 53317;
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

/// 组装常驻服务配置：本机设备信息 + 信任查询回调。
pub(crate) fn service_config(
    user_config: &crate::config::UserConfig,
    trusted: Arc<parking_lot::RwLock<HashSet<String>>>,
) -> ServiceConfig {
    let is_trusted: TrustPredicate = Arc::new(move |device: &DeviceInfo| {
        // 空 fingerprint 不能作为信任身份；锁中毒按未信任处理（fail closed）。
        !device.fingerprint.is_empty() && trusted.read().contains(&device.fingerprint)
    });
    ServiceConfig {
        alias: user_config
            .transfer_device_alias
            .clone()
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or_else(default_transfer_device_alias),
        // LocalSend 官方设备类型词表中的桌面端取值。
        device_type: Some("computer".to_owned()),
        listen_port: LOCAL_SEND_PORT,
        download_dir: transfer_download_dir(user_config),
        identity_path: file_core::app_dirs::app_data_dir().join(LOCAL_SEND_IDENTITY_FILE_NAME),
        is_trusted,
    }
}
