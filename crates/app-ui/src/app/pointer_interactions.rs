use super::FileBrowser;
use iced::{mouse, window, Point, Task};

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

    pub(super) fn finish_pointer_drag_interactions(&mut self) -> Task<Message> {
        let tab_drag_finished = self.finish_tab_drag();
        let pane_drag_finished = self.finish_pane_drag();
        self.finish_batch_rename_preview_drag();
        self.finish_terminal_panel_pointer_interaction();
        // 任何窗口里的左键释放都结束图片平移；面板内释放由
        // mouse_area 的 PanEnded 先行处理，这里只是兜底。
        self.preview_engine.preview_image_viewport.panning = false;
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

    /// 这个窗口的指针移动怎样作为消息进来。每条消息都会重建所有窗口的界面并重画，所以只在
    /// 跟着指针走时才要：按住左键的窗口每次都要（拖动、改大小、框选都要按住）；预览画面按引擎
    /// 要的来（底部控制条只要进出底部那一条，全屏时只在控件藏着时要）。其余时候位置和时刻只记在
    /// `cursor_position`。
    pub(super) fn pointer_follow(
        &self,
        window: window::Id,
    ) -> bennu_theme::pointer_tracker::PointerFollow<'static, Message> {
        use bennu_preview::engine::PointerInterest;
        use bennu_theme::pointer_tracker::PointerFollow;

        let on_move = move |position| Message::CursorMoved { window, position };
        if self.pointer_held_window == Some(window) {
            return PointerFollow::Moves(Box::new(on_move));
        }
        let interest = if self.preview_engine.preview_window == Some(window) {
            self.preview_engine.preview_window_pointer_interest()
        } else if window == self.main_window && self.main_window_video_fullscreen {
            self.preview_engine.main_window_pointer_interest()
        } else {
            PointerInterest::Nothing
        };
        match interest {
            PointerInterest::Nothing => PointerFollow::Still,
            PointerInterest::Band { top, bottom } => PointerFollow::Band {
                top,
                bottom,
                on_move: Box::new(on_move),
            },
            PointerInterest::Moves => PointerFollow::Moves(Box::new(on_move)),
        }
    }

    pub(super) fn hold_pointer(&mut self, window: window::Id, button: mouse::Button) {
        if button == mouse::Button::Left {
            self.pointer_held_window = Some(window);
        }
    }

    pub(super) fn release_pointer(&mut self) {
        self.pointer_held_window = None;
    }

    pub(super) fn update_pointer_motion(
        &mut self,
        window: window::Id,
        position: Point,
    ) -> Task<Message> {
        if self.preview_engine.preview_window == Some(window) {
            self.cursor_position.set(Some(position));
            let preview_engine = &mut self.preview_engine;
            preview_engine.update_sqlite_tables_resize_drag(position);
            return preview_engine
                .preview_pointer_moved(position.y)
                .map(Message::Preview);
        }
        if window == self.main_window && self.main_window_video_fullscreen {
            return self
                .preview_engine
                .preview_pointer_moved(position.y)
                .map(Message::Preview);
        }
        if self.settings_window == Some(window) {
            return self.update_context_menu_settings_drag(position);
        }
        if window != self.main_window {
            return Task::none();
        }
        self.cursor_position.set(Some(position));
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
        // 面板边界拖拽逐帧改变窗格的宽高，视口跟着收。
        let pane_resize_command = if self.pane_boundary_drag_is_active() {
            self.clamp_visible_pane_viewports()
        } else {
            Task::none()
        };
        Task::batch([file_drag_command, selection_command, pane_resize_command])
    }

    fn pane_boundary_drag_is_active(&self) -> bool {
        self.sidebar_resize_drag.is_some()
            || self.split_resize_drag.is_some()
            || self.right_preview_panel_resize_drag.is_some()
            || self.terminal_panel.is_resizing()
    }
}

#[cfg(test)]
mod tests {
    use iced::{event, mouse};

    use super::*;
    use crate::config;

    fn follows_every_move(browser: &FileBrowser, window: window::Id) -> bool {
        matches!(
            browser.pointer_follow(window),
            bennu_theme::pointer_tracker::PointerFollow::Moves(_)
        )
    }

    /// 主窗口只在按住左键期间把指针移动作为消息收进来；右键、松开之后都只静默记录。
    #[test]
    fn only_a_held_left_button_makes_the_main_window_follow_the_pointer() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let main = browser.main_window;
        let press = |button| Message::WindowPointerPressed {
            window: main,
            button,
            status: event::Status::Ignored,
        };
        assert!(!follows_every_move(&browser, main));

        drop(browser.update(press(mouse::Button::Right)));
        assert!(!follows_every_move(&browser, main));

        drop(browser.update(press(mouse::Button::Left)));
        assert!(follows_every_move(&browser, main));

        drop(browser.update(Message::WindowPointerReleased(event::Status::Ignored)));
        assert!(!follows_every_move(&browser, main));
    }

    /// 预览窗口里没有底部控制条（文本这类预览）时，移动只静默记录，不让所有窗口重建。
    #[test]
    fn a_preview_without_bottom_controls_does_not_follow_the_pointer() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let preview = window::Id::unique();
        browser.preview_engine.preview_window = Some(preview);
        assert!(matches!(
            browser.pointer_follow(preview),
            bennu_theme::pointer_tracker::PointerFollow::Still
        ));
    }
}
