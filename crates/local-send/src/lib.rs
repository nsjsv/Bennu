//! local-send：LocalSend v2 协议层 + 二维码下载服务。
//!
//! 纯协议/服务层，只依赖 tokio 生态，不依赖 iced；app-ui 通过本文件暴露的
//! 窄接口消费。公共 API 原则上只增不改形；例外是设备身份（2026-10）：本机
//! fingerprint 必须等于出示的客户端证书哈希，旧接口让调用方自填 fingerprint、
//! 直推每次另造身份，表达不了这条约束，所以 `ServiceConfig` 与直推入口改了形。
//! 第二次例外是接收开关（2026-11）：接收与发现原是一个整体，关接收只能连发现
//! 一起关；`ServiceConfig` 拆出可缺省的 `ReceiverConfig`，重启改由
//! `ServiceHandle::restart` 等旧服务放掉接收端口后再绑定。第三次例外是收藏设备
//! （2026-11）：信任回调只能回答"信不信"，扫描还要知道收藏设备记住的地址，所以
//! 换成服务持有的收藏快照（`ServiceConfig.favorites` + `ServiceHandle::set_favorites`）。
//!
//! 全部服务任务要求在 tokio 运行时上下文中使用（app-ui 的 iced tokio executor
//! 已在 update/命令上下文进入 tokio runtime，见 design.md 阶段 A 结论）。

mod device;
mod discovery;
mod error;
mod identity;
mod outbound_tls;
mod qr;
mod receive;
mod send;

#[cfg(test)]
mod integration;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, watch, Mutex};
pub use tokio_util::sync::CancellationToken;

pub use device::{Announce, DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
pub use error::LocalSendError;
pub use qr::{QrDownloadHandle, QrItem, QrSource};
pub use receive::{ReceiveReport, ReceivedFile, ReceivedOutcome};
pub use send::prepare::{prepare_files, FileFailure, PrepareProgress, PreparedFiles};
pub use send::{FileOutcome, FileReport, SendProgress, SendReport};

use identity::DeviceIdentity;
use send::DirectSender;

/// 常驻服务事件流（broadcast 容量有界；积压时 UI 收到 `Lagged` 应整体刷新）。
const EVENT_CAPACITY: usize = 128;

/// LocalSend 协议默认端口：发现（UDP 组播）与接收（TCP）共用。
pub const DEFAULT_PORT: u16 = 53317;

/// 常驻服务配置：只含调用方真正拥有的字段；协议字段（version、fingerprint、
/// port、protocol）由服务按设备身份组装，调用方无从传入与身份不符的 fingerprint。
pub struct ServiceConfig {
    /// 本机对外别名（announce、register、prepare-upload 共用）。
    pub alias: String,
    /// LocalSend 官方设备类型词表中的取值（桌面端为 `desktop`）。
    pub device_type: Option<String>,
    /// 发现与接收共用的端口：官方把组播端口与 HTTP 端口当同一个设置，改了端口
    /// 只能和同端口的设备互相发现，这里保持一致。
    pub port: u16,
    /// 设备身份文件（证书 + 私钥 PEM）：不存在时生成；内容不合法或读写失败时服务不启动。
    pub identity_path: PathBuf,
    /// 接收端；None = 不接收：不监听 TCP，也不在网络上露面（不 announce、不回应、
    /// 扫描只查询不登记），发现与直推照常。
    pub receiver: Option<ReceiverConfig>,
    /// 收藏设备：接收时算信任，扫描时按记住的地址直接连。之后经
    /// `ServiceHandle::set_favorites` 更新，不用重启。
    pub favorites: Vec<Favorite>,
}

/// 接收端配置：只有接收会用到的字段。改任何一项都经 `restart` 生效（同官方改 PIN
/// 重启服务）：PIN 错误计数随旧接收端一起清零，正是"改了 PIN 就放开"。
pub struct ReceiverConfig {
    /// 接收文件保存目录。
    pub download_dir: PathBuf,
    pub accept: AcceptPolicy,
    /// 发文件进来要带的 PIN（`?pin=`），收藏设备也要；None = 不要 PIN。
    pub pin: Option<String>,
}

/// 谁发来的文件不用确认就收（官方 Quick Save / Quick Save for Favorites）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptPolicy {
    AskEveryone,
    AcceptFavorites,
    AcceptEveryone,
}

/// 收藏设备：按证书指纹认人；`address` 是最后一次见到它的地址（IP + 它的端口），
/// 广播被吞、对方在别的网段时扫描靠它找到这台设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favorite {
    pub fingerprint: String,
    pub address: Option<SocketAddr>,
}

/// 按地址找设备（"按 IP 添加"）没找到的原因；文案归 UI。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindDeviceError {
    /// 连不上；附 HTTPS 那一次的失败原因。
    Unreachable(String),
    /// 只有明文 HTTP：认不了身份，不能收藏。
    NotEncrypted,
    /// 这个地址是本机。
    ThisDevice,
}

/// 接收端没开起来或运行中停止的原因；文案归 UI。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverFailure {
    /// 端口已被占用，常见于同机运行着另一个 LocalSend。
    PortInUse { port: u16 },
    /// 其他绑定失败（如权限不足）。
    BindFailed { port: u16, reason: String },
    /// 运行中监听出错，接收已停止。
    Stopped { port: u16, reason: String },
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
    /// 会话被接受（收藏/档位自动收下，或用户确认）：谁发来了哪些文件。
    RequestAccepted {
        session_id: String,
        device: DeviceInfo,
        files: Vec<FileMetadata>,
    },
    /// 会话被拒绝（用户拒绝或 30s 确认超时）。
    RequestRejected { session_id: String },
    /// 接收进度（按 ≥1MiB 增量节流）。
    /// 某个文件已收到的字节（按 ≥1MiB 增量节流，收完再报一次）；重传时从头算。
    ReceiveProgress {
        session_id: String,
        file_id: String,
        received: u64,
    },
    /// 会话结束（收完、对方取消、本机取消）：每个文件怎样了。
    ReceiveFinished {
        session_id: String,
        report: ReceiveReport,
    },
    /// 收到一段文字消息（已回 204，不建会话、不走接收确认）；发送方按证书认。
    MessageReceived { device: DeviceInfo, text: String },
    /// 接收端没开起来或已停止：每个服务只报一次，不重试；发现与直推不受影响。
    ReceiverFailed(ReceiverFailure),
}

/// 常驻服务句柄：直推、确认回写、手动刷新与重启。drop 即关停（关停通道随之关闭，
/// 发现与接收任务都会退出），常驻靠调用方一直持有它。
pub struct ServiceHandle {
    commands: mpsc::UnboundedSender<ServiceCommand>,
    shutdown_tx: watch::Sender<bool>,
    sender: DirectSender,
    receiver_released: receive::PortReleased,
    favorites: watch::Sender<Vec<Favorite>>,
    discovery: Arc<discovery::Discovery>,
}

/// UI → 常驻服务的命令（经无界通道路由到各服务任务）。
enum ServiceCommand {
    /// 接收确认 Modal 的回写。
    ResolveSession { session_id: String, accept: bool },
    /// 手动刷新：清空设备表（逐个广播 DeviceLost）并立即触发一轮扫描。
    RefreshDevices,
    /// 本机取消一次接收。
    CancelReceive { session_id: String },
}

impl ServiceHandle {
    /// 直推准备好的文件（`prepare_files`，几台设备共用一份）到目标设备，出示本机设备
    /// 身份。`pin` 是对方要的 PIN：先不带，对方回 `PinRequired` 再让用户输了带上重发
    /// （同官方），重发不用重新准备。
    ///
    /// 返回的 future 自带客户端与本机设备信息的克隆，不借用句柄，可直接
    /// `tokio::spawn`；进度通道满时丢弃进度，不反压传输。
    /// `cancel`：还在等对方接受时取消 = 撤回；传输中取消 = 断掉并通知对方（对方删掉
    /// 收了一半的）；都返回 `Canceled`。
    pub fn send_to_device(
        &self,
        target: &DeviceInfo,
        files: Arc<PreparedFiles>,
        pin: Option<String>,
        progress: mpsc::Sender<SendProgress>,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<SendReport, LocalSendError>> + Send + 'static {
        let sender = self.sender.clone();
        let target = target.clone();
        async move {
            sender
                .send(&target, &files, pin.as_deref(), progress, &cancel)
                .await
        }
    }

    /// 发一段文字给目标设备（同官方的文字消息）；`pin`、`cancel` 同 `send_to_device`。
    pub fn send_text(
        &self,
        target: &DeviceInfo,
        text: String,
        pin: Option<String>,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<(), LocalSendError>> + Send + 'static {
        let sender = self.sender.clone();
        let target = target.clone();
        async move {
            sender
                .send_text(&target, &text, pin.as_deref(), &cancel)
                .await
        }
    }

    /// 确认 Modal 回写：接受不是收藏的设备的请求。要"加入收藏"时先 `set_favorites`。
    pub fn accept_session(&self, session_id: &str) {
        let _ = self.commands.send(ServiceCommand::ResolveSession {
            session_id: session_id.to_string(),
            accept: true,
        });
    }

    /// 拒绝未信任设备的请求（挂起中的 prepare-upload 将得到 403）。
    pub fn reject_session(&self, session_id: &str) {
        let _ = self.commands.send(ServiceCommand::ResolveSession {
            session_id: session_id.to_string(),
            accept: false,
        });
    }

    /// 换收藏快照：立刻生效，下一个接收请求、下一轮扫描就按它来。持久化归 app-ui。
    pub fn set_favorites(&self, favorites: Vec<Favorite>) {
        self.favorites.send_replace(favorites);
    }

    /// 按地址找设备（"按 IP 添加"）：只认开了加密的设备，身份取自握手证书；找到的
    /// 设备同时登记进设备表，马上出现在列表里。
    pub fn find_device(
        &self,
        address: SocketAddr,
    ) -> impl Future<Output = Result<DeviceInfo, FindDeviceError>> + Send + 'static {
        let discovery = Arc::clone(&self.discovery);
        async move { discovery::find_device(&discovery, address).await }
    }

    /// 取消一次接收：收完的留着，收了一半的删掉，对方后面的文件发不进来；随后照常发
    /// `ReceiveFinished`。
    pub fn cancel_receive(&self, session_id: &str) {
        let _ = self.commands.send(ServiceCommand::CancelReceive {
            session_id: session_id.to_string(),
        });
    }

    /// 手动刷新设备列表：清空现有条目并立即触发一轮子网扫描。
    pub fn refresh_devices(&self) {
        let _ = self.commands.send(ServiceCommand::RefreshDevices);
    }

    /// 按新配置重启：关停本服务，新服务的接收端等本服务放掉接收端口后再绑定。
    /// 进行中的接收照常传完；不等端口放掉就绑定，同端口必然冲突。
    pub fn restart(
        self,
        config: ServiceConfig,
    ) -> Result<(ServiceHandle, broadcast::Receiver<ServiceEvent>), LocalSendError> {
        let _ = self.shutdown_tx.send(true);
        LocalSendService::start_after(config, self.receiver_released)
    }
}

/// 常驻服务门面。
pub struct LocalSendService;

impl LocalSendService {
    /// 启动常驻服务：发现与直推，配置了接收端时再加 TCP 接收并在网络上露面。
    ///
    /// 设备身份不可用（文件内容不合法或读写失败）时返回 Err，此时还没有绑定
    /// 任何端口、没有启动任何任务。接收端口绑定失败不返回 Err：发一次
    /// `ReceiverFailed` 事件后不重试（避免与已运行的 LocalSend 实例冲突风暴），
    /// 发现与直推照常。必须在 tokio 运行时上下文中调用。
    pub fn start(
        config: ServiceConfig,
    ) -> Result<(ServiceHandle, broadcast::Receiver<ServiceEvent>), LocalSendError> {
        Self::start_after(config, receive::port_already_released())
    }

    fn start_after(
        config: ServiceConfig,
        previous_released: receive::PortReleased,
    ) -> Result<(ServiceHandle, broadcast::Receiver<ServiceEvent>), LocalSendError> {
        let identity = DeviceIdentity::load_or_create(&config.identity_path)?;
        let own_device = own_device_info(config.alias, config.device_type, config.port, &identity);
        // 先于任何任务：身份证书当不了 TLS 证书时不碰网络就失败。
        let invalid_identity = |error: rustls::Error| LocalSendError::InvalidIdentity {
            path: config.identity_path.clone(),
            reason: error.to_string(),
        };
        let outbound = outbound_tls::OutboundTls::new(&identity).map_err(invalid_identity)?;
        // 发现用的客户端不钉指纹：还不知道对方是谁，身份从回应的握手证书读。
        let client = outbound.client(None)?;

        let (events, events_rx) = broadcast::channel(EVENT_CAPACITY);
        let devices = Arc::new(Mutex::new(discovery::DeviceTable::new(
            own_device.fingerprint.clone(),
        )));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (favorites, favorites_rx) = watch::channel(config.favorites);

        let (presence, receive_state, receiver_released) = match config.receiver {
            Some(receiver) => {
                let tls = receive::server_config(&identity).map_err(invalid_identity)?;
                let state = Arc::new(receive::ReceiveState::new(
                    own_device.clone(),
                    receiver,
                    favorites_rx.clone(),
                    Arc::clone(&devices),
                    events.clone(),
                    receive::CONFIRM_TIMEOUT,
                ));
                let released = receive::spawn_server(
                    Arc::clone(&state),
                    config.port,
                    tls,
                    previous_released,
                    shutdown_rx.clone(),
                );
                (
                    discovery::Presence::Visible(own_device.clone()),
                    Some(state),
                    released,
                )
            }
            None => (
                discovery::Presence::Hidden,
                None,
                receive::port_already_released(),
            ),
        };

        // sweeper 的手动刷新通知：命令路由收到 RefreshDevices 时 notify 一次。
        let sweeper_notify = Arc::new(tokio::sync::Notify::new());
        let discovery = Arc::new(discovery::Discovery {
            own_fingerprint: own_device.fingerprint.clone(),
            port: config.port,
            presence,
            devices: Arc::clone(&devices),
            events: events.clone(),
            client: client.clone(),
            favorites: favorites_rx,
        });
        discovery::spawn(
            Arc::clone(&discovery),
            Arc::clone(&sweeper_notify),
            shutdown_rx.clone(),
        );

        // 命令路由：handle（UI 线程）→ 服务任务（resolve 挂起的 oneshot / 手动刷新）。
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ServiceCommand>();
        let devices_for_refresh = Arc::clone(&devices);
        let events_for_refresh = events.clone();
        let sweeper_notify_for_refresh = Arc::clone(&sweeper_notify);
        let mut command_shutdown = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(ServiceCommand::ResolveSession { session_id, accept }) => {
                            // 没有接收端就没有挂起的会话，确认回写无处可去。
                            if let Some(state) = &receive_state {
                                state.resolve_pending(&session_id, accept).await;
                            }
                        }
                        Some(ServiceCommand::CancelReceive { session_id }) => {
                            if let Some(state) = &receive_state {
                                state.end_session(&session_id).await;
                            }
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
                sender: DirectSender::new(client, outbound, own_device),
                receiver_released,
                favorites,
                discovery,
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
    port: u16,
    identity: &DeviceIdentity,
) -> DeviceInfo {
    DeviceInfo {
        alias,
        version: "2.0".to_owned(),
        fingerprint: identity.fingerprint().to_owned(),
        device_model: None,
        device_type,
        download: None,
        port,
        // 接收端只提供 HTTPS（服务端证书即设备身份证书，官方客户端按 fingerprint 钉扎）。
        protocol: "https".to_owned(),
        address: None,
    }
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
            port: 0,
            identity_path: identity_path.clone(),
            receiver: Some(ReceiverConfig {
                download_dir: dir.path().to_path_buf(),
                accept: AcceptPolicy::AcceptFavorites,
                pin: None,
            }),
            favorites: Vec::new(),
        };

        let error = LocalSendService::start(config).err().unwrap();

        assert!(
            matches!(error, LocalSendError::InvalidIdentity { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&identity_path).unwrap(), b"broken");
    }
}
