//! LocalSend 接收端常驻 HTTPS 服务（TCP，端口可配，默认 53317）。
//!
//! 对端身份只在 TLS 握手处确定（强制客户端证书，见 `listener` 与 `tls`）：信任判定、
//! 挂起确认、会话归属都按证书指纹，请求体里自报的 fingerprint 不作数。
//!
//! 端点（前缀 /api/localsend/v2）：`info`、`register`、`prepare-upload`、`upload`、`cancel`；
//! 另回应 v1 的 `info`（同官方：老版官方 App 扫描时探测 v1 路由）。
//! 未信任设备的 prepare-upload 挂起等待 UI 确认（oneshot 回写），超时 30s 回 403，
//! 与 LocalSend 官方 App 行为一致。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::{broadcast, oneshot, watch, Mutex};

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::{AcceptPolicy, Favorite, ReceiverConfig, ServiceEvent};

mod destination;
mod listener;
mod report;
mod sessions;
mod tls;

pub(crate) use listener::{port_already_released, spawn_server, Peer, PortReleased};
pub use report::{ReceiveReport, ReceivedFile, ReceivedOutcome};
pub(crate) use sessions::{CancelQuery, ReceiveSessions, SessionEvents, UploadQuery};
pub(crate) use tls::server_config;

/// 未信任设备 prepare-upload 挂起等待用户确认的超时（design：30s 后 403）。
pub(crate) const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
/// receive 服务共享状态。
pub(crate) struct ReceiveState {
    own_device: DeviceInfo,
    accept: AcceptPolicy,
    pin: Option<String>,
    /// 每个 IP 连续输错 PIN 的次数；接收端换代（重启、改 PIN）即清零。
    pin_failures: Mutex<HashMap<IpAddr, u32>>,
    /// 收藏设备算信任；app-ui 改收藏时立刻换掉快照。
    favorites: watch::Receiver<Vec<Favorite>>,
    /// 不是收藏的设备的挂起确认：session_id → 决策回写端。
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// 放行之后的会话：发送方按握手证书指纹认。
    pub(crate) sessions: ReceiveSessions,
    devices: Arc<Mutex<crate::discovery::DeviceTable>>,
    events: broadcast::Sender<ServiceEvent>,
    confirm_timeout: Duration,
}

impl ReceiveState {
    pub(crate) fn new(
        own_device: DeviceInfo,
        receiver: ReceiverConfig,
        favorites: watch::Receiver<Vec<Favorite>>,
        devices: Arc<Mutex<crate::discovery::DeviceTable>>,
        events: broadcast::Sender<ServiceEvent>,
        confirm_timeout: Duration,
    ) -> Self {
        ReceiveState {
            own_device,
            accept: receiver.accept,
            pin: receiver.pin,
            pin_failures: Mutex::new(HashMap::new()),
            favorites,
            pending: Mutex::new(HashMap::new()),
            sessions: ReceiveSessions::new(
                receiver.download_dir,
                SessionEvents::Service(events.clone()),
            ),
            devices,
            events,
            confirm_timeout,
        }
    }

    /// UI 确认回写：accept=true 放行挂起的 prepare-upload；返回 false 表示会话已不存在。
    pub(crate) async fn resolve_pending(&self, session_id: &str, accept: bool) -> bool {
        let sender = self.pending.lock().await.remove(session_id);
        let Some(sender) = sender else {
            // 会话不存在（已超时/已完成），让 UI 感知决策落空。
            return false;
        };
        sender.send(accept).is_ok()
    }

    /// 不用确认就收：按接收确认档位；收藏按握手时的证书指纹比。
    fn accepts_without_asking(&self, fingerprint: &str) -> bool {
        match self.accept {
            AcceptPolicy::AskEveryone => false,
            AcceptPolicy::AcceptFavorites => self
                .favorites
                .borrow()
                .iter()
                .any(|favorite| favorite.fingerprint == fingerprint),
            AcceptPolicy::AcceptEveryone => true,
        }
    }

    /// 照官方 `pin.rs`：要 PIN 时每个 prepare-upload 都查（收藏也一样）。没带或不对
    /// 回 401；同一 IP 错满 3 次后一律 429（填对也不行），直到接收端换代；填对一次
    /// 清零；没带不算错。
    async fn check_pin(&self, ip: IpAddr, given: Option<&str>) -> Result<(), StatusCode> {
        let Some(required) = &self.pin else {
            return Ok(());
        };
        let mut failures = self.pin_failures.lock().await;
        let count = failures.get(&ip).copied().unwrap_or(0);
        if count >= MAX_PIN_FAILURES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        match given {
            Some(pin) if pin == required => {
                failures.remove(&ip);
                Ok(())
            }
            Some(_) => {
                failures.insert(ip, count + 1);
                Err(StatusCode::UNAUTHORIZED)
            }
            None => Err(StatusCode::UNAUTHORIZED),
        }
    }
}

/// 同一 IP 连续输错几次 PIN 后拦住（同官方）。
const MAX_PIN_FAILURES: u32 = 3;

/// prepare-upload 的查询参数：发送方带的 PIN。
#[derive(serde::Deserialize)]
struct PinQuery {
    pin: Option<String>,
}

pub(crate) fn router(state: Arc<ReceiveState>) -> Router {
    Router::new()
        .route("/api/localsend/v1/info", get(info))
        .route("/api/localsend/v2/info", get(info))
        .route("/api/localsend/v2/register", post(register))
        .route("/api/localsend/v2/prepare-upload", post(prepare_upload))
        .route("/api/localsend/v2/upload", post(upload))
        .route("/api/localsend/v2/cancel", post(cancel))
        .with_state(state)
}

/// 只查询、不登记：不露面的扫描方（关了接收的 Bennu）与老版官方 App 靠它发现本机。
async fn info(State(state): State<Arc<ReceiveState>>) -> Json<DeviceInfo> {
    Json(state.own_device.clone())
}

/// 对端回应我们的 announce 或扫描到本机：登记进设备表并广播 DeviceSeen（双向发现
/// 的另一半）。自报 fingerprint 与证书对不上的不登记，但照常回 200（同官方）。
async fn register(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Json(mut device): Json<DeviceInfo>,
) -> impl IntoResponse {
    if device.fingerprint.to_ascii_uppercase() == peer.fingerprint {
        device.fingerprint = peer.fingerprint;
        let changed =
            state
                .devices
                .lock()
                .await
                .upsert(device, peer.address.ip(), std::time::Instant::now());
        if let Some(device) = changed {
            let _ = state.events.send(ServiceEvent::DeviceSeen(device));
        }
    }
    (StatusCode::OK, Json(state.own_device.clone()))
}

/// 这个请求是不是一段文字消息（官方 `ReceiveSessionState.message` 的规则）：只有一个
/// 文件、类型是 `text/*`（旧版直接写 `text`）、带 preview。
fn text_message(files: &HashMap<String, FileMetadata>) -> Option<String> {
    let mut files = files.values();
    let (Some(file), None) = (files.next(), files.next()) else {
        return None;
    };
    let file_type = file.file_type.as_deref()?;
    let is_text = file_type == "text" || file_type.starts_with("text/");
    is_text.then(|| file.preview.clone()).flatten()
}

/// 生成接收端会话 id / 上传令牌（随机 16 字节 hex，见 lib::random_id）。
fn random_id() -> String {
    crate::random_id()
}

async fn prepare_upload(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Query(query): Query<PinQuery>,
    Json(mut request): Json<PrepareUploadRequest>,
) -> impl IntoResponse {
    if let Err(status) = state
        .check_pin(peer.address.ip(), query.pin.as_deref())
        .await
    {
        return (status, Json(Option::<PrepareUploadResponse>::None));
    }
    // 发送方是证书证明的那台设备（同官方 `certFingerprint ?? info.fingerprint`）；
    // 地址是连接来源，"加入收藏"要记住它。
    request.info.fingerprint = peer.fingerprint;
    request.info.address = Some(peer.address.ip());
    if request.files.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(Option::<PrepareUploadResponse>::None),
        );
    }

    // 文字消息：内容就在请求里，回 204（不要文件内容）。不走接收确认：弹出来的就是
    // 内容本身，没什么要先决定的（同官方，Quick Save 也不管文字）。
    if let Some(text) = text_message(&request.files) {
        let _ = state.events.send(ServiceEvent::MessageReceived {
            device: request.info,
            text,
        });
        return (StatusCode::NO_CONTENT, Json(None));
    }

    let session_id = random_id();
    if state.accepts_without_asking(&request.info.fingerprint) {
        let response = accept_session(&state, &session_id, &request.info, &request.files).await;
        return (StatusCode::CREATED, Json(Some(response)));
    }

    // 要确认：挂起等 UI 确认，超时回 403（协议语义：403 = 拒绝）。
    let (tx, rx) = oneshot::channel();
    state.pending.lock().await.insert(session_id.clone(), tx);
    // 对方在等确认时撤回（断开连接）：这个处理函数会被直接丢掉，靠守卫收起确认。
    let withdrawn = WithdrawnGuard {
        state: Arc::clone(&state),
        session_id: Some(session_id.clone()),
    };
    let _ = state.events.send(ServiceEvent::IncomingRequest {
        device: request.info.clone(),
        session_id: session_id.clone(),
        files: request.files.values().cloned().collect(),
    });
    let accepted = tokio::time::timeout(state.confirm_timeout, rx).await;
    withdrawn.disarm();
    match accepted {
        Ok(Ok(true)) => {
            let response = accept_session(&state, &session_id, &request.info, &request.files).await;
            (StatusCode::CREATED, Json(Some(response)))
        }
        Ok(Ok(false)) | Ok(Err(_)) => {
            // Err(_) = UI 端 drop 了回写端（服务关闭），按拒绝处理。
            let _ = state
                .events
                .send(ServiceEvent::RequestRejected { session_id });
            (
                StatusCode::FORBIDDEN,
                Json(Option::<PrepareUploadResponse>::None),
            )
        }
        Err(_) => {
            state.pending.lock().await.remove(&session_id);
            let _ = state
                .events
                .send(ServiceEvent::RequestRejected { session_id });
            (
                StatusCode::FORBIDDEN,
                Json(Option::<PrepareUploadResponse>::None),
            )
        }
    }
}

/// 等确认时对方撤回请求（连接断开，处理函数被丢掉）：去掉挂起的确认、发
/// `RequestRejected`，界面据此收起确认弹窗，不用等 30 秒超时。
struct WithdrawnGuard {
    state: Arc<ReceiveState>,
    /// None = 已经等到了结果，没撤回。
    session_id: Option<String>,
}

impl WithdrawnGuard {
    fn disarm(mut self) {
        self.session_id = None;
    }
}

impl Drop for WithdrawnGuard {
    fn drop(&mut self) {
        let Some(session_id) = self.session_id.take() else {
            return;
        };
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            if state.pending.lock().await.remove(&session_id).is_some() {
                let _ = state
                    .events
                    .send(ServiceEvent::RequestRejected { session_id });
            }
        });
    }
}

/// 开会话并告诉界面谁发来了什么：自动收下的会话界面没见过确认弹窗，也要能显示进度。
async fn accept_session(
    state: &Arc<ReceiveState>,
    session_id: &str,
    sender: &DeviceInfo,
    files: &HashMap<String, FileMetadata>,
) -> PrepareUploadResponse {
    let response = state
        .sessions
        .open(session_id, &sender.fingerprint, files)
        .await;
    let _ = state.events.send(ServiceEvent::RequestAccepted {
        session_id: session_id.to_owned(),
        device: sender.clone(),
        files: files.values().cloned().collect(),
    });
    response
}

async fn upload(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> impl IntoResponse {
    state.sessions.upload(&peer.fingerprint, &query, body).await
}

/// 对方取消：只有发起这次会话的那张证书能取消。
async fn cancel(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Query(query): Query<CancelQuery>,
) -> StatusCode {
    state
        .sessions
        .cancel_from(&peer.fingerprint, &query.session_id)
        .await
}
