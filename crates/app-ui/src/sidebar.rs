//! 纯搬移：位置/收藏/GTK 书签解析已下沉共享 crate `bennu-sidebar`
//! （主程序与 FileChooser portal 共用同一实现）；re-export 维持
//! crate::sidebar::* 既有调用路径。SidebarLocation/SidebarLocationKind
//! 类型本体也随迁，经 model.rs re-export。

pub(crate) use bennu_sidebar::{
    home_sidebar_location, sidebar_favorite_configs, sidebar_icon_symbol, sidebar_locations,
};
