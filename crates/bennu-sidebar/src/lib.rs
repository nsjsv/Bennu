//! 侧边栏数据单源：主程序 app-ui 与 FileChooser portal（portal-backend）
//! 共用同一份位置/收藏/GTK 书签计算、config.toml 侧边栏字段读取、设备与
//! 网络连接条目视图模型。本 crate 只承载纯数据与解析（spec 代码组织规则：
//! 不 import iced 运行时类型）；图标符号借用 bennu-theme::icons，视觉词汇
//! 单源不破。

pub mod config;
pub mod entries;
pub mod locations;
pub mod saved_connections;

pub use config::{
    parse_toml_network_connections, parse_toml_sidebar_favorites, read_sidebar_config,
    sidebar_config_path, SidebarConfig, SidebarFavoriteConfig,
};
pub use entries::{
    selected_sidebar_device, SidebarDeviceAction, SidebarDeviceEntry,
    SidebarNetworkConnectionAction, SidebarNetworkConnectionEntry, SIDEBAR_DEVICE_ICON_SYMBOL,
    SIDEBAR_NETWORK_CONNECTION_ICON_SYMBOL,
};
pub use locations::{
    home_sidebar_location, sidebar_favorite_configs, sidebar_icon_symbol, sidebar_locations,
    SidebarLocation, SidebarLocationKind,
};
pub use saved_connections::SavedNetworkConnection;

/// 侧边栏宽度契约：主程序持久化宽度与 portal 每窗独立宽度共用同一组
/// 默认值与夹取范围，两进程拖宽手感一致。
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 180.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 140.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 360.0;

pub fn normalize_sidebar_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
    } else {
        DEFAULT_SIDEBAR_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_width_clamps_into_bounds_and_rejects_non_finite() {
        assert_eq!(normalize_sidebar_width(200.0), 200.0);
        assert_eq!(normalize_sidebar_width(10.0), MIN_SIDEBAR_WIDTH);
        assert_eq!(normalize_sidebar_width(9999.0), MAX_SIDEBAR_WIDTH);
        assert_eq!(normalize_sidebar_width(f32::NAN), DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(
            normalize_sidebar_width(f32::INFINITY),
            DEFAULT_SIDEBAR_WIDTH
        );
    }
}
