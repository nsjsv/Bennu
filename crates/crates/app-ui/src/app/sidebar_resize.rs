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
            cursor_start_x: self.cursor_position.x,
            width_start: self.sidebar_width,
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    pub(super) fn update_sidebar_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.sidebar_resize_drag else {
            return;
        };

        self.sidebar_width =
            self.sidebar_width_for_window(drag.width_start + position.x - drag.cursor_start_x);
    }

    pub(super) fn finish_sidebar_resize_drag_command(&mut self) -> Task<Message> {
        if self.sidebar_resize_drag.take().is_none() {
            return Task::none();
        }

        self.user_config.sidebar_width = self.sidebar_width;
        self.persist_user_preferences_command()
    }

    pub(crate) fn sidebar_width_for_window(&self, width: f32) -> f32 {
        let max_sidebar_width = (self.main_window_width - config::MIN_COLUMN_WIDTH)
            .max(config::MIN_SIDEBAR_WIDTH)
            .min(config::MAX_SIDEBAR_WIDTH);
        config::normalize_sidebar_width(width).min(max_sidebar_width)
    }
}
