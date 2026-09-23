//! 窗口 chrome 域模型已下沉共享 crate `bennu-theme::window_controls`
//! （预览浮动窗与设置窗口共用，随预览视图迁移一并同源）。本模块只保留
//! 原路径重导出，crate 内调用点零改动。

pub(crate) use bennu_theme::window_controls::{
    WindowChromeLayout, WindowControlKind, WindowControlMoveDirection, WindowControlPlacement,
    WindowControlSide, WindowControlVisibility, WindowControlsConfig, WindowFrameState,
    MAIN_TOOLBAR_ROW_HEIGHT, WINDOW_TITLE_BAR_HEIGHT, WINDOW_TOP_BAR_HEIGHT,
};
