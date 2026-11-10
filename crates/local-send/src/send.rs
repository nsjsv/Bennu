//! LocalSend 直推客户端：prepare-upload → upload×N → cancel（有文件没传成时）。
//!
//! 文件先由 `prepare` 展开、算好校验（几台设备共用一份）；这里只管发。一个文件失败不
//! 影响别的（同官方）：对方校验不符回 422 时整个文件重传，同一文件最多共传 3 次。
//!
//! 可以中途取消：还在等对方接受时取消就是撤回请求（丢掉连接），返回 `Canceled`；传输中
//! 取消就断掉当前上传，并 `cancel` 对方的会话（对方删掉收了一半的、留下收完的），返回
//! 标了 `canceled` 的报告。

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::error::{error_chain_text, LocalSendError};
use crate::outbound_tls::OutboundTls;

pub(crate) mod prepare;

use prepare::{FileFailure, PreparedFile, PreparedFiles};

/// 直推进度事件的节流步长。
const PROGRESS_STEP: u64 = 1024 * 1024;
/// 同一文件最多传几次（同官方接收端的 `MAX_UPLOAD_ATTEMPTS`，发送方不能比它多）。
const MAX_UPLOAD_ATTEMPTS: u8 = 3;

/// 直推进度快照：当前文件的，与整批的（之前文件的大小之和 + 当前文件已读）。每个文件
/// 开始上传时报一次（0 字节），之后按节流步长报。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendProgress {
    pub file_name: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_sent: u64,
    pub file_size: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// 一次直推的结果：对方接受之后每个文件怎样了（含准备时就读不了的）。传输中取消的
/// 也是一份报告（`canceled`）：已传的照样算传了，传输记录要逐个文件记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub files: Vec<FileReport>,
    pub canceled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    /// 发给对方的名字（文件夹里的是 `文件夹名/相对路径`）。
    pub file_name: String,
    /// 本机的源文件；准备时就读不了的没有。
    pub path: Option<PathBuf>,
    pub size: u64,
    pub outcome: FileOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    Sent,
    /// 英文原因（日志与界面详情都显示它）。
    Failed(String),
    Canceled,
}

impl SendReport {
    pub fn sent(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.outcome == FileOutcome::Sent)
            .count()
    }

    /// 没发成的（取消的不算）。
    pub fn failures(&self) -> Vec<FileFailure> {
        self.files
            .iter()
            .filter_map(|file| match &file.outcome {
                FileOutcome::Failed(reason) => Some(FileFailure {
                    file_name: file.file_name.clone(),
                    reason: reason.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

impl FileReport {
    fn of(file: &PreparedFile, outcome: FileOutcome) -> Self {
        Self {
            file_name: file.file_name.clone(),
            path: Some(file.path.clone()),
            size: file.size,
            outcome,
        }
    }
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
        cancel: &CancellationToken,
    ) -> Result<SendReport, LocalSendError> {
        let mut reports: Vec<FileReport> = files
            .unreadable
            .iter()
            .map(|failure| FileReport {
                file_name: failure.file_name.clone(),
                path: None,
                size: 0,
                outcome: FileOutcome::Failed(failure.reason.clone()),
            })
            .collect();
        if files.files.is_empty() {
            // 全都读不了：不必打扰对方。
            return match reports.is_empty() {
                true => Err(LocalSendError::NothingToSend),
                false => Ok(SendReport {
                    files: reports,
                    canceled: false,
                }),
            };
        }
        let base = target
            .http_base_url()
            .ok_or(LocalSendError::AddressUnknown)?;
        let client = &self.client_for(target)?;
        let announced = files
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
                    preview: None,
                };
                (id, metadata)
            })
            .collect();
        // 协议：204 = 对方一个也不需要。
        let prepare = self.prepare_upload(client, &base, announced, pin);
        let Some(prepared_response) = unless_canceled(cancel, prepare).await? else {
            reports.extend(files.files.iter().map(not_taken));
            return Ok(SendReport {
                files: reports,
                canceled: false,
            });
        };
        let session_id = prepared_response.session_id;

        let mut announced_file_failed = false;
        let total = files.files.len();
        let bytes_total = files.files.iter().map(|file| file.size).sum();
        let mut bytes_before = 0;
        for (index, file) in files.files.iter().enumerate() {
            let reporter = ProgressReporter {
                channel: progress.clone(),
                file_name: file.file_name.clone(),
                file_index: index,
                total_files: total,
                file_size: file.size,
                bytes_before,
                bytes_total,
            };
            bytes_before += file.size;
            // 对方可以只要其中一部分（官方接收端能挑文件）。
            let Some(token) = prepared_response.files.get(&index.to_string()) else {
                reports.push(not_taken(file));
                continue;
            };
            reporter.report(0);
            let upload = Upload {
                client,
                base: &base,
                session_id: &session_id,
                file_id: index.to_string(),
                token,
                file,
                reporter,
            };
            let outcome = tokio::select! {
                outcome = upload.with_retries() => outcome,
                () = cancel.cancelled() => {
                    // 正在传的与后面的都算取消；已传的照样算传了。
                    cancel_session(client, &base, &session_id).await;
                    reports.extend(
                        files.files[index..]
                            .iter()
                            .map(|file| FileReport::of(file, FileOutcome::Canceled)),
                    );
                    return Ok(SendReport {
                        files: reports,
                        canceled: true,
                    });
                }
            };
            match outcome {
                Ok(()) => reports.push(FileReport::of(file, FileOutcome::Sent)),
                Err(reason) => {
                    announced_file_failed = true;
                    reports.push(FileReport::of(file, FileOutcome::Failed(reason)));
                }
            }
        }
        // 宣告过的文件没传完：对方的会话等不到它，取消让对方收尾（删半成品、报结果）。
        if announced_file_failed {
            cancel_session(client, &base, &session_id).await;
        }
        Ok(SendReport {
            files: reports,
            canceled: false,
        })
    }

    /// 发一段文字（同官方）：一个 `text/plain` 文件，内容放在 preview 里；对方读完回
    /// 204。有的接收端把文字当文件收、给了令牌，就把文字当那个文件的内容传过去。
    pub(crate) async fn send_text(
        &self,
        target: &DeviceInfo,
        text: &str,
        pin: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<(), LocalSendError> {
        let base = target
            .http_base_url()
            .ok_or(LocalSendError::AddressUnknown)?;
        let client = &self.client_for(target)?;
        let file_id = "0".to_owned();
        let message = FileMetadata {
            id: file_id.clone(),
            file_name: format!("{}.txt", crate::random_id()),
            size: text.len() as u64,
            file_type: Some("text/plain".to_owned()),
            sha256: None,
            preview: Some(text.to_owned()),
        };
        let announced = [(file_id.clone(), message)].into_iter().collect();
        // 官方接收端要等对方看完文字才回：这段时间里取消就是撤回。
        let prepare = self.prepare_upload(client, &base, announced, pin);
        let Some(prepared) = unless_canceled(cancel, prepare).await? else {
            return Ok(());
        };
        let Some(token) = prepared.files.get(&file_id) else {
            return Ok(());
        };
        let endpoint = "upload";
        let url = format!(
            "{base}/api/localsend/v2/{endpoint}?sessionId={}&fileId={file_id}&token={token}",
            prepared.session_id
        );
        let upload = async {
            client
                .post(&url)
                .body(text.to_owned())
                .send()
                .await
                .map_err(|source| LocalSendError::Request { endpoint, source })
        };
        let response = match unless_canceled(cancel, upload).await {
            Err(LocalSendError::Canceled) => {
                cancel_session(client, &base, &prepared.session_id).await;
                return Err(LocalSendError::Canceled);
            }
            response => response?,
        };
        if !response.status().is_success() {
            return Err(LocalSendError::PeerRejected {
                status: response.status(),
                endpoint,
            });
        }
        Ok(())
    }

    /// 开了加密的设备：握手钉住设备表里的指纹（发现时取自它的证书），换了设备的地址在
    /// 发出任何请求字节之前就断开。明文设备用不钉的共享客户端。
    fn client_for(&self, target: &DeviceInfo) -> Result<reqwest::Client, LocalSendError> {
        if target.is_certified() {
            self.outbound.client(Some(&target.fingerprint))
        } else {
            Ok(self.client.clone())
        }
    }

    /// prepare-upload：对方要哪些文件（返回会话与令牌）；204 = 一个也不要（None）。
    /// 被拒的原因按 `prepare_upload_refusal` 分清楚。
    async fn prepare_upload(
        &self,
        client: &reqwest::Client,
        base: &str,
        files: std::collections::HashMap<String, FileMetadata>,
        pin: Option<&str>,
    ) -> Result<Option<PrepareUploadResponse>, LocalSendError> {
        let request = PrepareUploadRequest {
            // 官方接收端以客户端证书哈希识别发送方，自报的身份必须是同一个设备身份。
            info: self.own_device.clone(),
            files,
        };
        let endpoint = "prepare-upload";
        let mut prepare = client
            .post(format!("{base}/api/localsend/v2/{endpoint}"))
            .json(&request);
        if let Some(pin) = pin {
            prepare = prepare.query(&[("pin", pin)]);
        }
        let response = prepare
            .send()
            .await
            .map_err(|source| LocalSendError::Request { endpoint, source })?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(prepare_upload_refusal(response.status(), pin.is_some()));
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|source| LocalSendError::Request { endpoint, source })
    }
}

/// 取消先到就返回 `Canceled`（丢掉还没完成的请求，连接随之关闭）。
async fn unless_canceled<T>(
    cancel: &CancellationToken,
    work: impl std::future::Future<Output = Result<T, LocalSendError>>,
) -> Result<T, LocalSendError> {
    tokio::select! {
        outcome = work => outcome,
        () = cancel.cancelled() => Err(LocalSendError::Canceled),
    }
}

fn not_taken(file: &PreparedFile) -> FileReport {
    FileReport::of(
        file,
        FileOutcome::Failed("the device did not take this file".to_owned()),
    )
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
    reporter: ProgressReporter,
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
            reporter: self.reporter.clone(),
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
        self.reporter.report(self.file.size);
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

/// 一个文件的进度怎么报：它在整批里的位置与大小。
#[derive(Clone)]
struct ProgressReporter {
    channel: mpsc::Sender<SendProgress>,
    file_name: String,
    file_index: usize,
    total_files: usize,
    file_size: u64,
    /// 这个文件之前的文件大小之和。
    bytes_before: u64,
    bytes_total: u64,
}

impl ProgressReporter {
    fn report(&self, bytes_sent: u64) {
        // try_send：通道满即丢弃进度（UI 只需要最新值，不允许反压传输）。
        let _ = self.channel.try_send(SendProgress {
            file_name: self.file_name.clone(),
            file_index: self.file_index,
            total_files: self.total_files,
            bytes_sent,
            file_size: self.file_size,
            bytes_done: self.bytes_before + bytes_sent.min(self.file_size),
            bytes_total: self.bytes_total,
        });
    }
}

/// 计数 AsyncRead：透传读取并在节流步长处投递进度。
struct CountingReader {
    inner: tokio::fs::File,
    count: u64,
    last_emitted: u64,
    reporter: ProgressReporter,
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
                    self.reporter.report(self.count);
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
        prepare::prepare_files(vec![file], mpsc::channel(1).0, CancellationToken::new())
            .await
            .unwrap()
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
            .send(
                &target_at(address),
                &one_file(&dir).await,
                None,
                progress,
                &crate::CancellationToken::new(),
            )
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
            .send(
                &target_at(address),
                &one_file(&dir).await,
                None,
                progress,
                &crate::CancellationToken::new(),
            )
            .await
            .unwrap();
        responder.await.unwrap();

        assert_eq!(report.sent(), 0);
        let failures = report.failures();
        let [FileFailure { file_name, reason }] = failures.as_slice() else {
            panic!("expected one failed file, got {report:?}");
        };
        assert_eq!(file_name, "one.txt");
        let refused = std::net::TcpStream::connect(address)
            .unwrap_err()
            .to_string();
        assert!(reason.contains(&refused), "{reason:?} lost {refused:?}");
    }
}
