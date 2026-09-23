use desktop_linux::NetworkConnection;

/// 保存到 config.toml 的网络连接：连接本体 + 自动连接偏好。凭据永不落
/// 配置（app-ui 网络连接规范）；密码只在宿主进程会话内或系统密钥环。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedNetworkConnection {
    pub connection: NetworkConnection,
    pub auto_connect: bool,
}

impl SavedNetworkConnection {
    pub fn new(connection: NetworkConnection, auto_connect: bool) -> Self {
        Self {
            connection,
            auto_connect,
        }
    }
}
