//! 侧边栏数据模型，以及旧版 config.toml 里侧边栏两个字段（sidebar_favorites /
//! network_connections）的解析——只有主程序把旧配置迁到设置行时用；现在的收藏与
//! 网络连接由 `stored` 从设置行读。

use std::path::PathBuf;

use desktop_linux::{NetworkConnection, NetworkConnectionId, NetworkProtocol};

use crate::locations::sidebar_favorite_label_from_path;
use crate::saved_connections::SavedNetworkConnection;

const SIDEBAR_FAVORITES_KEY: &str = "sidebar_favorites";
const SIDEBAR_FAVORITE_LABEL_KEY: &str = "label";
const SIDEBAR_FAVORITE_PATH_KEY: &str = "path";
const NETWORK_CONNECTIONS_KEY: &str = "network_connections";
const NETWORK_CONNECTION_ID_KEY: &str = "id";
const NETWORK_CONNECTION_LABEL_KEY: &str = "label";
const NETWORK_CONNECTION_PROTOCOL_KEY: &str = "protocol";
const NETWORK_CONNECTION_URI_KEY: &str = "uri";
const NETWORK_CONNECTION_AUTO_CONNECT_KEY: &str = "auto_connect";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarFavoriteConfig {
    pub label: String,
    pub path: PathBuf,
}

/// 侧边栏视角的收藏与网络连接。favorites 为 None 表示未配置
/// （消费方回落默认用户目录 + GTK 书签）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidebarConfig {
    pub favorites: Option<Vec<SidebarFavoriteConfig>>,
    pub network_connections: Vec<SavedNetworkConnection>,
}

pub fn parse_toml_sidebar_favorites(document: &toml::Table) -> Option<Vec<SidebarFavoriteConfig>> {
    let entries = document.get(SIDEBAR_FAVORITES_KEY)?.as_array()?;
    let mut favorites = Vec::new();
    for entry in entries {
        let Some(table) = entry.as_table() else {
            continue;
        };
        let Some(path) = toml_string(table, SIDEBAR_FAVORITE_PATH_KEY) else {
            continue;
        };
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() {
            continue;
        }
        let label = toml_string(table, SIDEBAR_FAVORITE_LABEL_KEY)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| sidebar_favorite_label_from_path(&path));
        favorites.push(SidebarFavoriteConfig { label, path });
    }
    Some(favorites)
}

pub fn parse_toml_network_connections(document: &toml::Table) -> Vec<SavedNetworkConnection> {
    let Some(entries) = document
        .get(NETWORK_CONNECTIONS_KEY)
        .and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };

    let mut connections = Vec::new();
    for entry in entries {
        let Some(table) = entry.as_table() else {
            continue;
        };
        let Some(id) = toml_string(table, NETWORK_CONNECTION_ID_KEY) else {
            continue;
        };
        let Some(protocol) = toml_string(table, NETWORK_CONNECTION_PROTOCOL_KEY)
            .and_then(NetworkProtocol::from_config_value)
        else {
            continue;
        };
        let Some(uri) = toml_string(table, NETWORK_CONNECTION_URI_KEY) else {
            continue;
        };
        let label = table
            .get(NETWORK_CONNECTION_LABEL_KEY)
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if id.trim().is_empty() {
            continue;
        }
        let auto_connect = table
            .get(NETWORK_CONNECTION_AUTO_CONNECT_KEY)
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        if let Ok(connection) =
            NetworkConnection::new(NetworkConnectionId::new(id.trim()), label, protocol, uri)
        {
            connections.push(SavedNetworkConnection::new(connection, auto_connect));
        }
    }
    connections
}

fn toml_string<'a>(document: &'a toml::Table, key: &str) -> Option<&'a str> {
    document
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_favorites_and_skips_empty_paths() {
        let document = r#"
sidebar_favorites = [
    { label = "Projects", path = "/srv/projects" },
    { path = "" },
    { label = "  ", path = "/home/user/Downloads" },
]
"#
        .parse::<toml::Table>()
        .unwrap();

        let favorites = parse_toml_sidebar_favorites(&document).expect("favorites must parse");

        assert_eq!(favorites.len(), 2);
        assert_eq!(favorites[0].label, "Projects");
        assert_eq!(favorites[1].label, "Downloads");
    }

    #[test]
    fn network_connections_skip_invalid_entries_and_default_auto_connect() {
        let document = r#"
network_connections = [
    { id = "nas", label = "NAS", protocol = "smb", uri = "smb://server/share", auto_connect = true },
    { id = "", protocol = "smb", uri = "smb://server/other" },
    { id = "bad", protocol = "ftp", uri = "ftp://server" },
    { id = "dav", protocol = "webdav", uri = "davs://server" },
]
"#
        .parse::<toml::Table>()
        .unwrap();

        let connections = parse_toml_network_connections(&document);

        assert_eq!(connections.len(), 2);
        assert_eq!(connections[0].connection.id.as_str(), "nas");
        assert!(connections[0].auto_connect);
        assert!(!connections[1].auto_connect);
        assert_eq!(connections[1].connection.id.as_str(), "dav");
    }
}
