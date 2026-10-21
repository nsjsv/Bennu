//! LocalSend 直推客户端：prepare-upload → upload×N → cancel（失败时）。
//!
//! 目录条目内存打包为单个 `<名称>.zip`（与 QR 路径的 zip 策略一致）；
//! `ponytail:` 内存缓冲整个目录，超大目录若成问题再改临时文件/分块。

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::mpsc;

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::error::{error_chain_text, LocalSendError};

/// 单请求连接超时：局域网内握手超过 5s 视为对端不可达；总时长不设限（大文件）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 直推进度事件的节流步长。
const PROGRESS_STEP: u64 = 1024 * 1024;

/// 直推进度快照（阶段 B 的窗口进度显示直接消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendProgress {
    pub file_name: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_sent: u64,
    pub file_size: u64,
}

/// 发送前的文件准备结果：目录已打包成 zip 缓冲。
enum PreparedFile {
    Path {
        file_name: String,
        size: u64,
        path: PathBuf,
    },
    Memory {
        file_name: String,
        bytes: axum::body::Bytes,
    },
}

impl PreparedFile {
    fn file_name(&self) -> &str {
        match self {
            PreparedFile::Path { file_name, .. } => file_name,
            PreparedFile::Memory { file_name, .. } => file_name,
        }
    }

    fn size(&self) -> u64 {
        match self {
            PreparedFile::Path { size, .. } => *size,
            PreparedFile::Memory { bytes, .. } => bytes.len() as u64,
        }
    }
}

/// LocalSend v2 契约入口：直推文件清单到目标设备。
pub async fn send_to_device(
    target: &DeviceInfo,
    items: Vec<PathBuf>,
    display_alias: &str,
) -> Result<(), LocalSendError> {
    send_to_device_inner(target, items, display_alias, None).await
}

/// 带进度通道的直推变体；通道满时进度被丢弃（UI 只需要最新值）。
pub async fn send_to_device_with_progress(
    target: &DeviceInfo,
    items: Vec<PathBuf>,
    display_alias: &str,
    progress: mpsc::Sender<SendProgress>,
) -> Result<(), LocalSendError> {
    send_to_device_inner(target, items, display_alias, Some(progress)).await
}

async fn send_to_device_inner(
    target: &DeviceInfo,
    items: Vec<PathBuf>,
    display_alias: &str,
    progress: Option<mpsc::Sender<SendProgress>>,
) -> Result<(), LocalSendError> {
    let base = target
        .http_base_url()
        .ok_or(LocalSendError::AddressUnknown)?;
    let prepared = prepare_items(items).await?;
    if prepared.is_empty() {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        // LocalSend 证书是自签名且无 CA 校验（规范 §2，安全性靠 fingerprint
        // 记忆）；官方客户端同样接受自签名证书，否则 HTTPS 设备全部握手失败。
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|source| LocalSendError::Request {
            endpoint: "client",
            source,
        })?;

    // 一次性发送方身份：契约只传 alias，指纹逐次随机生成即可。
    let mut files = std::collections::HashMap::new();
    for (index, item) in prepared.iter().enumerate() {
        files.insert(
            index.to_string(),
            FileMetadata {
                id: index.to_string(),
                file_name: item.file_name().to_string(),
                size: item.size(),
                file_type: Some(mime_of(item.file_name()).to_string()),
                sha256: None,
            },
        );
    }
    let request = PrepareUploadRequest {
        info: DeviceInfo {
            alias: display_alias.to_string(),
            version: "2.0".into(),
            fingerprint: crate::random_id(),
            device_model: None,
            device_type: None,
            download: None,
            port: 0,
            protocol: "http".into(),
            address: None,
        },
        files,
    };

    let endpoint = "prepare-upload";
    let url = format!("{base}/api/localsend/v2/{endpoint}");
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|source| LocalSendError::Request { endpoint, source })?;
    if !response.status().is_success() {
        return Err(LocalSendError::PeerRejected {
            status: response.status(),
            endpoint,
        });
    }
    let prepared_response: PrepareUploadResponse = response
        .json()
        .await
        .map_err(|source| LocalSendError::Request { endpoint, source })?;
    let session_id = prepared_response.session_id;

    // 逐文件上传；任一失败 → cancel 会话并返回失败汇总。
    for (index, item) in prepared.iter().enumerate() {
        let Some(token) = prepared_response.files.get(&index.to_string()) else {
            cancel_session(&client, &base, &session_id).await;
            return Err(LocalSendError::PeerProtocol {
                endpoint: "prepare-upload",
                reason: format!("missing token for fileId {index}"),
            });
        };
        let result = upload_one(
            &client,
            &base,
            &session_id,
            &index.to_string(),
            token,
            item,
            &progress,
            index,
            prepared.len(),
        )
        .await;
        if let Err(reason) = result {
            cancel_session(&client, &base, &session_id).await;
            return Err(LocalSendError::TransferFile {
                file_name: item.file_name().to_string(),
                reason,
            });
        }
    }
    Ok(())
}

/// 枚举并准备全部条目；目录 → 内存 zip（顶层保留目录名）。
async fn prepare_items(items: Vec<PathBuf>) -> Result<Vec<PreparedFile>, LocalSendError> {
    let mut prepared = Vec::new();
    for item in items {
        let metadata = tokio::fs::metadata(&item)
            .await
            .map_err(|source| LocalSendError::Io {
                path: Some(item.clone()),
                source,
            })?;
        if metadata.is_dir() {
            let dir_name = item
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| LocalSendError::InvalidPath { path: item.clone() })?
                .to_string();
            let bytes = zip_directory_to_memory(&item, &dir_name).await?;
            prepared.push(PreparedFile::Memory {
                file_name: format!("{dir_name}.zip"),
                // Bytes::from(Vec) 零拷贝接管缓冲。
                bytes: axum::body::Bytes::from(bytes),
            });
        } else {
            let file_name = item
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| LocalSendError::InvalidPath { path: item.clone() })?
                .to_string();
            prepared.push(PreparedFile::Path {
                file_name,
                size: metadata.len(),
                path: item.clone(),
            });
        }
    }
    Ok(prepared)
}

// 九个参数各自来自会话状态、单文件 prepare 结果与进度通道的不同来源，
// 且调用点只有一个；为迎合 lint 聚合成参数结构只会多一层无意义的中转映射。
#[allow(clippy::too_many_arguments)]
async fn upload_one(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    file_id: &str,
    token: &str,
    item: &PreparedFile,
    progress: &Option<mpsc::Sender<SendProgress>>,
    index: usize,
    total: usize,
) -> Result<(), String> {
    let endpoint = "upload";
    let url = format!(
        "{base}/api/localsend/v2/{endpoint}?sessionId={session_id}&fileId={file_id}&token={token}"
    );
    let body = match item {
        PreparedFile::Path { path, size, .. } => {
            let file = tokio::fs::File::open(path)
                .await
                .map_err(|error| error.to_string())?;
            // 计数 AsyncRead 包装：ReaderStream 从中切分出 chunk，进度按节流步长投递。
            let reader = CountingReader {
                inner: file,
                count: 0,
                last_emitted: 0,
                file_name: item.file_name().to_string(),
                file_size: *size,
                file_index: index,
                total_files: total,
                progress: progress.clone(),
            };
            reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(reader))
        }
        PreparedFile::Memory { bytes, .. } => {
            emit_progress(
                progress,
                item.file_name(),
                index,
                total,
                bytes.len() as u64,
                bytes.len() as u64,
            );
            reqwest::Body::from(bytes.clone())
        }
    };
    let response = client
        .post(&url)
        .body(body)
        .send()
        .await
        .map_err(|error| error_chain_text(&error))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    // 发送端进度只反映本机读取量；HTTP 200 即对端确认接收完成。
    emit_progress(
        progress,
        item.file_name(),
        index,
        total,
        item.size(),
        item.size(),
    );
    Ok(())
}

async fn cancel_session(client: &reqwest::Client, base: &str, session_id: &str) {
    let url = format!("{base}/api/localsend/v2/cancel?sessionId={session_id}");
    // cancel 是尽力而为的清理，失败只记日志不改变错误传播。
    let _ = client.post(&url).send().await;
}

/// 目录 → 内存 zip；async_zip 逐文件写缓冲，保持流式压缩避免一次性读入。
async fn zip_directory_to_memory(dir: &Path, top_level: &str) -> Result<Vec<u8>, LocalSendError> {
    fn zip_error(path: PathBuf, error: async_zip::error::ZipError) -> LocalSendError {
        LocalSendError::Io {
            path: Some(path),
            source: std::io::Error::other(error),
        }
    }
    let mut buffer = Vec::new();
    let mut writer = async_zip::tokio::write::ZipFileWriter::with_tokio(&mut buffer);
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, relative)) = stack.pop() {
        let mut entries =
            tokio::fs::read_dir(&current)
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(current.clone()),
                    source,
                })?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|source| LocalSendError::Io {
                path: Some(current.clone()),
                source,
            })?
        {
            let name_in_zip = if relative.is_empty() {
                format!("{top_level}/{}", entry.file_name().to_string_lossy())
            } else {
                format!(
                    "{top_level}/{relative}/{}",
                    entry.file_name().to_string_lossy()
                )
            };
            if entry
                .metadata()
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(entry.path()),
                    source,
                })?
                .is_dir()
            {
                let child_relative = if relative.is_empty() {
                    entry.file_name().to_string_lossy().to_string()
                } else {
                    format!("{relative}/{}", entry.file_name().to_string_lossy())
                };
                stack.push((entry.path(), child_relative));
                continue;
            }
            let mut bytes = Vec::new();
            tokio::fs::File::open(entry.path())
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(entry.path()),
                    source,
                })?
                .read_to_end(&mut bytes)
                .await
                .map_err(|source| LocalSendError::Io {
                    path: Some(entry.path()),
                    source,
                })?;
            let entry_builder = async_zip::ZipEntryBuilder::new(
                name_in_zip.into(),
                async_zip::Compression::Deflate,
            );
            writer
                .write_entry_whole(entry_builder, &bytes)
                .await
                .map_err(|error| zip_error(entry.path(), error))?;
        }
    }
    writer
        .close()
        .await
        .map_err(|error| zip_error(dir.to_path_buf(), error))?;
    Ok(buffer)
}

/// 扩展名 → MIME；未知类型统一 octet-stream（浏览器/对端都按下载处理）。
fn mime_of(file_name: &str) -> &'static str {
    mime_guess::MimeGuess::from_path(Path::new(file_name))
        .first_raw()
        .unwrap_or("application/octet-stream")
}

fn emit_progress(
    progress: &Option<mpsc::Sender<SendProgress>>,
    file_name: &str,
    file_index: usize,
    total_files: usize,
    bytes_sent: u64,
    file_size: u64,
) {
    if let Some(progress) = progress {
        // try_send：通道满即丢弃进度（UI 只需要最新值，不允许反压传输）。
        let _ = progress.try_send(SendProgress {
            file_name: file_name.to_string(),
            file_index,
            total_files,
            bytes_sent,
            file_size,
        });
    }
}

/// 计数 AsyncRead：透传读取并在节流步长处投递进度。
struct CountingReader {
    inner: tokio::fs::File,
    count: u64,
    last_emitted: u64,
    file_name: String,
    file_size: u64,
    file_index: usize,
    total_files: usize,
    progress: Option<mpsc::Sender<SendProgress>>,
}

impl AsyncRead for CountingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let read = (buf.filled().len() - before) as u64;
            if read > 0 {
                self.count += read;
                if self.count - self.last_emitted >= PROGRESS_STEP {
                    self.last_emitted = self.count;
                    emit_progress(
                        &self.progress,
                        &self.file_name,
                        self.file_index,
                        self.total_files,
                        self.count,
                        self.file_size,
                    );
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn mime_guess_covers_common_types() {
        assert_eq!(mime_of("a.txt"), "text/plain");
        assert_eq!(mime_of("b.unknownext"), "application/octet-stream");
    }

    // 端到端成功路径见 integration 与 receive 联动（A4）；这里覆盖纯函数与失败文本。
    #[tokio::test]
    async fn zip_directory_produces_valid_archive() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), b"hello")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
        tokio::fs::write(dir.path().join("sub").join("b.bin"), [7u8; 64])
            .await
            .unwrap();

        let zip = zip_directory_to_memory(dir.path(), "folder").await.unwrap();
        let reader = async_zip::tokio::read::seek::ZipFileReader::with_tokio(Cursor::new(zip))
            .await
            .unwrap();
        let mut names = Vec::new();
        for index in 0..reader.file().entries().len() {
            let entry = reader.file().entries()[index].filename().clone();
            names.push(entry.into_string().unwrap());
        }
        assert!(names.contains(&"folder/a.txt".to_string()));
        assert!(names.contains(&"folder/sub/b.bin".to_string()));
    }

    fn target_at(address: std::net::SocketAddr) -> DeviceInfo {
        DeviceInfo {
            alias: "Phone".into(),
            version: "2.0".into(),
            fingerprint: "phone-fp".into(),
            device_model: None,
            device_type: None,
            download: None,
            port: address.port(),
            protocol: "http".into(),
            address: Some(address.ip()),
        }
    }

    fn one_file(dir: &tempfile::TempDir) -> Vec<PathBuf> {
        let file = dir.path().join("one.txt");
        std::fs::write(&file, b"x").unwrap();
        vec![file]
    }

    /// 读完一个带 content-length 的请求再回响应：未读数据留在内核缓冲时
    /// 关闭连接会发 RST，客户端可能读不到响应。
    async fn read_http_request(stream: &mut tokio::net::TcpStream) {
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "client closed before sending the whole request");
            request.extend_from_slice(&chunk[..read]);
            let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
            let body_len: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse().unwrap());
            if request.len() >= head_end + 4 + body_len {
                return;
            }
        }
    }

    #[tokio::test]
    async fn prepare_upload_failure_text_keeps_every_cause() {
        // 绑定后立即释放：拿到一个确定无人监听的本机端口。
        let address = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();

        let error = send_to_device(&target_at(address), one_file(&dir), "TestPC")
            .await
            .unwrap_err();

        let LocalSendError::Request {
            endpoint: "prepare-upload",
            source,
        } = &error
        else {
            panic!("expected a prepare-upload request error, got {error:?}");
        };
        let text = error.to_string();
        let mut layer: Option<&(dyn std::error::Error + 'static)> = Some(source);
        while let Some(current) = layer {
            assert!(
                text.contains(&current.to_string()),
                "{text:?} lost {current}"
            );
            layer = current.source();
        }
        let refused = std::net::TcpStream::connect(address)
            .unwrap_err()
            .to_string();
        assert!(text.contains(&refused), "{text:?} lost {refused:?}");
    }

    #[tokio::test]
    async fn upload_failure_reason_keeps_the_connection_cause() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // 只应答一次 prepare-upload，且先关监听再回响应：随后的 upload 连接必然被拒。
        let responder = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            drop(listener);
            read_http_request(&mut stream).await;
            let body = r#"{"sessionId":"session","files":{"0":"token"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();

        let error = send_to_device(&target_at(address), one_file(&dir), "TestPC")
            .await
            .unwrap_err();
        responder.await.unwrap();

        let LocalSendError::TransferFile { reason, .. } = &error else {
            panic!("expected an upload failure, got {error:?}");
        };
        let refused = std::net::TcpStream::connect(address)
            .unwrap_err()
            .to_string();
        assert!(reason.contains(&refused), "{reason:?} lost {refused:?}");
    }
}
