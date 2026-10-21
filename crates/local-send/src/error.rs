//! local-send 协议层错误模型。
//!
//! 保留结构化错误，app-ui 在 command 边界统一转字符串展示；
//! Display 文案带上下文与底层原因，不吞错误。

use std::path::PathBuf;

/// 协议层与服务层的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum LocalSendError {
    /// 本机网络接口枚举失败或没有可用 IPv4 地址。
    #[error("no usable local network interface: {0}")]
    NoLocalInterface(String),

    /// 端口被占用（如另一 LocalSend 实例）：只报一次，不重试。
    #[error("port {port} is already in use by another process or LocalSend instance")]
    PortInUse { port: u16 },

    /// 目标设备缺少来源地址（未经过本机发现流程的 DeviceInfo 无法直推）。
    #[error("target device has no known address; it must come from device discovery")]
    AddressUnknown,

    /// 设备身份文件内容不合法（不是一张证书加一把配对私钥）。
    #[error("LocalSend identity file {} is invalid ({reason}); delete it to create a new identity", path.display())]
    InvalidIdentity { path: PathBuf, reason: String },

    /// 请求的资源路径不存在或不是常规文件。
    #[error("path does not exist or is not a regular file: {}", path.display())]
    InvalidPath { path: PathBuf },

    /// 对端拒绝或返回了非预期状态码。
    #[error("peer responded with HTTP {status} for {endpoint}")]
    PeerRejected {
        status: axum::http::StatusCode,
        endpoint: &'static str,
    },

    /// 对端响应不符合协议（缺字段、错结构等）。
    #[error("peer response does not match LocalSend protocol for {endpoint}: {reason}")]
    PeerProtocol {
        endpoint: &'static str,
        reason: String,
    },

    /// 传输过程中某个文件失败（发送端汇总）。
    #[error("failed to transfer {file_name}: {reason}")]
    TransferFile { file_name: String, reason: String },

    /// 网络请求失败。
    #[error("network request to {endpoint} failed: {}", error_chain_text(.source))]
    Request {
        endpoint: &'static str,
        source: reqwest::Error,
    },

    /// 序列化/反序列化失败。
    #[error("json codec failure: {0}")]
    Json(#[from] serde_json::Error),

    /// 本地文件读写失败。
    #[error("local io failure on {}: {source}", path.as_ref().map(|p| p.display().to_string()).unwrap_or_default())]
    Io {
        path: Option<PathBuf>,
        source: std::io::Error,
    },

    /// 定时器意外结束（服务内部任务提前退出）。
    #[error("local-send service task terminated unexpectedly")]
    ServiceTaskTerminated,
}

/// 把错误及其 source 链拼成一行（`a: b: c`）。
///
/// reqwest 0.12 的 `Display` 只写顶层（`error sending request for url (…)`），
/// 连接被拒、超时、TLS 失败等真正原因只在 `source()` 链里；不拼链就会丢原因。
pub(crate) fn error_chain_text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        text.push_str(": ");
        text.push_str(&current.to_string());
        cause = current.source();
    }
    text
}

impl From<std::io::Error> for LocalSendError {
    fn from(source: std::io::Error) -> Self {
        LocalSendError::Io { path: None, source }
    }
}
