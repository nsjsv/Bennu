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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, oneshot, watch, Mutex};
use tokio_util::sync::CancellationToken;

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::{AcceptPolicy, Favorite, ReceiverConfig, ServiceEvent};

mod destination;
mod listener;
mod tls;

pub(crate) use listener::{port_already_released, spawn_server, Peer, PortReleased};
pub(crate) use tls::server_config;

/// 未信任设备 prepare-upload 挂起等待用户确认的超时（design：30s 后 403）。
pub(crate) const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
/// 进度事件的节流步长：避免逐 chunk 事件淹没 broadcast 通道。
const PROGRESS_STEP: u64 = 1024 * 1024;

/// 单个接收文件的会话内状态。
#[derive(Debug)]
struct FileSlot {
    meta: FileMetadata,
    token: String,
    /// 实际落盘路径（首块写入前确定，重名规则在此时应用）。
    dest: Option<PathBuf>,
    received: u64,
    /// 已经开始过几次上传：校验不符时据此决定还让不让重传。
    attempts: u8,
    outcome: SlotOutcome,
}

/// 同一文件最多收几次（同官方 `MAX_UPLOAD_ATTEMPTS`）：校验不符的前几次回 422 让对方
/// 重发，到次数判失败。
const MAX_UPLOAD_ATTEMPTS: u8 = 3;

/// 文件槽终态：成功、失败（原因随 `ReceiveFinished` 上报）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotOutcome {
    Pending,
    Done,
    Failed(&'static str),
}

#[derive(Debug)]
struct Session {
    /// 发起 prepare-upload 的证书指纹：upload、cancel 必须来自同一张证书。
    sender: String,
    files: HashMap<String, FileSlot>,
    /// 这次发送里每个顶层文件夹实际落到的目录（同名的已改名）。
    folders: HashMap<String, PathBuf>,
    /// 会话被结束（对方取消或本机取消）时触发：正在写的上传立刻停下。
    ended: CancellationToken,
    finished_emitted: bool,
}

/// receive 服务共享状态。
pub(crate) struct ReceiveState {
    own_device: DeviceInfo,
    download_dir: PathBuf,
    accept: AcceptPolicy,
    pin: Option<String>,
    /// 每个 IP 连续输错 PIN 的次数；接收端换代（重启、改 PIN）即清零。
    pin_failures: Mutex<HashMap<IpAddr, u32>>,
    /// 收藏设备算信任；app-ui 改收藏时立刻换掉快照。
    favorites: watch::Receiver<Vec<Favorite>>,
    /// 不是收藏的设备的挂起确认：session_id → 决策回写端。
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    sessions: Mutex<HashMap<String, Session>>,
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
            download_dir: receiver.download_dir,
            accept: receiver.accept,
            pin: receiver.pin,
            pin_failures: Mutex::new(HashMap::new()),
            favorites,
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
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

/// 创建会话并为每个 fileId 生成上传令牌（放行的唯一路径），告诉界面谁发来了什么：
/// 自动收下的会话界面没见过确认弹窗，也要能显示进度。
async fn accept_session(
    state: &Arc<ReceiveState>,
    session_id: &str,
    sender: &DeviceInfo,
    files: &HashMap<String, FileMetadata>,
) -> PrepareUploadResponse {
    let mut tokens = HashMap::new();
    let mut slots = HashMap::new();
    for (file_id, meta) in files {
        let token = random_id();
        tokens.insert(file_id.clone(), token.clone());
        slots.insert(
            file_id.clone(),
            FileSlot {
                meta: meta.clone(),
                token,
                dest: None,
                received: 0,
                attempts: 0,
                outcome: SlotOutcome::Pending,
            },
        );
    }
    state.sessions.lock().await.insert(
        session_id.to_string(),
        Session {
            sender: sender.fingerprint.clone(),
            files: slots,
            folders: HashMap::new(),
            ended: CancellationToken::new(),
            finished_emitted: false,
        },
    );
    let _ = state.events.send(ServiceEvent::RequestAccepted {
        session_id: session_id.to_owned(),
        device: sender.clone(),
        files: files.values().cloned().collect(),
    });
    PrepareUploadResponse {
        session_id: session_id.to_string(),
        files: tokens,
    }
}

#[derive(serde::Deserialize)]
struct UploadQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fileId")]
    file_id: String,
    token: String,
}

async fn upload(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> impl IntoResponse {
    let (meta, attempts, ended) = {
        let mut sessions = state.sessions.lock().await;
        // 别的证书拿着令牌也不行；与未知会话同一回应，不告诉探测者会话存在。
        let Some(session) = sessions
            .get_mut(&query.session_id)
            .filter(|session| session.sender == peer.fingerprint)
        else {
            return (StatusCode::FORBIDDEN, "unknown session".to_string());
        };
        let Some(slot) = session.files.get_mut(&query.file_id) else {
            return (StatusCode::FORBIDDEN, "unknown fileId".to_string());
        };
        if slot.token != query.token {
            return (StatusCode::FORBIDDEN, "invalid token".to_string());
        }
        match slot.outcome {
            SlotOutcome::Pending => {
                slot.attempts = slot.attempts.saturating_add(1);
                (slot.meta.clone(), slot.attempts, session.ended.clone())
            }
            // 重复上传同一 fileId：令牌有效但槽位已终态，拒绝重放。
            _ => return (StatusCode::CONFLICT, "file already finalized".to_string()),
        }
    };

    let Some(segments) = destination::file_name_segments(&meta.file_name) else {
        mark_slot_failed(
            &state,
            &query.session_id,
            &query.file_id,
            "invalid file name",
        )
        .await;
        finish_if_complete(&state, &query.session_id).await;
        return (StatusCode::BAD_REQUEST, "invalid file name".to_string());
    };

    // 首块写入前才确定落盘路径：create_new 保证重名探测与占位原子完成。在会话锁内定，
    // 同一次发送的同一个文件夹只改一次名。必须登记：重传要写回同一个文件，否则会另起
    // 一个名字、留下半成品。
    let resolved = {
        let mut sessions = state.sessions.lock().await;
        match sessions.get_mut(&query.session_id) {
            None => return (StatusCode::FORBIDDEN, "unknown session".to_string()),
            Some(session) => match session
                .files
                .get(&query.file_id)
                .and_then(|slot| slot.dest.clone())
            {
                Some(dest) => Ok(dest),
                None => {
                    let resolved = destination::resolve_destination(
                        &state.download_dir,
                        &mut session.folders,
                        &segments,
                    )
                    .await;
                    if let (Ok(dest), Some(slot)) =
                        (&resolved, session.files.get_mut(&query.file_id))
                    {
                        slot.dest = Some(dest.clone());
                    }
                    resolved
                }
            },
        }
    };
    let dest = match resolved {
        Ok(dest) => dest,
        Err(error) => {
            mark_slot_failed(
                &state,
                &query.session_id,
                &query.file_id,
                "cannot create file",
            )
            .await;
            finish_if_complete(&state, &query.session_id).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot create file: {error}"),
            );
        }
    };

    match stream_to_file(&state, &query, body, &dest, &meta, attempts, &ended).await {
        Ok(()) => (StatusCode::OK, "ok".to_string()),
        Err(error) => (error.0, error.1),
    }
}

type UploadFailure = (StatusCode, String);

/// 流式写盘 + 可选 sha256 校验 + 节流进度事件。
async fn stream_to_file(
    state: &Arc<ReceiveState>,
    query: &UploadQuery,
    body: Body,
    dest: &PathBuf,
    meta: &FileMetadata,
    attempts: u8,
    ended: &CancellationToken,
) -> Result<(), UploadFailure> {
    // truncate：dest 由 unique_destination 占位/上次中断重传时，从头重写。
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(dest)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("open dest failed: {error}"),
            )
        })?;
    let mut hasher = meta.sha256.as_ref().map(|_| Sha256::new());
    let mut received = 0u64;
    let mut last_emitted = received;
    // futures-util StreamExt：axum 的 BodyDataStream 走 futures Stream 接口。
    use futures_util::StreamExt;
    let mut body = body.into_data_stream();
    loop {
        // 会话被结束：半成品由 `end_session` 删掉，这里只停下。
        let chunk = tokio::select! {
            chunk = body.next() => chunk,
            () = ended.cancelled() => return Err(session_ended()),
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("read body failed: {error}"),
            )
        })?;
        if let Err(error) = file.write_all(&chunk).await {
            return Err(fail_upload(
                state,
                query,
                dest,
                "write failed",
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write failed: {error}"),
            )
            .await);
        }
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&chunk);
        }
        received += chunk.len() as u64;
        if received - last_emitted >= PROGRESS_STEP {
            last_emitted = received;
            emit_progress(state, query, received);
        }
    }
    // 写入经 tokio 的后台缓冲，磁盘错误可能到这里才冒出来；没落盘就不能报接收成功。
    if let Err(error) = file.sync_data().await {
        return Err(fail_upload(
            state,
            query,
            dest,
            "write failed",
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write failed: {error}"),
        )
        .await);
    }

    // 协议 v2.2：声明了 sha256 但不匹配 → 422。还没到次数：槽退回待收、留着占位文件
    // （下次 truncate 重写，名字不被别人占走），让对方重发；到次数删半成品、判失败。
    if let (Some(expected), Some(hasher)) = (&meta.sha256, hasher) {
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            if attempts < MAX_UPLOAD_ATTEMPTS {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "checksum mismatch".to_string(),
                ));
            }
            return Err(fail_upload(
                state,
                query,
                dest,
                "checksum mismatch",
                StatusCode::UNPROCESSABLE_ENTITY,
                "checksum mismatch".to_string(),
            )
            .await);
        }
    }

    if meta.size != received {
        return Err(fail_upload(
            state,
            query,
            dest,
            "size mismatch",
            StatusCode::BAD_REQUEST,
            format!("size mismatch: expected {} bytes", meta.size),
        )
        .await);
    }

    {
        // 收完之前会话被结束了：文件已被当成半成品删掉，不能报成功。
        let mut sessions = state.sessions.lock().await;
        let Some(slot) = sessions
            .get_mut(&query.session_id)
            .and_then(|s| s.files.get_mut(&query.file_id))
        else {
            return Err(session_ended());
        };
        slot.outcome = SlotOutcome::Done;
        slot.received = received;
    }
    emit_progress(state, query, received);
    finish_if_complete(state, &query.session_id).await;
    Ok(())
}

fn session_ended() -> UploadFailure {
    (StatusCode::CONFLICT, "the session has ended".to_string())
}

fn emit_progress(state: &Arc<ReceiveState>, query: &UploadQuery, received: u64) {
    let _ = state.events.send(ServiceEvent::ReceiveProgress {
        session_id: query.session_id.clone(),
        file_id: query.file_id.clone(),
        received,
    });
}

/// 这个文件收不成了：删掉半成品、把文件槽标成失败、会话到终态就收尾，返回给发送方的错误。
async fn fail_upload(
    state: &Arc<ReceiveState>,
    query: &UploadQuery,
    dest: &Path,
    reason: &'static str,
    status: StatusCode,
    message: String,
) -> UploadFailure {
    if let Err(error) = tokio::fs::remove_file(dest).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                event = "localsend_partial_file_not_removed",
                path = %dest.display(),
                %error,
                "could not remove a partially received file"
            );
        }
    }
    mark_slot_failed(state, &query.session_id, &query.file_id, reason).await;
    finish_if_complete(state, &query.session_id).await;
    (status, message)
}

async fn mark_slot_failed(
    state: &Arc<ReceiveState>,
    session_id: &str,
    file_id: &str,
    reason: &'static str,
) {
    let mut sessions = state.sessions.lock().await;
    if let Some(slot) = sessions
        .get_mut(session_id)
        .and_then(|s| s.files.get_mut(file_id))
    {
        slot.outcome = SlotOutcome::Failed(reason);
    }
}

/// 会话所有文件槽到达终态后：发 `ReceiveFinished` 并移除会话（只发一次）。
async fn finish_if_complete(state: &Arc<ReceiveState>, session_id: &str) {
    let snapshot = {
        let mut sessions = state.sessions.lock().await;
        let Some(session) = sessions.get_mut(session_id) else {
            return;
        };
        if session.finished_emitted
            || !session
                .files
                .values()
                .all(|slot| slot.outcome != SlotOutcome::Pending)
        {
            return;
        }
        session.finished_emitted = true;
        session
            .files
            .values()
            .map(|slot| {
                (
                    slot.dest.clone(),
                    slot.meta.file_name.clone(),
                    slot.outcome.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut saved_paths = Vec::new();
    let mut failed = Vec::new();
    for (dest, file_name, outcome) in snapshot {
        match (outcome, dest) {
            (SlotOutcome::Done, Some(dest)) => saved_paths.push(dest),
            (SlotOutcome::Failed(reason), _) => failed.push((file_name, reason.to_string())),
            _ => failed.push((file_name, "incomplete".to_string())),
        }
    }
    state.sessions.lock().await.remove(session_id);
    let _ = state.events.send(ServiceEvent::ReceiveFinished {
        session_id: session_id.to_string(),
        saved_paths,
        failed,
    });
}

/// 对方取消：只有发起这次会话的那张证书能取消。
async fn cancel(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Query(query): Query<CancelQuery>,
) -> impl IntoResponse {
    let owned = state
        .sessions
        .lock()
        .await
        .get(&query.session_id)
        .is_some_and(|session| session.sender == peer.fingerprint);
    if !owned {
        return StatusCode::FORBIDDEN;
    }
    state
        .end_session(&query.session_id, "canceled by sender")
        .await;
    StatusCode::OK
}

impl ReceiveState {
    /// 结束一次会话（对方取消、本机取消共用）：正在写的上传停下，没收完的标失败并删掉
    /// 半成品，收完的留着；发 `ReceiveFinished`。会话不在了返回 false。
    pub(crate) async fn end_session(&self, session_id: &str, reason: &'static str) -> bool {
        let Some(session) = self.sessions.lock().await.remove(session_id) else {
            return false;
        };
        session.ended.cancel();
        // 已经在收尾（`finish_if_complete` 正在报结果）：不重复报。
        if session.finished_emitted {
            return true;
        }
        let mut saved_paths = Vec::new();
        let mut failed = Vec::new();
        for slot in session.files.into_values() {
            match (slot.outcome, slot.dest) {
                (SlotOutcome::Done, Some(dest)) => saved_paths.push(dest),
                (outcome, dest) => {
                    if let Some(dest) = dest {
                        let _ = tokio::fs::remove_file(&dest).await;
                    }
                    let reason = match outcome {
                        SlotOutcome::Failed(earlier) => earlier,
                        _ => reason,
                    };
                    failed.push((slot.meta.file_name, reason.to_string()));
                }
            }
        }
        let _ = self.events.send(ServiceEvent::ReceiveFinished {
            session_id: session_id.to_string(),
            saved_paths,
            failed,
        });
        true
    }
}

#[derive(serde::Deserialize)]
struct CancelQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[cfg(test)]
mod tests;
