//! 无系统标题栏窗口的窗口管理（选择窗与预览窗共用）：窗口按钮、边缘缩放，以及
//! 各窗口的最大化状态。窗口自己画顶栏以后，这些就是
//! “每个窗口都有”的事，不属于某个功能模块。

use std::collections::HashSet;

use iced::{window, Task};

use bennu_theme::window_controls::{WindowControlKind, WindowFrameState};

use crate::{Message, PickerDaemon};

#[derive(Debug, Default)]
pub(crate) struct WindowFrames {
    maximized: HashSet<window::Id>,
}

impl WindowFrames {
    pub(crate) fn frame_state(&self, window: window::Id) -> WindowFrameState {
        if self.maximized.contains(&window) {
            WindowFrameState::Maximized
        } else {
            WindowFrameState::Restored
        }
    }

    /// 窗口关了：迟到的最大化观测不能让它复活。
    pub(crate) fn forget(&mut self, window: window::Id) {
        self.maximized.remove(&window);
    }

    fn observe(&mut self, window: window::Id, maximized: bool) {
        if maximized {
            self.maximized.insert(window);
        } else {
            self.maximized.remove(&window);
        }
    }
}

pub(crate) fn handle_window_control(
    daemon: &mut PickerDaemon,
    window: window::Id,
    kind: WindowControlKind,
) -> Task<Message> {
    match kind {
        WindowControlKind::Minimize => window::minimize(window, true),
        WindowControlKind::MaximizeRestore => toggle_maximized(window),
        // 预览窗关闭要收尾预览会话、把焦点还给选择窗；选择窗关闭走 WindowClosed
        // （会话据此回复“取消”）。
        WindowControlKind::Close if daemon.preview.is_preview_window(window) => {
            daemon.preview.close_focused_preview()
        }
        WindowControlKind::Close => window::close(window),
    }
}

pub(crate) fn handle_resize_edge_pressed(
    daemon: &PickerDaemon,
    window: window::Id,
    direction: window::Direction,
) -> Task<Message> {
    match daemon.frames.frame_state(window) {
        WindowFrameState::Restored => window::drag_resize(window, direction),
        WindowFrameState::Maximized => Task::none(),
    }
}

pub(crate) fn accept_maximize_observed(
    daemon: &mut PickerDaemon,
    window: window::Id,
    maximized: bool,
) {
    // 只接受仍然开着的窗口：关窗后的迟到回信直接丢。
    if daemon.windows.contains_key(&window) || daemon.preview.is_preview_window(window) {
        daemon.frames.observe(window, maximized);
    }
}

fn toggle_maximized(window: window::Id) -> Task<Message> {
    window::toggle_maximize(window).chain(
        window::is_maximized(window)
            .map(move |maximized| Message::WindowMaximizeObserved { window, maximized }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_state_follows_observations_until_forgotten() {
        let mut frames = WindowFrames::default();
        let window = window::Id::unique();
        assert_eq!(frames.frame_state(window), WindowFrameState::Restored);

        frames.observe(window, true);
        assert_eq!(frames.frame_state(window), WindowFrameState::Maximized);

        frames.forget(window);
        assert_eq!(frames.frame_state(window), WindowFrameState::Restored);
    }

    #[test]
    fn close_button_on_the_preview_window_ends_the_preview_session() {
        let mut daemon = crate::test_daemon();
        let _ = daemon.preview.request_preview(
            None,
            Some(&crate::preview_host::PreviewSelection {
                path: "/tmp/a.txt".into(),
                kind: file_core::FileKind::File,
                file_bytes: 16,
                on_network: false,
            }),
        );
        let preview_window = daemon.preview.engine.preview_window.unwrap();

        // 别的窗口（选择窗）的关闭按钮不碰预览会话。
        let _ = handle_window_control(&mut daemon, window::Id::unique(), WindowControlKind::Close);
        assert_eq!(daemon.preview.engine.preview_window, Some(preview_window));

        let _ = handle_window_control(&mut daemon, preview_window, WindowControlKind::Close);
        assert!(daemon.preview.engine.preview_window.is_none());
        assert!(daemon.preview.engine.preview.is_none());
    }

    #[test]
    fn late_maximize_observation_for_a_closed_window_is_dropped() {
        let mut daemon = crate::test_daemon();
        let closed = window::Id::unique();
        accept_maximize_observed(&mut daemon, closed, true);
        assert_eq!(
            daemon.frames.frame_state(closed),
            WindowFrameState::Restored
        );
    }
}
