//! LocalSend 直推客户端：prepare-upload → upload×N → cancel（有文件没传成时）。
//!
//! 文件先由 `prepare` 展开、算好校验（几台设备共用一份）；这里只管发。一个文件失败不
//! 影响别的（同官方）：对方校验不符回 422 时整个文件重传，同一文件最多共传 3 次。

use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::error::{error_chain_text, LocalSendError};
use crate::outbound_tls::OutboundTls;

pub(crate) mod prepare;

use prepare::{FileFailure, PreparedFile, PreparedFiles};

/// 直推进度事件的节流步长。
const PROGRESS_STEP: u64 = 1024 * 1024;
/// 同一文件最多传几次（同官方接收端的 `MAX_UPLOAD_ATTEMPTS`，发送方不能比它多）。
const MAX_UPLOAD_ATTEMPTS: u8 = 3;

/// 直推进度快照（阶段 B 的窗口进度显示直接消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendProgress {
    pub file_name: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_sent: u64,
    pub file_size: u64,
}

/// 一次直推的结果：发成了几个，哪些没发成（含准备时就读不了的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub sent: usize,
    pub failed: Vec<FileFailure>,
}

/// 直推客户端：出站 TLS 配置（出示设备身份）+ 本机设备信息。
#[derive(Clone)]
pub(crate) struct DirectSender {
    /// 发给明文 HTTP 设备用：没有证书可钉。
    client: reqwest::Client,
    outbound: OutboundTls,
    own_device: DeviceInfo,
}

impl DirectSender {
    pub(crate) fn new(
        client: reqwest::Client,
        outbound: OutboundTls,
        own_device: DeviceInfo,
    ) -> Self {
        Self {
            client,
            outbound,
            own_device,
        }
    }

    /// 直推准备好的文件；进度通道满时丢弃进度（UI 只需要最新值）。整次被拒（PIN、
    /// 拒绝、连不上）返回 Err；单个文件没发成记进 `SendReport.failed`。
    pub(crate) async fn send(
        &self,
        target: &DeviceInfo,
        files: &PreparedFiles,
        pin: Option<&str>,
        progress: mpsc::Sender<SendProgress>,
    ) -> Result<SendReport, LocalSendError> {
        if files.files.is_empty() {
            // 全都读不了：不必打扰对方。
            return match files.unreadable.is_empty() {
                true => Err(LocalSendError::NothingToSend),
                false => Ok(SendReport {
                    sent: 0,
                    failed: files.unreadable.clone(),
                }),
            };
        }
        let base = target
            .http_base_url()
            .ok_or(LocalSendError::AddressUnknown)?;
        // 开了加密的设备：握手钉住设备表里的指纹（发现时取自它的证书），换了设备的
        // 地址在发出文件清单之前就断开。
        let pinned;
        let client = if target.is_certified() {
            pinned = self.outbound.client(Some(&target.fingerprint))?;
            &pinned
        } else {
            &self.client
        };

        let request = PrepareUploadRequest {
            // 官方接收端以客户端证书哈希识别发送方，自报的身份必须是同一个设备身份。
            info: self.own_device.clone(),
            files: files
                .files
                .iter()
                .enumerate()
                .map(|(index, file)| {
                    let id = index.to_string();
                    let metadata = FileMetadata {
                        id: id.clone(),
                        file_name: file.file_name.clone(),
                        size: file.size,
                        file_type: Some(mime_of(&file.file_name).to_string()),
                        sha256: Some(file.sha256.clone()),
                    };
                    (id, metadata)
                })
                .collect(),
        };
        let endpoint = "prepare-upload";
        let url = format!("{base}/api/localsend/v2/{endpoint}");
        let mut prepare = client.post(&url).json(&request);
        if let Some(pin) = pin {
            prepare = prepare.query(&[("pin", pin)]);
        }
        let response = prepare
            .send()
            .await
            .map_err(|source| LocalSendError::Request { endpoint, source })?;
        let mut failed = files.unreadable.clone();
        // 协议：204 = 对方一个也不需要。
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            failed.extend(files.files.iter().map(not_taken));
            return Ok(SendReport { sent: 0, failed });
        }
        if !response.status().is_success() {
            return Err(prepare_upload_refusal(response.status(), pin.is_some()));
        }
        let prepared_response: PrepareUploadResponse = response
            .json()
            .await
            .map_err(|source| LocalSendError::Request { endpoint, source })?;
        let session_id = prepared_response.session_id;

        let mut sent = 0;
        let mut announced_file_failed = false;
        let total = files.files.len();
        for (index, file) in files.files.iter().enumerate() {
            // 对方可以只要其中一部分（官方接收端能挑文件）。
            let Some(token) = prepared_response.files.get(&index.to_string()) else {
                failed.push(not_taken(file));
                continue;
            };
            let upload = Upload {
                client,
                base: &base,
                session_id: &session_id,
                file_id: index.to_string(),
                token,
                file,
                progress: &progress,
                index,
                total,
            };
            match upload.with_retries().await {
                Ok(()) => sent += 1,
                Err(reason) => {
                    announced_file_failed = true;
                    failed.push(FileFailure {
                        file_name: file.file_name.clone(),
                        reason,
                    });
                }
            }
        }
        // 宣告过的文件没传完：对方的会话等不到它，取消让对方收尾（删半成品、报结果）。
        if announced_file_failed {
            cancel_session(client, &base, &session_id).await;
        }
        Ok(SendReport { sent, failed })
    }
}

fn not_taken(file: &PreparedFile) -> FileFailure {
    FileFailure {
        file_name: file.file_name.clone(),
        reason: "the device did not take this file".to_owned(),
    }
}

/// prepare-upload 被拒的原因：PIN、拒绝、拦截各有各的意思（同官方发送端的分支），
/// 界面据此决定是让用户输 PIN 还是报错。
fn prepare_upload_refusal(status: reqwest::StatusCode, pin_given: bool) -> LocalSendError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED if pin_given => LocalSendError::WrongPin,
        reqwest::StatusCode::UNAUTHORIZED => LocalSendError::PinRequired,
        reqwest::StatusCode::FORBIDDEN => LocalSendError::Declined,
        reqwest::StatusCode::TOO_MANY_REQUESTS => LocalSendError::TooManyPinAttempts,
        status => LocalSendError::PeerRejected {
            status,
            endpoint: "prepare-upload",
        },
    }
}

/// 一个文件的上传：地址、令牌与进度归属都来自会话与这批文件。
struct Upload<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    session_id: &'a str,
    file_id: String,
    token: &'a str,
    file: &'a PreparedFile,
    progress: &'a mpsc::Sender<SendProgress>,
    index: usize,
    total: usize,
}

enum UploadError {
    ChecksumMismatch,
    Other(String),
}

impl Upload<'_> {
    /// 对方说校验不符（422）就整个文件重传，同一文件最多共传 3 次。
    async fn with_retries(&self) -> Result<(), String> {
        for attempt in 1..=MAX_UPLOAD_ATTEMPTS {
            match self.once().await {
                Ok(()) => return Ok(()),
                Err(UploadError::ChecksumMismatch) if attempt < MAX_UPLOAD_ATTEMPTS => continue,
                Err(UploadError::ChecksumMismatch) => break,
                Err(UploadError::Other(reason)) => return Err(reason),
            }
        }
        Err(format!(
            "the received file did not match its checksum {MAX_UPLOAD_ATTEMPTS} times"
        ))
    }

    async fn once(&self) -> Result<(), UploadError> {
        let url = format!(
            "{}/api/localsend/v2/upload?sessionId={}&fileId={}&token={}",
            self.base, self.session_id, self.file_id, self.token
        );
        let source = tokio::fs::File::open(&self.file.path)
            .await
            .map_err(|error| UploadError::Other(error.to_string()))?;
        // 计数 AsyncRead 包装：ReaderStream 从中切分出 chunk，进度按节流步长投递。
        let reader = CountingReader {
            inner: source,
            count: 0,
            last_emitted: 0,
            file_name: self.file.file_name.clone(),
            file_size: self.file.size,
            file_index: self.index,
            total_files: self.total,
            progress: self.progress.clone(),
        };
        let response = self
            .client
            .post(&url)
            .body(reqwest::Body::wrap_stream(
                tokio_util::io::ReaderStream::new(reader),
            ))
            .send()
            .await
            .map_err(|error| UploadError::Other(error_chain_text(&error)))?;
        match response.status() {
            status if status.is_success() => {}
            reqwest::StatusCode::UNPROCESSABLE_ENTITY => return Err(UploadError::ChecksumMismatch),
            status => return Err(UploadError::Other(format!("HTTP {status}"))),
        }
        // 发送端进度只反映本机读取量；HTTP 200 即对端确认接收完成。
        emit_progress(
            self.progress,
            &self.file.file_name,
            self.index,
            self.total,
            self.file.size,
            self.file.size,
        );
        Ok(())
    }
}

async fn cancel_session(client: &reqwest::Client, base: &str, session_id: &str) {
    let url = format!("{base}/api/localsend/v2/cancel?sessionId={session_id}");
    // cancel 是尽力而为的清理，失败只记日志不改变错误传播。
    let _ = client.post(&url).send().await;
}

/// 扩展名 → MIME；未知类型统一 octet-stream（浏览器/对端都按下载处理）。
fn mime_of(file_name: &str) -> &'static str {
    mime_guess::MimeGuess::from_path(Path::new(file_name))
        .first_raw()
        .unwrap_or("application/octet-stream")
}

fn emit_progress(
    progress: &mpsc::Sender<SendProgress>,
    file_name: &str,
    file_index: usize,
    total_files: usize,
    bytes_sent: u64,
    file_size: u64,
) {
    // try_send：通道满即丢弃进度（UI 只需要最新值，不允许反压传输）。
    let _ = progress.try_send(SendProgress {
        file_name: file_name.to_string(),
        file_index,
        total_files,
        bytes_sent,
        file_size,
    });
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
    progress: mpsc::Sender<SendProgress>,
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

/// 测试用直推客户端：在 `dir` 里新建设备身份，按服务同样的方式组装；
/// 返回客户端与身份 fingerprint。
#[cfg(test)]
pub(crate) fn sender_with_new_identity(dir: &Path) -> (DirectSender, String) {
    let identity =
        crate::identity::DeviceIdentity::load_or_create(&dir.join("identity.pem")).unwrap();
    let own_device = crate::own_device_info("TestPC".to_owned(), None, 53317, &identity);
    let outbound = OutboundTls::new(&identity).unwrap();
    let sender = DirectSender::new(outbound.client(None).unwrap(), outbound, own_device);
    (sender, identity.fingerprint().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn mime_guess_covers_common_types() {
        assert_eq!(mime_of("a.txt"), "text/plain");
        assert_eq!(mime_of("b.unknownext"), "application/octet-stream");
    }

    // 端到端成功路径见 integration 与 receive 联动（A4）；这里覆盖纯函数与失败文本。
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

    async fn one_file(dir: &tempfile::TempDir) -> PreparedFiles {
        let file = dir.path().join("one.txt");
        std::fs::write(&file, b"x").unwrap();
        prepare::prepare_files(vec![file], mpsc::channel(1).0).await
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
        let (sender, _) = sender_with_new_identity(dir.path());
        let (progress, _) = mpsc::channel(1);

        let error = sender
            .send(&target_at(address), &one_file(&dir).await, None, progress)
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
        let (sender, _) = sender_with_new_identity(dir.path());
        let (progress, _) = mpsc::channel(1);

        let report = sender
            .send(&target_at(address), &one_file(&dir).await, None, progress)
            .await
            .unwrap();
        responder.await.unwrap();

        assert_eq!(report.sent, 0);
        let [FileFailure { file_name, reason }] = report.failed.as_slice() else {
            panic!("expected one failed file, got {report:?}");
        };
        assert_eq!(file_name, "one.txt");
        let refused = std::net::TcpStream::connect(address)
            .unwrap_err()
            .to_string();
        assert!(reason.contains(&refused), "{reason:?} lost {refused:?}");
    }
}
