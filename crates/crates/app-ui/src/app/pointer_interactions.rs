use super::FileBrowser;
use iced::{window, Point, Task};

use crate::model::Message;

impl FileBrowser {
    pub(super) fn clear_pointer_driven_interaction_state(&mut self) {
        self.tab_drag = None;
        self.pane_drag = None;
        self.pane_drag_pointer_press = None;
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.drag_selection_anchor = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.sidebar_resize_drag = None;
        self.right_preview_panel_resize_drag = None;
        self.right_preview_ratio_resize_drag = None;
        self.column_resize_drag = None;
        self.list_column_resize_drag = None;
        self.list_column_reorder_drag = None;
        self.split_resize_drag = None;
        self.hovered_list_header_column = None;
    }

    pub(super) fn finish_pointer_drag_interactions(&mut self, window: window::Id) -> Task<Message> {
        let tab_drag_finished = self.finish_tab_drag();
        let pane_drag_finished = self.finish_pane_drag();
        self.finish_batch_rename_preview_drag();
        self.finish_terminal_panel_pointer_interaction();
        // 任何窗口里的左键释放都结束图片平移；面板内释放由
        // mouse_area 的 PanEnded 先行处理，这里只是兜底。
        self.preview_image_viewport.panning = false;
        if self.preview_window == Some(window) && self.preview_window_drag_active {
            self.preview_window_drag_active = false;
            if let Some(pointer_y) = self.preview_window_pointer_y {
                self.preview_window_chrome.update_for_cursor_y(pointer_y);
                self.refresh_preview_window_bottom_controls();
            } else {
                self.preview_window_chrome.start_hide();
                self.preview_window_bottom_controls.start_hide();
            }
        }
        Task::batch([
            tab_drag_finished,
            pane_drag_finished,
            self.finish_context_menu_settings_drag(),
            self.finish_sidebar_bookmark_drag(),
            self.finish_sidebar_resize_drag_command(),
            self.finish_right_preview_panel_drag_commands(),
            self.finish_column_resize_drag_command(),
            self.finish_sqlite_tables_resize_drag(),
            self.finish_list_column_resize_drag_command(),
            self.finish_list_column_reorder_drag_command(),
            self.finish_split_resize(),
            self.finish_drag_selection(None),
            self.schedule_thumbnail_refresh(),
            self.request_breadcrumb_drop_target_bounds_measurement(),
        ])
    }

    pub(super) fn update_pointer_motion(
        &mut self,
        window: window::Id,
        position: Point,
    ) -> Task<Message> {
        if self.preview_window == Some(window) {
            if self.preview_window_uses_window_chrome() {
                self.cursor_position = position;
                self.update_sqlite_tables_resize_drag(position);
                return Task::none();
            }
            self.cancel_preview_window_initial_chrome_hide();
            self.preview_window_pointer_y = Some(position.y);
            self.refresh_preview_window_bottom_controls();
            if !self.preview_window_drag_active {
                self.preview_window_chrome.update_for_cursor_y(position.y);
            }
            return Task::none();
        }
        if self.settings_window == Some(window) {
            return self.update_context_menu_settings_drag(position);
        }
        if window != self.main_window {
            return Task::none();
        }
        self.cursor_position = position;
        self.promote_ctrl_shift_pane_drag_from_active_pointer_drag();
        self.update_tab_drag(position);
        self.update_pane_drag(position);
        let file_drag_command = self.update_file_drag(position);
        self.update_file_drag_edge_scroll(position);
        self.update_sidebar_bookmark_drag(position);
        self.update_sidebar_resize_drag(position);
        self.update_right_preview_panel_resize_drag(position);
        self.update_right_preview_panel_ratio_resize_drag(position);
        self.update_column_resize_drag(position);
        self.update_list_column_resize_drag(position);
        self.update_split_resize(position);
        self.update_list_column_reorder_drag(position);
        self.update_terminal_panel_resize_drag(position);
        self.update_terminal_tab_drag(position);
        let selection_command = if self.update_selection_marquee(position) {
            crate::column_entry_bounds::column_entry_bounds_command()
        } else {
            Task::none()
        };
        Task::batch([file_drag_command, selection_command])
    }
}
