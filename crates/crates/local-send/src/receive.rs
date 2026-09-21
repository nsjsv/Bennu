//! LocalSend 接收端常驻 HTTP 服务（TCP 53317）。
//!
//! 端点（前缀 /api/localsend/v2）：`register`、`prepare-upload`、`upload`、`cancel`。
//! 未信任设备的 prepare-upload 挂起等待 UI 确认（oneshot 回写），超时 30s 回 403，
//! 与 LocalSend 官方 App 行为一致。

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, oneshot, Mutex};

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::ServiceEvent;

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
    outcome: SlotOutcome,
}

/// 文件槽终态：成功、失败（原因随 `ReceiveFinished` 上报）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotOutcome {
    Pending,
    Done,
    Failed(&'static str),
}

#[derive(Debug)]
struct Session {
    files: HashMap<String, FileSlot>,
    finished_emitted: bool,
}

/// receive 服务共享状态。
pub(crate) struct ReceiveState {
    own_device: DeviceInfo,
    download_dir: PathBuf,
    is_trusted: Arc<dyn Fn(&DeviceInfo) -> bool + Send + Sync>,
    /// `accept_session(id, true)` 写入的内存补充信任集（覆盖 UI 持久化完成前的窗口期）。
    supplemental_trusted: Mutex<HashSet<String>>,
    /// 未信任设备的挂起确认：session_id → 决策回写端 + 设备指纹（remember 时补信任）。
    pending: Mutex<HashMap<String, (oneshot::Sender<bool>, String)>>,
    sessions: Mutex<HashMap<String, Session>>,
    devices: Arc<Mutex<crate::discovery::DeviceTable>>,
    events: broadcast::Sender<ServiceEvent>,
    confirm_timeout: Duration,
}

impl ReceiveState {
    pub(crate) fn new(
        own_device: DeviceInfo,
        download_dir: PathBuf,
        is_trusted: Arc<dyn Fn(&DeviceInfo) -> bool + Send + Sync>,
        devices: Arc<Mutex<crate::discovery::DeviceTable>>,
        events: broadcast::Sender<ServiceEvent>,
        confirm_timeout: Duration,
    ) -> Self {
        ReceiveState {
            own_device,
            download_dir,
            is_trusted,
            supplemental_trusted: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            devices,
            events,
            confirm_timeout,
        }
    }

    /// UI 确认回写：accept=true 放行挂起的 prepare-upload；返回 false 表示会话已不存在。
    pub(crate) async fn resolve_pending(
        &self,
        session_id: &str,
        accept: bool,
        remember: bool,
    ) -> bool {
        let sender = self.pending.lock().await.remove(session_id);
        let Some((sender, fingerprint)) = sender else {
            // 会话不存在（已超时/已完成），让 UI 感知决策落空。
            return false;
        };
        if accept && remember {
            // remember 的持久化归 app-ui；这里只做内存补充信任，覆盖持久化完成前的窗口期。
            self.supplemental_trusted.lock().await.insert(fingerprint);
        }
        sender.send(accept).is_ok()
    }
}

pub(crate) fn router(state: Arc<ReceiveState>) -> Router {
    Router::new()
        .route("/api/localsend/v2/register", post(register))
        .route("/api/localsend/v2/prepare-upload", post(prepare_upload))
        .route("/api/localsend/v2/upload", post(upload))
        .route("/api/localsend/v2/cancel", post(cancel))
        .with_state(state)
}

async fn register(
    State(state): State<Arc<ReceiveState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(device): Json<DeviceInfo>,
) -> impl IntoResponse {
    // 对端回应我们的 announce：登记进设备表并广播 DeviceSeen（双向发现的另一半）。
    let changed = state
        .devices
        .lock()
        .await
        .upsert(device, peer.ip(), std::time::Instant::now());
    if let Some(device) = changed {
        let _ = state.events.send(ServiceEvent::DeviceSeen(device));
    }
    (StatusCode::OK, Json(state.own_device.clone()))
}

/// 清洗网络传来的文件名：只保留末段路径组件，拒绝空名与目录穿越。
/// 这是信任边界：fileName 来自任意同网设备，直接拼接会写出下载目录之外。
fn sanitize_file_name(raw: &str) -> Option<String> {
    let name = raw.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or("");
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name.to_string())
}

/// 生成接收端会话 id / 上传令牌（随机 16 字节 hex，见 lib::random_id）。
fn random_id() -> String {
    crate::random_id()
}

/// 判断设备是否放行：UI 注入的信任回调 || accept_session(remember=true) 的补充信任。
async fn device_is_trusted(state: &Arc<ReceiveState>, device: &DeviceInfo) -> bool {
    if (state.is_trusted)(device) {
        return true;
    }
    state
        .supplemental_trusted
        .lock()
        .await
        .contains(&device.fingerprint)
}

async fn prepare_upload(
    State(state): State<Arc<ReceiveState>>,
    Json(request): Json<PrepareUploadRequest>,
) -> impl IntoResponse {
    if request.files.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(Option::<PrepareUploadResponse>::None),
        );
    }

    let session_id = random_id();
    if device_is_trusted(&state, &request.info).await {
        let response = accept_session(&state, &session_id, &request.files).await;
        let _ = state.events.send(ServiceEvent::RequestAccepted {
            session_id: session_id.clone(),
        });
        return (StatusCode::CREATED, Json(Some(response)));
    }

    // 未信任：挂起等 UI 确认，超时回 403（协议语义：403 = 拒绝）。
    let (tx, rx) = oneshot::channel();
    state
        .pending
        .lock()
        .await
        .insert(session_id.clone(), (tx, request.info.fingerprint.clone()));
    let _ = state.events.send(ServiceEvent::IncomingRequest {
        device: request.info.clone(),
        session_id: session_id.clone(),
        files: request.files.values().cloned().collect(),
    });
    let accepted = tokio::time::timeout(state.confirm_timeout, rx).await;
    match accepted {
        Ok(Ok(true)) => {
            let response = accept_session(&state, &session_id, &request.files).await;
            let _ = state
                .events
                .send(ServiceEvent::RequestAccepted { session_id });
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

/// 创建会话并为每个 fileId 生成上传令牌（信任放行的唯一路径）。
async fn accept_session(
    state: &Arc<ReceiveState>,
    session_id: &str,
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
                outcome: SlotOutcome::Pending,
            },
        );
    }
    state.sessions.lock().await.insert(
        session_id.to_string(),
        Session {
            files: slots,
            finished_emitted: false,
        },
    );
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
    Query(query): Query<UploadQuery>,
    body: Body,
) -> impl IntoResponse {
    let (meta, _) = {
        let sessions = state.sessions.lock().await;
        let Some(session) = sessions.get(&query.session_id) else {
            return (StatusCode::FORBIDDEN, "unknown session".to_string());
        };
        let Some(slot) = session.files.get(&query.file_id) else {
            return (StatusCode::FORBIDDEN, "unknown fileId".to_string());
        };
        if slot.token != query.token {
            return (StatusCode::FORBIDDEN, "invalid token".to_string());
        }
        match slot.outcome {
            SlotOutcome::Pending => (slot.meta.clone(), slot.token.clone()),
            // 重复上传同一 fileId：令牌有效但槽位已终态，拒绝重放。
            _ => return (StatusCode::CONFLICT, "file already finalized".to_string()),
        }
    };

    let Some(file_name) = sanitize_file_name(&meta.file_name) else {
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

    // 首块写入前才确定落盘路径：create_new 保证重名探测与占位原子完成。
    let dest = {
        let sessions = state.sessions.lock().await;
        sessions
            .get(&query.session_id)
            .and_then(|s| s.files.get(&query.file_id))
            .and_then(|slot| slot.dest.clone())
    };
    let dest = match dest {
        Some(dest) => dest,
        None => match unique_destination(&state.download_dir, &file_name).await {
            Ok(dest) => {
                if let Ok(mut sessions) = state.sessions.try_lock() {
                    if let Some(slot) = sessions
                        .get_mut(&query.session_id)
                        .and_then(|s| s.files.get_mut(&query.file_id))
                    {
                        slot.dest = Some(dest.clone());
                    }
                }
                dest
            }
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
        },
    };

    match stream_to_file(&state, &query, body, &dest, &meta).await {
        Ok(()) => (StatusCode::OK, "ok".to_string()),
        Err(error) => (error.0, error.1),
    }
}

/// 重名规则：`name.ext` 已存在则 `name (1).ext`、`name (2).ext` 依次探测（R8）。
/// create_new 保证并发会话写入同名文件时不会互相覆盖。
async fn unique_destination(dir: &PathBuf, file_name: &str) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 0..10_000u32 {
        let candidate = match (index, extension) {
            (0, _) => dir.join(file_name),
            (n, Some(ext)) => dir.join(format!("{stem} ({n}).{ext}")),
            (n, None) => dir.join(format!("{stem} ({n})")),
        };
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many name collisions",
    ))
}

type UploadFailure = (StatusCode, String);

/// 流式写盘 + 可选 sha256 校验 + 节流进度事件。
async fn stream_to_file(
    state: &Arc<ReceiveState>,
    query: &UploadQuery,
    body: Body,
    dest: &PathBuf,
    meta: &FileMetadata,
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
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("read body failed: {error}"),
            )
        })?;
        file.write_all(&chunk).await.map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write failed: {error}"),
            )
        })?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&chunk);
        }
        received += chunk.len() as u64;
        if received - last_emitted >= PROGRESS_STEP {
            last_emitted = received;
            emit_progress(state, query, &meta.file_name, received, meta.size);
        }
    }
    let _ = file.sync_data().await;

    // 协议 v2.2：声明了 sha256 但不匹配 → 422，删除半成品。
    if let (Some(expected), Some(hasher)) = (&meta.sha256, hasher) {
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(dest).await;
            mark_slot_failed(
                state,
                &query.session_id,
                &query.file_id,
                "checksum mismatch",
            )
            .await;
            finish_if_complete(state, &query.session_id).await;
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "checksum mismatch".to_string(),
            ));
        }
    }

    if meta.size != received {
        let _ = tokio::fs::remove_file(dest).await;
        mark_slot_failed(state, &query.session_id, &query.file_id, "size mismatch").await;
        finish_if_complete(state, &query.session_id).await;
        return Err((
            StatusCode::BAD_REQUEST,
            format!("size mismatch: expected {} bytes", meta.size),
        ));
    }

    {
        let mut sessions = state.sessions.lock().await;
        if let Some(slot) = sessions
            .get_mut(&query.session_id)
            .and_then(|s| s.files.get_mut(&query.file_id))
        {
            slot.outcome = SlotOutcome::Done;
            slot.received = received;
        }
    }
    emit_progress(state, query, &meta.file_name, received, meta.size);
    finish_if_complete(state, &query.session_id).await;
    Ok(())
}

fn emit_progress(
    state: &Arc<ReceiveState>,
    query: &UploadQuery,
    file_name: &str,
    received: u64,
    total: u64,
) {
    let _ = state.events.send(ServiceEvent::ReceiveProgress {
        session_id: query.session_id.clone(),
        file_name: file_name.to_string(),
        received,
        total,
    });
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

async fn cancel(
    State(state): State<Arc<ReceiveState>>,
    Query(query): Query<CancelQuery>,
) -> impl IntoResponse {
    let mut snapshot: Vec<(String, Option<PathBuf>, SlotOutcome)> = Vec::new();
    {
        let mut sessions = state.sessions.lock().await;
        let Some(session) = sessions.get_mut(&query.session_id) else {
            return StatusCode::FORBIDDEN;
        };
        for slot in session.files.values_mut() {
            if slot.outcome == SlotOutcome::Pending {
                // 发送方取消：未完成槽位标记失败并删除半成品。
                slot.outcome = SlotOutcome::Failed("canceled by sender");
            }
            snapshot.push((
                slot.meta.file_name.clone(),
                slot.dest.clone(),
                slot.outcome.clone(),
            ));
        }
        session.finished_emitted = true;
    }
    let mut saved_paths = Vec::new();
    let mut failed = Vec::new();
    for (file_name, dest, outcome) in snapshot {
        match outcome {
            SlotOutcome::Done => {
                if let Some(dest) = dest {
                    saved_paths.push(dest);
                }
            }
            SlotOutcome::Failed(reason) => {
                // cancel 的清理语义：未完成的落盘文件是半成品，必须删除。
                if let Some(dest) = dest {
                    let _ = tokio::fs::remove_file(&dest).await;
                }
                failed.push((file_name, reason.to_string()));
            }
            SlotOutcome::Pending => unreachable!("上面已全部置为终态"),
        }
    }
    state.sessions.lock().await.remove(&query.session_id);
    let _ = state.events.send(ServiceEvent::ReceiveFinished {
        session_id: query.session_id.clone(),
        saved_paths,
        failed,
    });
    StatusCode::OK
}

#[derive(serde::Deserialize)]
struct CancelQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}
