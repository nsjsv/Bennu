use iced::window;

use crate::model::{Message, WindowControlKind, WindowControlsConfig, WindowFrameState};

use bennu_preview::window_shell::{WindowShell, HEADER_BAR_HEIGHT};
use bennu_theme::metrics::CONTROL_HEIGHT;

/// 工具栏排一行所需的宽度：三组按钮、搜索框、面板开关、默认在右边的三个窗口按钮、
/// 间距和顶栏内边距约 540，再给地址栏留 200。
const SINGLE_ROW_TOOLBAR_MIN_WIDTH: f32 = 740.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneNavigationLayout {
    SingleRow,
    StackedRows,
}

/// `toolbar_width` 是内容顶栏的宽度（窗口减侧栏）：工具栏只占内容那一栏，
/// 拿整窗宽度判断会在窄窗口里把它挤出顶栏。
pub(crate) fn pane_navigation_layout(toolbar_width: f32) -> PaneNavigationLayout {
    if toolbar_width < SINGLE_ROW_TOOLBAR_MIN_WIDTH {
        PaneNavigationLayout::StackedRows
    } else {
        PaneNavigationLayout::SingleRow
    }
}

/// 窄窗口工具栏折成两行时两行之间的距离。
pub(crate) const PANE_NAVIGATION_ROW_SPACING: f32 = 8.0;

/// 主窗口内容顶栏（工具栏）的高度：单行是标准顶栏高；折两行时多一行控件高 + 行距。
/// 窗格区起点、预览面板高度都按它算，和界面是同一个来源。
pub(crate) fn main_toolbar_height(toolbar_width: f32) -> f32 {
    match pane_navigation_layout(toolbar_width) {
        PaneNavigationLayout::SingleRow => HEADER_BAR_HEIGHT,
        PaneNavigationLayout::StackedRows => {
            HEADER_BAR_HEIGHT + CONTROL_HEIGHT + PANE_NAVIGATION_ROW_SPACING
        }
    }
}

/// 本进程窗口的外壳注入：窗口按钮发本进程的窗口管理消息，顶栏空白处可拖动、双击最大化。
pub(crate) fn window_shell<'a>(
    config: &'a WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
) -> WindowShell<'a, Message> {
    WindowShell {
        config,
        window,
        frame_state,
        on_control: Box::new(host_window_control_message),
        title_drag: Message::WindowDragRequested(window),
        title_double_click: Message::WindowMaximizeToggled(window),
        on_resize: Box::new(move |direction| Message::WindowResizeRequested(window, direction)),
    }
}

fn host_window_control_message(kind: WindowControlKind, window: window::Id) -> Message {
    match kind {
        WindowControlKind::Minimize => Message::WindowMinimizeRequested(window),
        WindowControlKind::MaximizeRestore => Message::WindowMaximizeToggled(window),
        WindowControlKind::Close => Message::AuxiliaryWindowCloseRequested(window),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_stacks_when_the_content_header_is_too_narrow_for_one_row() {
        assert_eq!(
            pane_navigation_layout(SINGLE_ROW_TOOLBAR_MIN_WIDTH - 1.0),
            PaneNavigationLayout::StackedRows
        );
        assert_eq!(
            pane_navigation_layout(SINGLE_ROW_TOOLBAR_MIN_WIDTH),
            PaneNavigationLayout::SingleRow
        );
        assert_eq!(
            main_toolbar_height(SINGLE_ROW_TOOLBAR_MIN_WIDTH - 1.0),
            HEADER_BAR_HEIGHT + CONTROL_HEIGHT + PANE_NAVIGATION_ROW_SPACING
        );
    }
}
