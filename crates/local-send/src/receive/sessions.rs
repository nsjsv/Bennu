//! 接收会话：一次接收被放行之后的全部事情——按令牌收文件、落盘与重名、校验不符让对方
//! 重发、进度节流、半成品删除、对方或本机结束会话、全部到终态报结果。
//!
//! 常驻接收端与链接接收共用。谁能开会话（PIN、确认、随机码）归各自的入口，这里只认
//! 开会话时给的 `owner`：upload、cancel 必须来自同一个发送方（常驻接收端是握手证书
//! 指纹，链接接收是连接来源 IP）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use super::destination;
use super::report::{ReceiveReport, ReceivedFile, ReceivedOutcome};
use crate::device::{FileMetadata, PrepareUploadResponse};
use crate::{LinkEvent, ServiceEvent};

/// 进度事件的节流步长：避免逐 chunk 事件淹没 broadcast 通道。
const PROGRESS_STEP: u64 = 1024 * 1024;
/// 同一文件最多收几次（同官方 `MAX_UPLOAD_ATTEMPTS`）：校验不符的前几次回 422 让对方
/// 重发，到次数判失败。
const MAX_UPLOAD_ATTEMPTS: u8 = 3;

/// 单个接收文件的会话内状态。
#[derive(Debug)]
pub(super) struct FileSlot {
    pub(super) meta: FileMetadata,
    token: String,
    /// 实际落盘路径（首块写入前确定，重名规则在此时应用）。
    pub(super) dest: Option<PathBuf>,
    received: u64,
    /// 已经开始过几次上传：校验不符时据此决定还让不让重传。
    attempts: u8,
    pub(super) outcome: SlotOutcome,
}

/// 文件槽终态：成功、失败（原因随结果上报）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SlotOutcome {
    Pending,
    Done,
    Failed(&'static str),
}

#[derive(Debug)]
struct Session {
    owner: String,
    files: HashMap<String, FileSlot>,
    /// 这次发送里每个顶层文件夹实际落到的目录（同名的已改名）。
    folders: HashMap<String, PathBuf>,
    /// 会话被结束（对方取消或本机取消）时触发：正在写的上传立刻停下。
    ended: CancellationToken,
    finished_emitted: bool,
}

/// 会话的进度与结果发到哪：两个入口各有自己的事件通道。
pub(crate) enum SessionEvents {
    Service(broadcast::Sender<ServiceEvent>),
    Link(broadcast::Sender<LinkEvent>),
}

impl SessionEvents {
    fn progress(&self, session_id: &str, file_id: &str, received: u64) {
        let (session_id, file_id) = (session_id.to_owned(), file_id.to_owned());
        match self {
            Self::Service(events) => {
                let _ = events.send(ServiceEvent::ReceiveProgress {
                    session_id,
                    file_id,
                    received,
                });
            }
            Self::Link(events) => {
                let _ = events.send(LinkEvent::Progress {
                    session_id,
                    file_id,
                    received,
                });
            }
        }
    }

    fn finished(&self, session_id: &str, report: ReceiveReport) {
        let session_id = session_id.to_owned();
        match self {
            Self::Service(events) => {
                let _ = events.send(ServiceEvent::ReceiveFinished { session_id, report });
            }
            Self::Link(events) => {
                let _ = events.send(LinkEvent::Finished { session_id, report });
            }
        }
    }
}

pub(crate) struct ReceiveSessions {
    download_dir: PathBuf,
    sessions: Mutex<HashMap<String, Session>>,
    events: SessionEvents,
}

#[derive(serde::Deserialize)]
pub(crate) struct UploadQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fileId")]
    file_id: String,
    token: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct CancelQuery {
    #[serde(rename = "sessionId")]
    pub(crate) session_id: String,
}

type UploadFailure = (StatusCode, String);

impl ReceiveSessions {
    pub(crate) fn new(download_dir: PathBuf, events: SessionEvents) -> Self {
        Self {
            download_dir,
            sessions: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// 开一次会话：每个 fileId 发一个上传令牌（放行的唯一路径）。
    pub(crate) async fn open(
        &self,
        session_id: &str,
        owner: &str,
        files: &HashMap<String, FileMetadata>,
    ) -> PrepareUploadResponse {
        let mut tokens = HashMap::new();
        let mut slots = HashMap::new();
        for (file_id, meta) in files {
            let token = crate::random_id();
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
        self.sessions.lock().await.insert(
            session_id.to_owned(),
            Session {
                owner: owner.to_owned(),
                files: slots,
                folders: HashMap::new(),
                ended: CancellationToken::new(),
                finished_emitted: false,
            },
        );
        PrepareUploadResponse {
            session_id: session_id.to_owned(),
            files: tokens,
        }
    }

    pub(crate) async fn upload(
        &self,
        owner: &str,
        query: &UploadQuery,
        body: Body,
    ) -> (StatusCode, String) {
        let (meta, attempts, ended) = {
            let mut sessions = self.sessions.lock().await;
            // 别的发送方拿着令牌也不行；与未知会话同一回应，不告诉探测者会话存在。
            let Some(session) = sessions
                .get_mut(&query.session_id)
                .filter(|session| session.owner == owner)
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
            self.mark_slot_failed(&query.session_id, &query.file_id, "invalid file name")
                .await;
            self.finish_if_complete(&query.session_id).await;
            return (StatusCode::BAD_REQUEST, "invalid file name".to_string());
        };

        // 首块写入前才确定落盘路径：create_new 保证重名探测与占位原子完成。在会话锁内定，
        // 同一次发送的同一个文件夹只改一次名。必须登记：重传要写回同一个文件，否则会另起
        // 一个名字、留下半成品。
        let resolved = {
            let mut sessions = self.sessions.lock().await;
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
                            &self.download_dir,
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
                self.mark_slot_failed(&query.session_id, &query.file_id, "cannot create file")
                    .await;
                self.finish_if_complete(&query.session_id).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("cannot create file: {error}"),
                );
            }
        };

        match self
            .stream_to_file(query, body, &dest, &meta, attempts, &ended)
            .await
        {
            Ok(()) => (StatusCode::OK, "ok".to_string()),
            Err(error) => (error.0, error.1),
        }
    }

    /// 对方取消：只有开会话的那个发送方能取消。
    pub(crate) async fn cancel_from(&self, owner: &str, session_id: &str) -> StatusCode {
        let owned = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .is_some_and(|session| session.owner == owner);
        if !owned {
            return StatusCode::FORBIDDEN;
        }
        self.end_session(session_id).await;
        StatusCode::OK
    }

    /// 结束一次会话（对方取消、本机取消共用）：正在写的上传停下，没收完的算取消、删掉
    /// 半成品，收完的留着；报结果。会话不在了返回 false。
    pub(crate) async fn end_session(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.lock().await.remove(session_id) else {
            return false;
        };
        session.ended.cancel();
        // 已经在收尾（`finish_if_complete` 正在报结果）：不重复报。
        if session.finished_emitted {
            return true;
        }
        let mut files = Vec::with_capacity(session.files.len());
        for slot in session.files.values() {
            let file = ReceivedFile::from_slot(slot);
            if !matches!(file.outcome, ReceivedOutcome::Saved(_)) {
                if let Some(dest) = &slot.dest {
                    let _ = tokio::fs::remove_file(dest).await;
                }
            }
            files.push(file);
        }
        self.events.finished(session_id, ReceiveReport { files });
        true
    }

    /// 结束全部还开着的会话（链接接收空闲到点）。
    pub(crate) async fn end_all(&self) {
        let session_ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        for session_id in session_ids {
            self.end_session(&session_id).await;
        }
    }

    pub(crate) async fn is_empty(&self) -> bool {
        self.sessions.lock().await.is_empty()
    }

    /// 流式写盘 + 可选 sha256 校验 + 节流进度事件。
    async fn stream_to_file(
        &self,
        query: &UploadQuery,
        body: Body,
        dest: &Path,
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
                return Err(self
                    .fail_upload(
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
                self.events
                    .progress(&query.session_id, &query.file_id, received);
            }
        }
        // 写入经 tokio 的后台缓冲，磁盘错误可能到这里才冒出来；没落盘就不能报接收成功。
        if let Err(error) = file.sync_data().await {
            return Err(self
                .fail_upload(
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
                return Err(self
                    .fail_upload(
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
            return Err(self
                .fail_upload(
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
            let mut sessions = self.sessions.lock().await;
            let Some(slot) = sessions
                .get_mut(&query.session_id)
                .and_then(|s| s.files.get_mut(&query.file_id))
            else {
                return Err(session_ended());
            };
            slot.outcome = SlotOutcome::Done;
            slot.received = received;
        }
        self.events
            .progress(&query.session_id, &query.file_id, received);
        self.finish_if_complete(&query.session_id).await;
        Ok(())
    }

    /// 这个文件收不成了：删掉半成品、把文件槽标成失败、会话到终态就收尾，返回给发送方的错误。
    async fn fail_upload(
        &self,
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
        self.mark_slot_failed(&query.session_id, &query.file_id, reason)
            .await;
        self.finish_if_complete(&query.session_id).await;
        (status, message)
    }

    async fn mark_slot_failed(&self, session_id: &str, file_id: &str, reason: &'static str) {
        let mut sessions = self.sessions.lock().await;
        if let Some(slot) = sessions
            .get_mut(session_id)
            .and_then(|s| s.files.get_mut(file_id))
        {
            slot.outcome = SlotOutcome::Failed(reason);
        }
    }

    /// 会话所有文件槽到达终态后：报结果并移除会话（只报一次）。
    async fn finish_if_complete(&self, session_id: &str) {
        let report = {
            let mut sessions = self.sessions.lock().await;
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
            ReceiveReport {
                files: session
                    .files
                    .values()
                    .map(ReceivedFile::from_slot)
                    .collect(),
            }
        };
        self.sessions.lock().await.remove(session_id);
        self.events.finished(session_id, report);
    }
}

fn session_ended() -> UploadFailure {
    (StatusCode::CONFLICT, "the session has ended".to_string())
}

#[cfg(test)]
mod tests;
