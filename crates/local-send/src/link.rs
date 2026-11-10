//! 通过链接接收：没装 LocalSend 的手机扫码打开网页，把文件传到这台电脑（官方 1.18
//! "Receive via link" 的网页一侧）。
//!
//! 与扫码下载一样是临时明文 HTTP 服务：随机端口，链接里的随机码是唯一的门，不用确认、
//! 不要 PIN，接收开关关着也能用。放行之后的会话与常驻接收端共用 `ReceiveSessions`；
//! 网页没有证书，发送方按连接来源 IP 认。
//!
//! 停下（`LinkControl::stop`）后不再接新的，开着的会话照常收完，全部结束才关服务；
//! 10 分钟没动静就结束剩下的会话并关服务。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::JoinHandle;

use crate::device::FileMetadata;
use crate::error::LocalSendError;
use crate::receive::{CancelQuery, ReceiveReport, ReceiveSessions, SessionEvents, UploadQuery};

mod page;

/// 进度按每 MiB 一条，几部手机同时发也够界面跟上。
const EVENT_CAPACITY: usize = 256;

pub struct LinkReceiveConfig {
    /// 网页标题里的电脑名。
    pub computer_name: String,
    pub download_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LinkEvent {
    /// 一部手机开始发：从哪个地址、哪些文件。
    Accepted {
        session_id: String,
        address: IpAddr,
        files: Vec<FileMetadata>,
    },
    /// 某个文件已收到的字节（同 `ServiceEvent::ReceiveProgress`）。
    Progress {
        session_id: String,
        file_id: String,
        received: u64,
    },
    Finished {
        session_id: String,
        report: ReceiveReport,
    },
    /// 10 分钟没动静：剩下的会话已结束，服务已关。
    TimedOut,
    /// 停下之后开着的会话都结束了，服务已关。
    Closed,
}

#[derive(Debug)]
enum LinkCommand {
    Stop,
    Cancel(String),
}

/// 停下服务、取消一次接收；可以克隆给每个接收任务，服务关了再调用什么也不做。
#[derive(Debug, Clone)]
pub struct LinkControl {
    commands: mpsc::UnboundedSender<LinkCommand>,
}

impl LinkControl {
    /// 不再接新的；开着的会话收完再关服务（发 `Closed`）。
    pub fn stop(&self) {
        let _ = self.commands.send(LinkCommand::Stop);
    }

    /// 本机结束一次接收：没收完的算取消，网页上提示电脑取消了。
    pub fn cancel_session(&self, session_id: &str) {
        let _ = self
            .commands
            .send(LinkCommand::Cancel(session_id.to_owned()));
    }
}

struct LinkShared {
    computer_name: String,
    /// `/<随机码>`：网页里的请求都发到它下面。
    base_path: String,
    sessions: ReceiveSessions,
    events: broadcast::Sender<LinkEvent>,
    /// 还接不接新的。开会话时拿着读锁，停下时拿写锁：停下之后不会再冒出新会话，
    /// 排空检查不会漏掉正在开的那个。
    accepting: RwLock<bool>,
    last_activity: parking_lot::Mutex<Instant>,
}

impl LinkShared {
    fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }
}

pub struct LinkReceiveHandle {
    urls: Vec<String>,
    events: broadcast::Sender<LinkEvent>,
    control: LinkControl,
    server: JoinHandle<()>,
    monitor: JoinHandle<()>,
}

impl LinkReceiveHandle {
    /// 每个局域网候选地址一条链接（私网优先），二维码按用户选的那条生成。
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn events(&self) -> broadcast::Receiver<LinkEvent> {
        self.events.subscribe()
    }

    pub fn control(&self) -> LinkControl {
        self.control.clone()
    }
}

impl Drop for LinkReceiveHandle {
    fn drop(&mut self) {
        self.server.abort();
        self.monitor.abort();
    }
}

pub struct LinkReceiveServer;

impl LinkReceiveServer {
    /// 启动（10 分钟没动静自动停，同扫码下载）。
    pub async fn start(config: LinkReceiveConfig) -> Result<LinkReceiveHandle, LocalSendError> {
        Self::start_with_idle_timeout(config, crate::qr::DEFAULT_IDLE_TIMEOUT).await
    }

    pub async fn start_with_idle_timeout(
        config: LinkReceiveConfig,
        idle_timeout: Duration,
    ) -> Result<LinkReceiveHandle, LocalSendError> {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
            .await
            .map_err(|source| LocalSendError::Io { path: None, source })?;
        let port = listener
            .local_addr()
            .map_err(|source| LocalSendError::Io { path: None, source })?
            .port();
        let lan_ips = crate::discovery::lan_candidates()?;
        if lan_ips.is_empty() {
            return Err(LocalSendError::NoLocalInterface(
                "no usable LAN address".to_owned(),
            ));
        }
        let base_path = format!("/{}", crate::random_id());
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let shared = Arc::new(LinkShared {
            computer_name: config.computer_name,
            base_path: base_path.clone(),
            sessions: ReceiveSessions::new(
                config.download_dir,
                SessionEvents::Link(events.clone()),
            ),
            events: events.clone(),
            accepting: RwLock::new(true),
            last_activity: parking_lot::Mutex::new(Instant::now()),
        });
        let router = router(Arc::clone(&shared));
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        let (commands_tx, commands) = mpsc::unbounded_channel();
        let monitor = tokio::spawn(run_monitor(
            Arc::clone(&shared),
            commands,
            idle_timeout,
            server.abort_handle(),
        ));
        Ok(LinkReceiveHandle {
            urls: lan_ips
                .into_iter()
                .map(|(_, ip)| format!("http://{ip}:{port}{base_path}"))
                .collect(),
            events,
            control: LinkControl {
                commands: commands_tx,
            },
            server,
            monitor,
        })
    }
}

/// 停下与空闲都在这里收尾：停下后会话排空才关服务；空闲到点先结束剩下的会话（界面
/// 照常拿到每次的结果）再关。
async fn run_monitor(
    shared: Arc<LinkShared>,
    mut commands: mpsc::UnboundedReceiver<LinkCommand>,
    idle_timeout: Duration,
    server: tokio::task::AbortHandle,
) {
    let mut events = shared.events.subscribe();
    loop {
        if !*shared.accepting.read().await && shared.sessions.is_empty().await {
            server.abort();
            let _ = shared.events.send(LinkEvent::Closed);
            return;
        }
        let idle_deadline = *shared.last_activity.lock() + idle_timeout;
        tokio::select! {
            command = commands.recv() => match command {
                Some(LinkCommand::Stop) => *shared.accepting.write().await = false,
                Some(LinkCommand::Cancel(session_id)) => {
                    shared.sessions.end_session(&session_id).await;
                }
                // 句柄和所有控制端都没了：没人再看结果。
                None => {
                    server.abort();
                    return;
                }
            },
            // 会话的开始、进度、结束都算动静；结束之后回到上面看是否排空。
            _ = events.recv() => shared.touch(),
            () = tokio::time::sleep_until(idle_deadline.into()) => {
                if shared.last_activity.lock().elapsed() >= idle_timeout {
                    *shared.accepting.write().await = false;
                    shared.sessions.end_all().await;
                    server.abort();
                    let _ = shared.events.send(LinkEvent::TimedOut);
                    return;
                }
            }
        }
    }
}

/// 全部挂在随机码下面：别的路径一律 404，不告诉探测者这里有服务。
fn router(shared: Arc<LinkShared>) -> Router {
    let base_path = shared.base_path.clone();
    Router::new().nest(
        &base_path,
        Router::new()
            .route("/", get(upload_page))
            .route("/prepare-upload", post(prepare_upload))
            .route("/upload", post(upload))
            .route("/cancel", post(cancel))
            .with_state(shared),
    )
}

async fn upload_page(State(shared): State<Arc<LinkShared>>) -> Response {
    shared.touch();
    if !*shared.accepting.read().await {
        return (StatusCode::GONE, Html(page::stopped(&shared.computer_name))).into_response();
    }
    Html(page::upload(&shared.computer_name, &shared.base_path)).into_response()
}

/// 网页发来的这次要传哪些文件（不像 LocalSend 设备那样带设备信息：网页没有身份）。
#[derive(serde::Deserialize)]
struct LinkUploadRequest {
    files: HashMap<String, FileMetadata>,
}

async fn prepare_upload(
    State(shared): State<Arc<LinkShared>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<LinkUploadRequest>,
) -> Response {
    shared.touch();
    let accepting = shared.accepting.read().await;
    if !*accepting {
        return StatusCode::GONE.into_response();
    }
    if request.files.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session_id = crate::random_id();
    let response = shared
        .sessions
        .open(&session_id, &address.ip().to_string(), &request.files)
        .await;
    // 先于回应：网页拿到令牌才会上传，界面先有任务再有进度。
    let _ = shared.events.send(LinkEvent::Accepted {
        session_id,
        address: address.ip(),
        files: request.files.into_values().collect(),
    });
    drop(accepting);
    Json(response).into_response()
}

async fn upload(
    State(shared): State<Arc<LinkShared>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> impl IntoResponse {
    shared.touch();
    shared
        .sessions
        .upload(&address.ip().to_string(), &query, body)
        .await
}

/// 网页上一个文件没发成、或离开页面（`sendBeacon`）时结束这次发送。
async fn cancel(
    State(shared): State<Arc<LinkShared>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Query(query): Query<CancelQuery>,
) -> StatusCode {
    shared.touch();
    shared
        .sessions
        .cancel_from(&address.ip().to_string(), &query.session_id)
        .await
}
