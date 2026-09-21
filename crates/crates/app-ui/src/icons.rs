//! lucide 图标已整体迁至共享 crate `bennu-theme::icons`（主程序与
//! portal-backend 共用同一份图标资产与扩展名映射）。本模块只保留
//! 原路径，把共享库重导出给 crate 内既有调用点。

pub(crate) use bennu_theme::icons::*;
