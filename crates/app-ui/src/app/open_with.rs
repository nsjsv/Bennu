use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::commands::{open_with_application_command, open_with_applications_command};
use crate::model::Message;
use crate::open_with::OpenWithState;

impl FileBrowser {
    pub(super) fn request_open_with_applications(&mut self, path: PathBuf) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        self.open_with = Some(OpenWithState::loading(path.clone(), None));
        self.prepare_open_with_floating_state();
        Task::batch([rename_command, open_with_applications_command(path)])
    }

    pub(super) fn request_open_with_after_default_open_failed(
        &mut self,
        path: PathBuf,
        fallback_error: String,
    ) -> Task<Message> {
        self.open_with = Some(OpenWithState::loading(path.clone(), Some(fallback_error)));
        self.prepare_open_with_floating_state();
        open_with_applications_command(path)
    }

    pub(super) fn accept_open_with_applications(
        &mut self,
        path: PathBuf,
        applications: Result<desktop_linux::OpenWithApplicationList, String>,
    ) -> Task<Message> {
        let Some(open_with) = self
            .open_with
            .as_mut()
            .filter(|open_with| *open_with.path() == path)
        else {
            return Task::none();
        };

        match applications {
            Ok(applications) => {
                if open_with.accept_application_list(applications) {
                    self.clear_global_error();
                }
            }
            Err(error) => {
                let fallback_error = open_with.fallback_error().map(str::to_owned);
                self.open_with = None;
                self.show_global_error(open_with_error_with_fallback(
                    fallback_error.as_deref(),
                    &error,
                ));
            }
        }
        Task::none()
    }

    pub(super) fn toggle_open_with_default_application(&mut self, selected: bool) -> Task<Message> {
        if let Some(open_with) = &mut self.open_with {
            open_with.select_default_application_setting(selected);
        }
        Task::none()
    }

    pub(super) fn select_open_with_application(&mut self, desktop_id: String) -> Task<Message> {
        let Some(open_with) = self.open_with.as_ref() else {
            return Task::none();
        };
        let path = open_with.path().clone();
        let launch_mode = open_with.launch_mode();
        self.open_with = None;
        self.clear_global_error();
        open_with_application_command(path, desktop_id, launch_mode)
    }

    pub(super) fn accept_open_with_application_finished(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        match result {
            Ok(()) => self.clear_global_error(),
            Err(error) => self.show_global_error(error),
        }
        Task::none()
    }

    fn prepare_open_with_floating_state(&mut self) {
        self.context_menu = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.operation_queue.close_panel();
        let _ = self.cancel_address_editing();
        self.clear_pointer_driven_interaction_state();
        self.shortcut_capture = None;
    }
}

fn open_with_error_with_fallback(fallback_error: Option<&str>, open_with_error: &str) -> String {
    match fallback_error {
        Some(fallback_error) => format!("{fallback_error}; {open_with_error}"),
        None => open_with_error.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use iced::Point;

    use super::*;
    use crate::app::column_resize::ColumnResizeDrag;
    use crate::config::ui_thread_startup_config;
    use crate::model::{
        FileDragNativeDndState, FileDragPhase, FileDragState, PaneDragState, SelectionMarquee,
        SelectionMarqueePhase, SelectionMarqueeSource, SidebarBookmarkDragState,
        SidebarBookmarkDropSlot, TabDragMode, TabDragState,
    };

    #[test]
    fn open_with_clears_pointer_driven_interaction_state() {
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        let path = PathBuf::from("/workspace/file.txt");

        let _ = browser.start_sidebar_resize_drag();
        browser.column_resize_drag = Some(ColumnResizeDrag {
            column_index: 0,
            cursor_start_x: 0.0,
            width_start: 240.0,
            content_width_start: 720.0,
        });
        browser.tab_drag = Some(TabDragState {
            source_pane_id: browser.active_pane_id(),
            tab_id: browser.active_tab_id,
            phase: FileDragPhase::Dragging,
            mode: TabDragMode::Reorder,
            split_target: None,
        });
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: browser.active_pane_id(),
            phase: FileDragPhase::Dragging,
            target: None,
        });
        browser.file_drag = Some(FileDragState {
            gesture_id: crate::model::FileDragGestureId(1),
            source_pane_id: browser.active_pane_id(),
            source_tab_id: browser.active_tab_id,
            sources: vec![path.clone()],
            pressed_path: path.clone(),
            bookmark_source: None,
            stationary_action: crate::model::FileDragStationaryAction::SelectionOnly,
            phase: FileDragPhase::Dragging,
            native_dnd: FileDragNativeDndState::NotRequested,
            column_directories_snapshot: Vec::new(),
            press_origin: iced::Point::ORIGIN,
            preview_entries: Vec::new(),
        });
        browser.selection_marquee = Some(SelectionMarquee {
            gesture_origin: Point::new(0.0, 0.0),
            start: Point::new(0.0, 0.0),
            current: Point::new(10.0, 10.0),
            source: SelectionMarqueeSource::PaneBlank,
            phase: SelectionMarqueePhase::Selecting,
            scroll_anchor: crate::model::SelectionMarqueeScrollAnchor::List {
                pane_id: browser.active_pane_id(),
                offset_y: 0.0,
            },
            base_selection: HashSet::new(),
            preserve_existing: false,
        });
        browser.drag_selection_anchor = Some(path.clone());
        browser.sidebar_bookmark_drag = Some(SidebarBookmarkDragState {
            path: path.clone(),
            origin: Point::new(0.0, 0.0),
            source_index: 0,
            phase: FileDragPhase::Dragging,
            order_changed: false,
        });
        browser.sidebar_bookmark_drop_slot = Some(SidebarBookmarkDropSlot::Insert { index: 0 });

        let _ = browser.request_open_with_applications(path);

        assert!(browser.tab_drag.is_none());
        assert!(browser.pane_drag.is_none());
        assert!(browser.file_drag.is_none());
        assert!(browser.selection_marquee.is_none());
        assert!(browser.drag_selection_anchor.is_none());
        assert!(browser.sidebar_bookmark_drag.is_none());
        assert!(browser.sidebar_bookmark_drop_slot.is_none());
        assert!(browser.sidebar_resize_drag.is_none());
        assert!(browser.column_resize_drag.is_none());
    }
}
