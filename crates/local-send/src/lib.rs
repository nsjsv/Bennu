//! local-send：LocalSend v2 协议层 + 二维码下载服务。
//!
//! 纯协议/服务层，只依赖 tokio 生态，不依赖 iced；app-ui 通过本文件暴露的
//! 窄接口消费。公共 API 原则上只增不改形；例外是设备身份（2026-10）：本机
//! fingerprint 必须等于出示的客户端证书哈希，旧接口让调用方自填 fingerprint、
//! 直推每次另造身份，表达不了这条约束，所以 `ServiceConfig` 与直推入口改了形。
//!
//! 全部服务任务要求在 tokio 运行时上下文中使用（app-ui 的 iced tokio executor
//! 已在 update/命令上下文进入 tokio runtime，见 design.md 阶段 A 结论）。

mod device;
mod discovery;
mod error;
mod identity;
mod qr;
mod receive;
mod send;

#[cfg(test)]
mod integration;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, watch, Mutex};

pub use device::{Announce, DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
pub use error::LocalSendError;
pub use qr::{QrDownloadHandle, QrItem, QrSource};
pub use send::SendProgress;

use identity::DeviceIdentity;
use send::DirectSender;

/// 常驻服务事件流（broadcast 容量有界；积压时 UI 收到 `Lagged` 应整体刷新）。
const EVENT_CAPACITY: usize = 128;

/// 信任判定回调：UI 注入（UserPreferences 的信任设备表），服务只查询不存储。
pub type TrustPredicate = Arc<dyn Fn(&DeviceInfo) -> bool + Send + Sync>;

/// 常驻服务配置：只含调用方真正拥有的字段；协议字段（version、fingerprint、
/// port、protocol）由服务按设备身份组装，调用方无从传入与身份不符的 fingerprint。
pub struct ServiceConfig {
    /// 本机对外别名（announce、register、prepare-upload 共用）。
    pub alias: String,
    /// LocalSend 官方设备类型词表中的取值（桌面端为 `computer`）。
    pub device_type: Option<String>,
    /// 接收端 HTTP 监听端口（53317）。
    pub listen_port: u16,
    /// 接收文件保存目录。
    pub download_dir: PathBuf,
    /// 设备身份文件（证书 + 私钥 PEM）：不存在时生成；内容不合法或读写失败时服务不启动。
    pub identity_path: PathBuf,
    /// 信任设备查询回调。
    pub is_trusted: TrustPredicate,
}

/// 二维码下载会话事件（design.md 契约）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrEvent {
    /// 首个请求命中（手机已扫码打开页面）。
    Connected,
    /// 累计发送字节与是否有传输进行中（监控任务 ~1s 节流）。
    Progress { bytes_sent: u64, active: bool },
    /// 全部条目已完整下载，服务即将自动关停。
    Finished,
    /// 超过空闲时限（默认 10 分钟）无活动，服务自动关停。
    TimedOut,
}

/// 常驻服务事件。
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    /// 发现新设备或设备信息变化（address 已回填来源 IP）。
    DeviceSeen(DeviceInfo),
    /// 设备超过 30s 未刷新被剔除（UI 移除列表条目）。
    DeviceLost(DeviceInfo),
    /// 未信任设备请求发送文件，等待 UI 确认。
    IncomingRequest {
        device: DeviceInfo,
        session_id: String,
        files: Vec<FileMetadata>,
    },
    /// 会话被接受（信任设备自动放行或用户确认）。
    RequestAccepted { session_id: String },
    /// 会话被拒绝（用户拒绝或 30s 确认超时）。
    RequestRejected { session_id: String },
    /// 接收进度（按 ≥1MiB 增量节流）。
    ReceiveProgress {
        session_id: String,
        file_name: String,
        received: u64,
        total: u64,
    },
    /// 会话结束：saved_paths 为成功落盘文件，failed 为 (文件名, 原因)。
    ReceiveFinished {
        session_id: String,
        saved_paths: Vec<PathBuf>,
        failed: Vec<(String, String)>,
    },
    /// 服务级错误（端口占用等）：只报一次，UI 展示后不再重试。
    ServiceError(String),
}

/// 常驻服务句柄：直推、确认回写、手动刷新与关停；drop 不会停服务（常驻语义，随应用退出）。
pub struct ServiceHandle {
    commands: mpsc::UnboundedSender<ServiceCommand>,
    shutdown_tx: watch::Sender<bool>,
    sender: DirectSender,
}

/// UI → 常驻服务的命令（经无界通道路由到各服务任务）。
enum ServiceCommand {
    /// 接收确认 Modal 的回写。
    ResolveSession {
        session_id: String,
        accept: bool,
        remember: bool,
    },
    /// 手动刷新：清空设备表（逐个广播 DeviceLost）并立即触发一轮扫描。
    RefreshDevices,
}

impl ServiceHandle {
    /// 直推文件清单到目标设备，出示本机设备身份。
    ///
    /// 返回的 future 自带客户端与本机设备信息的克隆，不借用句柄，可直接
    /// `tokio::spawn`；进度通道满时丢弃进度，不反压传输。
    pub fn send_to_device(
        &self,
        target: &DeviceInfo,
        items: Vec<PathBuf>,
        progress: mpsc::Sender<SendProgress>,
    ) -> impl Future<Output = Result<(), LocalSendError>> + Send + 'static {
        let sender = self.sender.clone();
        let target = target.clone();
        async move { sender.send(&target, items, progress).await }
    }

    /// 确认 Modal 回写：接受未信任设备的请求；remember=true 同时补入内存信任集。
    /// 持久化信任设备归 app-ui。
    pub fn accept_session(&self, session_id: &str, remember: bool) {
        let _ = self.commands.send(ServiceCommand::ResolveSession {
            session_id: session_id.to_string(),
            accept: true,
            remember,
        });
    }

    /// 拒绝未信任设备的请求（挂起中的 prepare-upload 将得到 403）。
    pub fn reject_session(&self, session_id: &str) {
        let _ = self.commands.send(ServiceCommand::ResolveSession {
            session_id: session_id.to_string(),
            accept: false,
            remember: false,
        });
    }

    /// 手动刷新设备列表：清空现有条目并立即触发一轮子网扫描。
    pub fn refresh_devices(&self) {
        let _ = self.commands.send(ServiceCommand::RefreshDevices);
    }

    /// 关停常驻服务：停 UDP 发现与 HTTP 接收，释放端口。
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// 常驻服务门面。
pub struct LocalSendService;

impl LocalSendService {
    /// 启动常驻服务：UDP 组播发现 + 接收端 HTTP。
    ///
    /// 设备身份不可用（文件内容不合法或读写失败）时返回 Err，此时还没有绑定
    /// 任何端口、没有启动任何任务。端口绑定失败不返回 Err：按 N2 发一次
    /// `ServiceError` 事件后保持惰性，UI 只需展示错误，服务不重试（避免与已运行的
    /// LocalSend 实例冲突风暴）。必须在 tokio 运行时上下文中调用。
    pub fn start(
        config: ServiceConfig,
    ) -> Result<(ServiceHandle, broadcast::Receiver<ServiceEvent>), LocalSendError> {
        let identity = DeviceIdentity::load_or_create(&config.identity_path)?;
        let own_device = own_device_info(
            config.alias,
            config.device_type,
            config.listen_port,
            &identity,
        );
        let client = peer_client(&identity)?;

        let (events, events_rx) = broadcast::channel(EVENT_CAPACITY);
        let devices = Arc::new(Mutex::new(discovery::DeviceTable::default()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // sweeper 的手动刷新通知：命令路由收到 RefreshDevices 时 notify 一次。
        let sweeper_notify = Arc::new(tokio::sync::Notify::new());
        discovery::spawn(
            own_device.clone(),
            Arc::clone(&devices),
            events.clone(),
            client.clone(),
            Arc::clone(&sweeper_notify),
            shutdown_rx.clone(),
        );

        // 接收端 HTTP：绑定失败同样走 ServiceError 事件而非 Err。
        let receive_state = Arc::new(receive::ReceiveState::new(
            own_device.clone(),
            config.download_dir,
            config.is_trusted,
            Arc::clone(&devices),
            events.clone(),
            receive::CONFIRM_TIMEOUT,
        ));
        let router = receive::router(Arc::clone(&receive_state));
        let port = config.listen_port;
        let service_events = events.clone();
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                Ok(listener) => {
                    let _ = axum::serve(
                        listener,
                        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                    )
                    .await;
                }
                Err(error) => {
                    // N2：端口被占（如另一 LocalSend 实例）只报一次，不重试。
                    tracing::warn!(event = "localsend_tcp_bind_failed", %error, port, "tcp bind failed");
                    let _ = service_events.send(ServiceEvent::ServiceError(format!(
                        "TCP 端口 {port} 被占用（可能有另一个 LocalSend 应用正在运行）: {error}"
                    )));
                }
            }
        });

        // 命令路由：handle（UI 线程）→ 服务任务（resolve 挂起的 oneshot / 手动刷新）。
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ServiceCommand>();
        let receive_for_commands = Arc::clone(&receive_state);
        let devices_for_refresh = Arc::clone(&devices);
        let events_for_refresh = events.clone();
        let sweeper_notify_for_refresh = Arc::clone(&sweeper_notify);
        let mut command_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(ServiceCommand::ResolveSession { session_id, accept, remember }) => {
                            receive_for_commands
                                .resolve_pending(&session_id, accept, remember)
                                .await;
                        }
                        Some(ServiceCommand::RefreshDevices) => {
                            // 清空设备表并逐个广播 DeviceLost，再立即触发一轮扫描。
                            let cleared = devices_for_refresh.lock().await.clear();
                            for device in cleared {
                                let _ = events_for_refresh.send(ServiceEvent::DeviceLost(device));
                            }
                            sweeper_notify_for_refresh.notify_one();
                        }
                        None => return,
                    },
                    _ = command_shutdown.changed() => return,
                }
            }
        });

        Ok((
            ServiceHandle {
                commands: command_tx,
                shutdown_tx,
                sender: DirectSender::new(client, own_device),
            },
            events_rx,
        ))
    }
}

/// 本机对外设备信息：fingerprint 只取自设备身份，announce、register、
/// prepare-upload 都用这一份。
pub(crate) fn own_device_info(
    alias: String,
    device_type: Option<String>,
    listen_port: u16,
    identity: &DeviceIdentity,
) -> DeviceInfo {
    DeviceInfo {
        alias,
        version: "2.0".to_owned(),
        fingerprint: identity.fingerprint().to_owned(),
        device_model: None,
        device_type,
        download: None,
        port: listen_port,
        protocol: "http".to_owned(),
        address: None,
    }
}

/// 唯一的出站客户端：直推、legacy 扫描、register 回应共用，全部出示设备身份。
pub(crate) fn peer_client(identity: &DeviceIdentity) -> Result<reqwest::Client, LocalSendError> {
    reqwest::Client::builder()
        // 局域网内握手超过 5s 视为对端不可达；总时长不设限（大文件直推）。
        .connect_timeout(Duration::from_secs(5))
        // LocalSend 证书是自签名且无 CA 校验（规范 §2，安全性靠 fingerprint
        // 记忆）；官方客户端同样接受自签名证书，否则 HTTPS 设备全部握手失败。
        .danger_accept_invalid_certs(true)
        // 新版官方 App 对 HTTPS 强制客户端证书，并以证书哈希识别发送方。
        .identity(identity.reqwest_identity()?)
        .build()
        .map_err(|source| LocalSendError::Request {
            endpoint: "client",
            source,
        })
}

/// 二维码下载服务门面。
pub struct QrDownloadServer;

impl QrDownloadServer {
    /// 启动一次性下载服务（默认 10 分钟无活动自动关停）。
    pub async fn start(items: Vec<QrItem>) -> Result<QrDownloadHandle, LocalSendError> {
        qr::start(items).await
    }

    /// 测试/特殊场景：注入空闲超时。
    pub async fn start_with_idle_timeout(
        items: Vec<QrItem>,
        idle_timeout: Duration,
    ) -> Result<QrDownloadHandle, LocalSendError> {
        qr::start_with_idle_timeout(items, idle_timeout).await
    }
}

/// 生成 16 字节随机 hex id（会话 id、上传令牌、下载令牌）。
pub(crate) fn random_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 生成二维码矩阵（阶段 B：矩阵 → RGBA → ImageHandle 渲染）。
pub fn qr_matrix(url: &str) -> fast_qr::QRCode {
    // URL 来自本服务自身构造，编码失败即程序性错误，panic 合理。
    fast_qr::QRBuilder::new(url)
        .ecl(fast_qr::ECL::M)
        .build()
        .expect("qr encode failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_matrix_encodes_url() {
        let code = qr_matrix("http://192.168.1.5:49152/abcdef");
        assert!(code.size >= 21);
    }

    // 普通 #[test] 没有 tokio 运行时：若身份检查晚于 spawn 任务，这里会直接 panic。
    #[test]
    fn start_refuses_an_invalid_identity_before_touching_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let identity_path = dir.path().join("localsend-identity.pem");
        std::fs::write(&identity_path, b"broken").unwrap();
        let config = ServiceConfig {
            alias: "TestPC".to_owned(),
            device_type: None,
            listen_port: 0,
            download_dir: dir.path().to_path_buf(),
            identity_path: identity_path.clone(),
            is_trusted: Arc::new(|_| false),
        };

        let error = LocalSendService::start(config).err().unwrap();

        assert!(
            matches!(error, LocalSendError::InvalidIdentity { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&identity_path).unwrap(), b"broken");
    }
}
