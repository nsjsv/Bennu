//! 主程序设置行（`state.sqlite`）里收藏与网络连接的存储形态与唯一解释：主程序与
//! 文件选择器（portal）共用，两边侧边栏显示同一份。设置文档本身在宿主侧，这里只定义
//! 侧边栏那两个键的元素形态，字段名即磁盘格式，旧设置行必须原样可读。

use bennu_state::StoredPath;
use desktop_linux::{NetworkConnection, NetworkConnectionId, NetworkProtocol};
use serde::{Deserialize, Serialize};

use crate::config::SidebarFavoriteConfig;
use crate::saved_connections::SavedNetworkConnection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSidebarFavorite {
    pub label: String,
    pub path: StoredPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredNetworkConnection {
    pub id: String,
    pub label: String,
    pub protocol: String,
    pub uri: String,
    pub auto_connect: bool,
}

pub fn sidebar_favorites_from_stored(
    favorites: &[StoredSidebarFavorite],
) -> Vec<SidebarFavoriteConfig> {
    favorites
        .iter()
        .map(|favorite| SidebarFavoriteConfig {
            label: favorite.label.clone(),
            path: favorite.path.to_path_buf(),
        })
        .collect()
}

/// 协议不认识或 URI 不合法的条目逐条丢弃；调用方可按条数差判断有没有丢。
pub fn network_connections_from_stored(
    connections: &[StoredNetworkConnection],
) -> Vec<SavedNetworkConnection> {
    connections
        .iter()
        .filter_map(|stored| {
            let protocol = NetworkProtocol::from_config_value(&stored.protocol)?;
            NetworkConnection::new(
                NetworkConnectionId::new(stored.id.trim()),
                stored.label.trim().to_owned(),
                protocol,
                &stored.uri,
            )
            .ok()
            .map(|connection| SavedNetworkConnection::new(connection, stored.auto_connect))
        })
        .collect()
}
