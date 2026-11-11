//! 本进程窗口（选择窗、预览窗）的外壳注入：窗口按钮、顶栏拖动 / 双击最大化、
//! 边缘缩放都发通用窗口消息，由 `window_frames` 处理。双击顶栏和最大化按钮是
//! 同一个动作，发同一条消息。

use iced::window;

use bennu_preview::window_shell::WindowShell;
use bennu_theme::window_controls::{WindowControlKind, WindowControlsConfig};

use crate::{Message, PickerDaemon};

/// portal 没有自己的窗口按钮设置，用默认布局（与主程序默认一致）。
static WINDOW_CONTROLS: std::sync::LazyLock<WindowControlsConfig> =
    std::sync::LazyLock::new(WindowControlsConfig::default);

pub(crate) fn window_shell(daemon: &PickerDaemon, window: window::Id) -> WindowShell<'_, Message> {
    WindowShell {
        config: &WINDOW_CONTROLS,
        sidebar_style: daemon.look.sidebar_style,
        window,
        frame_state: daemon.frames.frame_state(window),
        on_control: Box::new(window_control_message),
        title_drag: Message::WindowDragRequested { window },
        title_double_click: window_control_message(WindowControlKind::MaximizeRestore, window),
        on_resize: Box::new(move |direction| Message::WindowResizeEdgePressed {
            window,
            direction,
        }),
    }
}

fn window_control_message(kind: WindowControlKind, window: window::Id) -> Message {
    Message::WindowControl { window, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_buttons_map_to_the_generic_window_control() {
        let window = window::Id::unique();
        for kind in WindowControlKind::ALL {
            assert!(matches!(
                window_control_message(kind, window),
                Message::WindowControl { window: target, kind: sent }
                    if target == window && sent == kind
            ));
        }
    }
}
