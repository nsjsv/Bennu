//! 主题系统已整体迁至共享 crate `bennu-theme`（主程序与 portal-backend
//! 共用的唯一主题来源）。本模块只保留原路径，把共享库重导出给 crate
//! 内既有调用点（`crate::matugen_theme::…`），调用点零改动。

pub(crate) use bennu_theme::*;
