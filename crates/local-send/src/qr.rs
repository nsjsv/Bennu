//! 二维码临时下载服务：一次性 token 鉴权、HTML 列表、单文件流、目录 zip 流式。
//!
//! 服务生命周期：全部条目下载完成 / 10 分钟无活动 / 句柄 drop 或显式 shutdown。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::error::LocalSendError;
use crate::QrEvent;

/// 无活动自动关停（R4：10 分钟）。
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// 进度/生命周期事件通道容量。
const EVENT_CAPACITY: usize = 64;
/// zip 流的进程内缓冲窗口（duplex 管道，提供下载背压）。
const DUPLEX_BUFFER: usize = 64 * 1024;

/// 下载条目来源：单文件或目录（目录按 zip 流式打包，不落临时盘）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrSource {
    File(PathBuf),
    Dir(PathBuf),
}

/// 一次性下载会话的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrItem {
    pub display_name: String,
    pub source: QrSource,
}

/// 列表页/zip 用的元数据（启动时一次探测，请求路径零文件系统统计）。
struct ItemMeta {
    display_name: String,
    source: QrSource,
    size: u64,
    zip_name: String,
}

struct QrShared {
    token: String,
    items: Vec<ItemMeta>,
    events: broadcast::Sender<QrEvent>,
    /// 全部响应累计发送字节。
    bytes_sent: AtomicU64,
    /// 进行中的响应数量（Progress.active 语义）。
    active: AtomicUsize,
    /// 已完整下载过的条目下标（读到 EOF 才算）。
    downloaded: Mutex<HashSet<usize>>,
    last_activity: Mutex<Instant>,
    connected: AtomicBool,
}

impl QrShared {
    fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Connected 只发一次：首个扫码请求即建立连接。
    fn notify_connected(&self) {
        if !self.connected.swap(true, Ordering::Relaxed) {
            let _ = self.events.send(QrEvent::Connected);
        }
    }
}

/// 进行中响应的守卫：离开作用域（发送完成或客户端断开）自动减计数。
struct ActiveGuard {
    shared: Arc<QrShared>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.shared.active.fetch_sub(1, Ordering::Relaxed);
        self.shared.touch();
    }
}

/// 一次性下载服务句柄：`urls` 是各候选网卡地址的下载链接（二维码按
/// 用户选中的地址生成），`events` 驱动窗口进度。
pub struct QrDownloadHandle {
    urls: Vec<String>,
    events: broadcast::Receiver<QrEvent>,
    server: JoinHandle<()>,
    monitor: JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl QrDownloadHandle {
    /// 全部候选地址的下载链接（按私网优先排序，服务本体只有一个端口/token）。
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn events(&self) -> broadcast::Receiver<QrEvent> {
        self.events.resubscribe()
    }

    /// 显式关停（窗口关闭时调用）；Drop 同样关停。
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.server.abort();
        self.monitor.abort();
    }
}

impl Drop for QrDownloadHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// LocalSend 契约入口：启动一次性下载服务（默认 10 分钟无活动关停）。
pub(crate) async fn start(items: Vec<QrItem>) -> Result<QrDownloadHandle, LocalSendError> {
    start_with_idle_timeout(items, DEFAULT_IDLE_TIMEOUT).await
}

/// 测试可注入空闲超时的启动变体。
pub(crate) async fn start_with_idle_timeout(
    items: Vec<QrItem>,
    idle_timeout: Duration,
) -> Result<QrDownloadHandle, LocalSendError> {
    if items.is_empty() {
        return Err(LocalSendError::InvalidPath {
            path: PathBuf::new(),
        });
    }
    let mut metas = Vec::with_capacity(items.len());
    for item in items {
        let size = source_size(&item.source).await?;
        let zip_name = match &item.source {
            QrSource::Dir(dir) => {
                let name = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("download")
                    .to_string();
                format!("{name}.zip")
            }
            QrSource::File(_) => "download.zip".to_string(),
        };
        metas.push(ItemMeta {
            display_name: item.display_name,
            source: item.source,
            size,
            zip_name,
        });
    }

    // 监听随机端口；为每个局域网候选地址各生成一条链接（私网优先），
    // 二维码按用户点选的地址渲染，服务本体只有一份（随机端口 + token）。
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
    let token = crate::random_id();

    let (events_tx, events_rx) = broadcast::channel(EVENT_CAPACITY);
    let shared = Arc::new(QrShared {
        token,
        items: metas,
        events: events_tx,
        bytes_sent: AtomicU64::new(0),
        active: AtomicUsize::new(0),
        downloaded: Mutex::new(HashSet::new()),
        last_activity: Mutex::new(Instant::now()),
        connected: AtomicBool::new(false),
    });

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let router = router(Arc::clone(&shared));
    let server = tokio::spawn(async move {
        // shutdown：立即停监听断连接；临时服务无需优雅排空。
        tokio::select! {
            _ = axum::serve(listener, router) => {}
            _ = shutdown_watch(shutdown_rx) => {}
        }
    });
    let monitor = tokio::spawn(run_monitor(
        Arc::clone(&shared),
        idle_timeout,
        shutdown_tx.clone(),
        server.abort_handle(),
    ));

    Ok(QrDownloadHandle {
        urls: lan_ips
            .into_iter()
            .map(|(_, ip)| format!("http://{ip}:{port}/{}", shared.token))
            .collect(),
        events: events_rx,
        server,
        monitor,
        shutdown_tx,
    })
}

async fn shutdown_watch(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// 监控任务：节流进度事件 + 全部下载完成 → Finished + 空闲超时 → TimedOut；
/// 两个终态都必须真正关停 HTTP 服务（R4：传完/超时后端口不再监听）。
async fn run_monitor(
    shared: Arc<QrShared>,
    idle_timeout: Duration,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    server_abort: tokio::task::AbortHandle,
) {
    let mut last_bytes = 0u64;
    let mut last_active = false;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let bytes = shared.bytes_sent.load(Ordering::Relaxed);
        let active = shared.active.load(Ordering::Relaxed) > 0;
        if bytes != last_bytes || active != last_active {
            last_bytes = bytes;
            last_active = active;
            let _ = shared.events.send(QrEvent::Progress {
                bytes_sent: bytes,
                active,
            });
        }
        {
            let downloaded = shared.downloaded.lock();
            if !downloaded.is_empty() && downloaded.len() == shared.items.len() {
                let _ = shared.events.send(QrEvent::Finished);
                stop_server(&shutdown_tx, &server_abort);
                return;
            }
        }
        if !active {
            let last = *shared.last_activity.lock();
            if last.elapsed() >= idle_timeout {
                let _ = shared.events.send(QrEvent::TimedOut);
                stop_server(&shutdown_tx, &server_abort);
                return;
            }
        }
    }
}

fn stop_server(
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
    server_abort: &tokio::task::AbortHandle,
) {
    let _ = shutdown_tx.send(true);
    server_abort.abort();
}

async fn source_size(source: &QrSource) -> Result<u64, LocalSendError> {
    match source {
        QrSource::File(path) => {
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(path.clone()),
                    source,
                })?;
            if !meta.is_file() {
                return Err(LocalSendError::InvalidPath { path: path.clone() });
            }
            Ok(meta.len())
        }
        QrSource::Dir(path) => {
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(path.clone()),
                    source,
                })?;
            if !meta.is_dir() {
                return Err(LocalSendError::InvalidPath { path: path.clone() });
            }
            Ok(dir_size(path).await)
        }
    }
}

async fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

fn router(shared: Arc<QrShared>) -> Router {
    Router::new()
        .route("/{token}", get(index_page))
        .route("/{token}/file/{index}", get(serve_file))
        .route("/{token}/zip", get(serve_zip))
        .with_state(shared)
}

/// token 是唯一鉴权：不匹配一律 404，不区分路径阶段。
fn check_token(shared: &QrShared, token: &str) -> bool {
    shared.token == token
}

async fn index_page(
    State(shared): State<Arc<QrShared>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    if !check_token(&shared, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    shared.touch();
    shared.notify_connected();
    let mut rows = String::new();
    for (index, item) in shared.items.iter().enumerate() {
        let size_text = format_size(item.size);
        // 目录条目没有单文件链接，指向整包 zip。
        let href = match item.source {
            QrSource::File(_) => format!("/file/{index}"),
            QrSource::Dir(_) => "/zip".to_string(),
        };
        rows.push_str(&format!(
            "<li><a href=\"{href}\">{}</a> <small>{size_text}</small></li>",
            html_escape(&item.display_name),
        ));
    }
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Downloads</title></head><body><h1>Downloads</h1><ul>{rows}</ul>\
         <p><a href=\"/zip\">Download all (zip)</a></p></body></html>"
    );
    Html(body).into_response()
}

async fn serve_file(
    State(shared): State<Arc<QrShared>>,
    AxumPath((token, index)): AxumPath<(String, usize)>,
) -> Response {
    if !check_token(&shared, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(item) = shared.items.get(index) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let QrSource::File(path) = &item.source else {
        // 列表页对目录条目只提供 zip 链接；/file/{i} 指向目录时同样 404。
        return StatusCode::NOT_FOUND.into_response();
    };
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    shared.touch();
    shared.notify_connected();
    shared.active.fetch_add(1, Ordering::Relaxed);

    // CountingReader 读到 EOF 时登记"已下载"；ActiveGuard 在流结束时回收计数。
    let reader = CountingReader {
        inner: file,
        shared: Arc::clone(&shared),
        index,
        done: false,
        guard: ActiveGuard {
            shared: Arc::clone(&shared),
        },
    };
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(reader));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        content_disposition(&item.display_name),
    );
    response
}

async fn serve_zip(
    State(shared): State<Arc<QrShared>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    if !check_token(&shared, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    shared.touch();
    shared.notify_connected();
    shared.active.fetch_add(1, Ordering::Relaxed);

    // zip 流式写进程内管道：async_zip 产出 → duplex → HTTP body，无临时文件。
    let (tx, rx) = tokio::io::duplex(DUPLEX_BUFFER);
    let zip_shared = Arc::clone(&shared);
    let items: Vec<(String, QrSource)> = shared
        .items
        .iter()
        .map(|item| (zip_entry_prefix(item), item.source.clone()))
        .collect();
    tokio::spawn(async move {
        let mut tx = tx;
        let result = write_zip_stream(&mut tx, &items, &zip_shared).await;
        if let Err(error) = result {
            // 客户端中断（duplex 对端关闭）属正常下载取消，其余错误记日志。
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                tracing::warn!(event = "localsend_qr_zip_failed", %error, "zip stream failed");
            }
        } else {
            // 完整写出 zip = 全部条目已交付（duplex 背压保证消费完才算成功）。
            let mut downloaded = zip_shared.downloaded.lock();
            for index in 0..zip_shared.items.len() {
                downloaded.insert(index);
            }
        }
        let _ = tx.shutdown().await;
        zip_shared.active.fetch_sub(1, Ordering::Relaxed);
        zip_shared.touch();
    });

    let zip_name = if shared.items.len() == 1 {
        shared.items[0].zip_name.clone()
    } else {
        "download.zip".to_string()
    };
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(rx));
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, content_disposition(&zip_name));
    response
}

/// zip 内条目前缀：单文件用文件名，目录用目录名。
fn zip_entry_prefix(item: &ItemMeta) -> String {
    match &item.source {
        QrSource::File(path) => path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&item.display_name)
            .to_string(),
        QrSource::Dir(dir) => dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder")
            .to_string(),
    }
}

/// 把全部条目写入 zip 流；文件逐个写入，目录递归（R1：流式不落盘）。
async fn write_zip_stream(
    sink: &mut DuplexStream,
    items: &[(String, QrSource)],
    shared: &Arc<QrShared>,
) -> std::io::Result<()> {
    let zip_error = |error: async_zip::error::ZipError| std::io::Error::other(error);
    let mut writer = async_zip::tokio::write::ZipFileWriter::with_tokio(sink);
    for (prefix, source) in items {
        match source {
            QrSource::File(path) => {
                let mut buffer = Vec::new();
                tokio::fs::File::open(path)
                    .await?
                    .read_to_end(&mut buffer)
                    .await?;
                let builder = async_zip::ZipEntryBuilder::new(
                    prefix.clone().into(),
                    async_zip::Compression::Deflate,
                );
                writer
                    .write_entry_whole(builder, &buffer)
                    .await
                    .map_err(zip_error)?;
                shared
                    .bytes_sent
                    .fetch_add(buffer.len() as u64, Ordering::Relaxed);
                shared.touch();
            }
            QrSource::Dir(dir) => {
                let mut stack = vec![(dir.to_path_buf(), String::new())];
                while let Some((current, relative)) = stack.pop() {
                    let mut entries = tokio::fs::read_dir(&current).await?;
                    while let Some(entry) = entries.next_entry().await? {
                        let name = if relative.is_empty() {
                            format!("{prefix}/{}", entry.file_name().to_string_lossy())
                        } else {
                            format!(
                                "{prefix}/{relative}/{}",
                                entry.file_name().to_string_lossy()
                            )
                        };
                        if entry.metadata().await?.is_dir() {
                            let child = if relative.is_empty() {
                                entry.file_name().to_string_lossy().to_string()
                            } else {
                                format!("{relative}/{}", entry.file_name().to_string_lossy())
                            };
                            stack.push((entry.path(), child));
                            continue;
                        }
                        let mut buffer = Vec::new();
                        tokio::fs::File::open(entry.path())
                            .await?
                            .read_to_end(&mut buffer)
                            .await?;
                        let builder = async_zip::ZipEntryBuilder::new(
                            name.into(),
                            async_zip::Compression::Deflate,
                        );
                        writer
                            .write_entry_whole(builder, &buffer)
                            .await
                            .map_err(zip_error)?;
                        shared
                            .bytes_sent
                            .fetch_add(buffer.len() as u64, Ordering::Relaxed);
                        shared.touch();
                    }
                }
            }
        }
    }
    writer.close().await.map_err(zip_error)?;
    Ok(())
}

fn content_disposition(name: &str) -> header::HeaderValue {
    let mut encoded = String::from("attachment; filename*=UTF-8''");
    for byte in name.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~') {
            encoded.push(c);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    header::HeaderValue::from_str(&encoded)
        .unwrap_or(header::HeaderValue::from_static("attachment"))
}

/// HTML 文本转义：文件名来自用户选择，防止注入列表页。
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 计数 AsyncRead：透传文件字节并累计到共享计数器；EOF 即标记条目完整下载。
struct CountingReader {
    inner: tokio::fs::File,
    shared: Arc<QrShared>,
    index: usize,
    done: bool,
    /// 保底计数回收：Drop 时机 = 响应流结束（完成或断开）。仅作为 Drop 守卫持有。
    #[allow(dead_code)]
    guard: ActiveGuard,
}

impl AsyncRead for CountingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let read = (buf.filled().len() - before) as u64;
            if read > 0 {
                this.shared.bytes_sent.fetch_add(read, Ordering::Relaxed);
                this.shared.touch();
            } else if !this.done {
                // Ok(0) = EOF：条目完整交付。
                this.done = true;
                this.shared.downloaded.lock().insert(this.index);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_and_size_format() {
        assert_eq!(html_escape("<a&b>"), "&lt;a&amp;b&gt;");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KiB");
    }

    #[test]
    fn content_disposition_encodes_utf8() {
        let value = content_disposition("报告 report.pdf")
            .to_str()
            .unwrap()
            .to_string();
        assert!(value.starts_with("attachment; filename*=UTF-8''"));
        assert!(value.contains("%E6%8A%A5"));
    }
}
