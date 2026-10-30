//! 文件选择器的侧边栏设置：只读主程序状态库里收藏与网络连接两个键，两边侧边栏显示同一份。
//! 设置文档的其余键与这里无关；备份与提示归主程序启动加载。

use std::path::Path;

use bennu_sidebar::{
    network_connections_from_stored, sidebar_favorites_from_stored, SidebarConfig,
    StoredNetworkConnection, StoredSidebarFavorite,
};
use bennu_state::{StoreResult, UserSettingsStore};
use serde::{Deserialize, Serialize};

/// 设置文档里侧边栏用到的两个键。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SidebarSettings {
    sidebar_favorites: Option<Vec<StoredSidebarFavorite>>,
    network_connections: Vec<StoredNetworkConnection>,
}

/// 文件选择器每次打开窗口读一次：只读，不备份、不写。还没有设置行时为默认——没设收藏，
/// 由调用方回落默认用户目录与 GTK 书签。
pub(crate) fn read_sidebar_config(state_database_path: &Path) -> StoreResult<SidebarConfig> {
    let Some(read) = UserSettingsStore::new(state_database_path)?
        .read_user_preferences(&SidebarSettings::default())?
    else {
        return Ok(SidebarConfig::default());
    };
    let preferences = read.preferences;
    Ok(SidebarConfig {
        favorites: preferences
            .sidebar_favorites
            .as_deref()
            .map(sidebar_favorites_from_stored),
        network_connections: network_connections_from_stored(&preferences.network_connections),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bennu_sidebar::SidebarFavoriteConfig;
    use bennu_state::StoredPath;

    use super::*;

    fn stored_connection(id: &str, protocol: &str, uri: &str) -> StoredNetworkConnection {
        StoredNetworkConnection {
            id: id.to_owned(),
            label: format!(" {id} "),
            protocol: protocol.to_owned(),
            uri: uri.to_owned(),
            auto_connect: true,
        }
    }

    #[test]
    fn the_sidebar_shows_the_favorites_and_connections_saved_by_the_main_program() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite");
        UserSettingsStore::new(&database)
            .unwrap()
            .replace_user_preferences(&SidebarSettings {
                sidebar_favorites: Some(vec![StoredSidebarFavorite {
                    label: "Projects".to_owned(),
                    path: StoredPath::from_path(Path::new("/srv/projects")),
                }]),
                network_connections: vec![
                    stored_connection("nas", "smb", "smb://server/share"),
                    stored_connection("bad", "ftp", "ftp://server"),
                ],
            })
            .unwrap();

        let config = read_sidebar_config(&database).unwrap();

        assert_eq!(
            config.favorites,
            Some(vec![SidebarFavoriteConfig {
                label: "Projects".to_owned(),
                path: PathBuf::from("/srv/projects"),
            }])
        );
        assert_eq!(config.network_connections.len(), 1);
        assert_eq!(config.network_connections[0].connection.id.as_str(), "nas");
        assert_eq!(config.network_connections[0].connection.label, "nas");
        assert!(config.network_connections[0].auto_connect);
    }

    #[test]
    fn without_saved_settings_the_sidebar_uses_its_defaults() {
        let directory = tempfile::tempdir().unwrap();

        let config = read_sidebar_config(&directory.path().join("state.sqlite")).unwrap();

        assert_eq!(config, SidebarConfig::default());
        assert_eq!(config.favorites, None);
    }
}
