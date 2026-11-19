use iced::{Point, Task};

use super::FileBrowser;
use crate::config;
use crate::model::Message;

#[derive(Debug, Clone, Copy)]
pub(super) struct SidebarResizeDrag {
    cursor_start_x: f32,
    width_start: f32,
}

impl FileBrowser {
    pub(super) fn start_sidebar_resize_drag(&mut self) -> Task<Message> {
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }

        self.sidebar_resize_drag = Some(SidebarResizeDrag {
            cursor_start_x: self.cursor_or_origin().x,
            width_start: self.sidebar_width_when_shown,
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.preview_engine.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    pub(super) fn update_sidebar_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.sidebar_resize_drag else {
            return;
        };

        self.sidebar_width_when_shown =
            self.sidebar_width_for_window(drag.width_start + position.x - drag.cursor_start_x);
    }

    pub(super) fn finish_sidebar_resize_drag_command(&mut self) -> Task<Message> {
        if self.sidebar_resize_drag.take().is_none() {
            return Task::none();
        }

        self.user_config.sidebar_width = self.sidebar_width_when_shown;
        self.persist_user_preferences_command()
    }

    pub(crate) fn sidebar_visible(&self) -> bool {
        self.user_config.sidebar_visible
    }

    /// 布局、命中区域用的侧栏宽度：侧栏隐藏时是 0，内容区从窗口左边开始。
    pub(crate) fn sidebar_layout_width(&self) -> f32 {
        if self.sidebar_visible() {
            self.sidebar_width_when_shown
        } else {
            0.0
        }
    }

    /// F9：隐藏 / 显示侧栏，重启后保持。隐藏不改宽度，再显示时还是原来那么宽。
    pub(super) fn toggle_sidebar(&mut self) -> Task<Message> {
        self.user_config.sidebar_visible = !self.user_config.sidebar_visible;
        self.sidebar_resize_drag = None;
        Task::batch([
            self.persist_user_preferences_command(),
            self.clamp_visible_pane_viewports(),
            self.schedule_thumbnail_refresh(),
        ])
    }

    pub(crate) fn sidebar_width_for_window(&self, width: f32) -> f32 {
        let max_sidebar_width = (self.main_window_width - config::MIN_COLUMN_WIDTH)
            .max(config::MIN_SIDEBAR_WIDTH)
            .min(config::MAX_SIDEBAR_WIDTH);
        config::normalize_sidebar_width(width).min(max_sidebar_width)
    }
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{key, Key, Modifiers};
    use iced::{event, Point};

    use super::*;
    use crate::model::BrowserPaneId;

    #[test]
    fn f9_hides_the_sidebar_for_layout_and_hit_testing_and_keeps_its_width() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.main_window_width = 1000.0;
        browser.main_window_height = 700.0;
        browser.sidebar_width_when_shown = 220.0;
        let inside_old_sidebar = Point::new(100.0, 400.0);
        assert_eq!(browser.pane_id_at_position(inside_old_sidebar), None);

        drop(browser.handle_keyboard_key_pressed(
            Key::Named(key::Named::F9),
            Modifiers::default(),
            event::Status::Ignored,
        ));

        assert!(!browser.sidebar_visible());
        assert_eq!(browser.sidebar_layout_width(), 0.0);
        assert_eq!(browser.main_content_width(), 1000.0);
        assert_eq!(
            browser.pane_id_at_position(inside_old_sidebar),
            Some(BrowserPaneId::PRIMARY)
        );
        assert!(!browser.user_config.user_preferences().sidebar_visible);

        drop(browser.toggle_sidebar());
        assert_eq!(browser.sidebar_layout_width(), 220.0);
    }
}
