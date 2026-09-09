use iced::{Point, Task};

use super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use crate::model::{
    BrowserPaneId, ContextMenuState, FileDragPhase, ListColumnKind, ListColumnMenuState, Message,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct ListColumnResizeDrag {
    pub(super) kind: ListColumnKind,
    pub(super) cursor_start_x: f32,
    pub(super) width_start: f32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ListColumnReorderDrag {
    kind: ListColumnKind,
    phase: FileDragPhase,
    drop_target: Option<ListColumnKind>,
}

impl ListColumnReorderDrag {
    fn is_dragging(self) -> bool {
        matches!(self.phase, FileDragPhase::Dragging)
    }
}

impl FileBrowser {
    pub(crate) fn list_column_being_reordered(&self) -> Option<ListColumnKind> {
        let drag = self.list_column_reorder_drag?;
        drag.is_dragging().then_some(drag.kind)
    }

    pub(crate) fn list_column_reorder_insertion_target(&self) -> Option<ListColumnKind> {
        let drag = self.list_column_reorder_drag?;
        if drag.is_dragging() {
            drag.drop_target
        } else {
            None
        }
    }

    pub(crate) fn hovered_list_header_column(
        &self,
        pane_id: BrowserPaneId,
    ) -> Option<ListColumnKind> {
        self.hovered_list_header_column
            .filter(|(hovered_pane_id, _)| *hovered_pane_id == pane_id)
            .map(|(_, column)| column)
    }

    pub(super) fn select_list_sort_column(&mut self, column: ListColumnKind) -> Task<Message> {
        let before_sort = self.user_config.list_view_preferences.sort();
        self.user_config
            .list_view_preferences
            .select_sort_column(column);
        let after_sort = self.user_config.list_view_preferences.sort();
        if before_sort == after_sort {
            return Task::none();
        }

        self.options.sort_field = after_sort.field;
        self.options.sort_direction = after_sort.direction;
        Task::batch([
            self.persist_user_preferences_command(),
            self.reload_visible_panes_preserving_list_directory_summaries(),
        ])
    }

    pub(super) fn open_list_column_menu(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        self.activate_pane(pane_id);
        self.clear_list_header_hover_in_pane(pane_id);
        let rename_command = self.commit_rename_if_active();
        self.clear_preview();
        self.operation_queue.close_panel();
        self.open_with = None;
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.drag_selection_anchor = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        let _ = self.cancel_address_editing();
        self.context_menu = Some(ContextMenuState::ListColumns(ListColumnMenuState {
            position: self.cursor_position,
        }));
        rename_command
    }

    pub(super) fn toggle_list_column_visibility(
        &mut self,
        column: ListColumnKind,
    ) -> Task<Message> {
        let before_sort = self.user_config.list_view_preferences.sort();
        let visible = self
            .user_config
            .list_view_preferences
            .columns()
            .iter()
            .find(|config| config.kind == column)
            .map_or(true, |config| !config.visible);
        self.user_config
            .list_view_preferences
            .set_column_visible(column, visible);
        let after_sort = self.user_config.list_view_preferences.sort();
        self.options.sort_field = after_sort.field;
        self.options.sort_direction = after_sort.direction;
        if before_sort == after_sort {
            self.persist_user_preferences_command()
        } else {
            Task::batch([
                self.persist_user_preferences_command(),
                self.reload_visible_panes_preserving_list_directory_summaries(),
            ])
        }
    }

    pub(super) fn start_list_column_resize_drag(
        &mut self,
        pane_id: BrowserPaneId,
        column: ListColumnKind,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }

        let width_start = self
            .user_config
            .list_view_preferences
            .columns()
            .iter()
            .find(|config| config.kind == column)
            .map(|config| config.width)
            .unwrap_or(0.0);
        self.list_column_resize_drag = Some(ListColumnResizeDrag {
            kind: column,
            cursor_start_x: self.cursor_position.x,
            width_start,
        });
        self.list_column_reorder_drag = None;
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    pub(super) fn start_list_column_reorder_drag(
        &mut self,
        pane_id: BrowserPaneId,
        column: ListColumnKind,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }

        self.list_column_reorder_drag = Some(ListColumnReorderDrag {
            kind: column,
            phase: FileDragPhase::WaitingForMovement {
                origin: self.cursor_position,
            },
            drop_target: None,
        });
        self.list_column_resize_drag = None;
        self.cancel_file_drag_interaction();
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    pub(super) fn update_list_column_reorder_drag(&mut self, position: Point) {
        let Some(drag) = &mut self.list_column_reorder_drag else {
            return;
        };
        let FileDragPhase::WaitingForMovement { origin } = drag.phase else {
            return;
        };
        let delta_x = position.x - origin.x;
        let delta_y = position.y - origin.y;
        if delta_x * delta_x + delta_y * delta_y
            >= POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE
        {
            drag.phase = FileDragPhase::Dragging;
        }
    }

    pub(super) fn enter_list_header_column(
        &mut self,
        pane_id: BrowserPaneId,
        target: ListColumnKind,
    ) -> Task<Message> {
        self.hovered_list_header_column = Some((pane_id, target));
        self.enter_list_column_reorder_target(target)
    }

    fn enter_list_column_reorder_target(&mut self, target: ListColumnKind) -> Task<Message> {
        let Some(drag) = self.list_column_reorder_drag.as_mut() else {
            return Task::none();
        };
        drag.drop_target = (target != drag.kind).then_some(target);
        Task::none()
    }

    pub(super) fn exit_list_header_column(
        &mut self,
        pane_id: BrowserPaneId,
        target: ListColumnKind,
    ) -> Task<Message> {
        if self.hovered_list_header_column == Some((pane_id, target)) {
            self.hovered_list_header_column = None;
        }
        self.exit_list_column_reorder_target(target)
    }

    fn exit_list_column_reorder_target(&mut self, target: ListColumnKind) -> Task<Message> {
        let Some(drag) = self.list_column_reorder_drag.as_mut() else {
            return Task::none();
        };
        if drag.drop_target == Some(target) {
            drag.drop_target = None;
        }
        Task::none()
    }

    pub(super) fn clear_list_header_hover_in_pane(&mut self, pane_id: BrowserPaneId) {
        if self.hovered_list_header_column.map(|(id, _)| id) == Some(pane_id) {
            self.hovered_list_header_column = None;
        }
    }

    pub(super) fn finish_list_column_reorder_drag_command(&mut self) -> Task<Message> {
        let Some(drag) = self.list_column_reorder_drag.take() else {
            return Task::none();
        };
        if matches!(drag.phase, FileDragPhase::WaitingForMovement { .. }) {
            self.select_list_sort_column(drag.kind)
        } else if let Some(target) = drag.drop_target {
            if self
                .user_config
                .list_view_preferences
                .move_column_to(drag.kind, target)
            {
                self.persist_user_preferences_command()
            } else {
                Task::none()
            }
        } else {
            Task::none()
        }
    }

    pub(super) fn update_list_column_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.list_column_resize_drag else {
            return;
        };

        let resized_width = drag.width_start + position.x - drag.cursor_start_x;
        self.user_config
            .list_view_preferences
            .set_column_width(drag.kind, resized_width);
    }

    pub(super) fn finish_list_column_resize_drag_command(&mut self) -> Task<Message> {
        if self.list_column_resize_drag.take().is_some() {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

    use super::*;
    use crate::config;
    use crate::model::{
        BrowserPane, BrowserPaneLayout, BrowserTab, BrowserViewMode, ColumnBrowserViewport,
        ExpandedDirectory, ExpandedDirectoryStatus, ListDirectorySummary, SplitAxis,
    };
    use crate::thumbnail_cache::ColumnViewport;

    fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            kind,
            EntryMetadata {
                len: 0,
                modified: None,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    fn loaded_directory(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
        ExpandedDirectory {
            entries,
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        }
    }

    fn pane_from_tab_for_test(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
        BrowserPane {
            id: pane_id,
            current_dir: tab.directory.clone(),
            is_trash_view: tab.is_trash_view,
            entries: tab.entries.clone(),
            directory_discovery: tab.directory_discovery.clone(),
            directory_loading_placeholder: None,
            trash_entries: tab.trash_entries.clone(),
            selected: tab.selected.clone(),
            selected_paths: tab.selected_paths.clone(),
            selection_anchor: tab.selection_anchor.clone(),
            deepest_open_column_directory: tab.deepest_open_column_directory.clone(),
            expanded_directories: tab.expanded_directories.clone(),
            view_mode: tab.view_mode,
            column_browser_viewport: ColumnBrowserViewport::default(),
            column_viewports: HashMap::<PathBuf, ColumnViewport>::new(),
            tabs: vec![tab.clone()],
            active_tab_id: tab.id,
            directory_load_generation: 0,
            directory_load_cancel: None,
            back_stack: tab.back_stack.clone(),
            forward_stack: tab.forward_stack.clone(),
            directory_collection_phase: crate::model::DirectoryCollectionPhase::Ready,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        }
    }

    fn remember_summary(browser: &mut FileBrowser, path: &Path, count: usize, size: u64) {
        browser
            .list_directory_summary_cache
            .remember_direct_child_count(path.to_path_buf(), count);
        let request = browser
            .list_directory_summary_cache
            .start_request(path.to_path_buf(), true)
            .expect("recursive request");
        assert!(browser.list_directory_summary_cache.store_summary(
            &request,
            ListDirectorySummary {
                direct_child_count: count,
                recursive_total_size_bytes: Some(size),
                files_total_size_bytes: Some(size),
                hidden_files_total_size_bytes: Some(0),
            }
        ));
    }

    #[test]
    fn selecting_list_sort_column_updates_scan_options() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.select_list_sort_column(ListColumnKind::Size));

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);

        drop(browser.select_list_sort_column(ListColumnKind::Size));

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Descending);
    }

    #[test]
    fn hiding_current_list_sort_column_updates_scan_options() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.select_list_sort_column(ListColumnKind::Size));
        drop(browser.toggle_list_column_visibility(ListColumnKind::Size));

        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
    }

    #[test]
    fn selecting_list_sort_column_reloads_visible_panes_without_invalidating_directory_summaries() {
        let active_root = PathBuf::from("/workspace/active");
        let active_dir = active_root.join("project");
        let active_child = active_dir.join("src");
        let inactive_root = PathBuf::from("/workspace/inactive");
        let inactive_dir = inactive_root.join("docs");
        let inactive_child = inactive_dir.join("guides");
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        browser.current_dir = active_root.clone();
        browser.view_mode = BrowserViewMode::List;
        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
        browser.entries = vec![test_entry(active_dir.clone(), FileKind::Directory)].into();
        browser.expanded_directories.insert(
            active_dir.clone(),
            loaded_directory(vec![test_entry(active_child.clone(), FileKind::Directory)]),
        );

        let mut active_tab = BrowserTab::directory(0, active_root);
        active_tab.view_mode = BrowserViewMode::List;
        active_tab.entries = browser.entries.clone();
        active_tab.expanded_directories = browser.expanded_directories.clone();
        active_tab.selected_paths = HashSet::new();
        browser.tabs = vec![active_tab.clone()];
        browser.active_tab_id = active_tab.id;

        let mut inactive_tab = BrowserTab::directory(1, inactive_root);
        inactive_tab.view_mode = BrowserViewMode::List;
        inactive_tab.entries = vec![test_entry(inactive_dir.clone(), FileKind::Directory)].into();
        inactive_tab.expanded_directories.insert(
            inactive_dir.clone(),
            loaded_directory(vec![test_entry(
                inactive_child.clone(),
                FileKind::Directory,
            )]),
        );

        browser.pane_layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 500,
        };
        browser.panes = vec![
            pane_from_tab_for_test(BrowserPaneId::PRIMARY, active_tab),
            pane_from_tab_for_test(BrowserPaneId(1), inactive_tab),
        ];

        for (path, count, size) in [
            (&active_dir, 2usize, 2048u64),
            (&active_child, 1usize, 1024u64),
            (&inactive_dir, 3usize, 4096u64),
            (&inactive_child, 2usize, 512u64),
        ] {
            remember_summary(&mut browser, path, count, size);
        }

        drop(browser.select_list_sort_column(ListColumnKind::Size));

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
        assert_eq!(browser.directory_load_generation, 1);
        assert!(browser.directory_collection_phase.is_discovering());
        assert_eq!(
            browser
                .expanded_directories
                .get(&active_dir)
                .expect("active expanded directory")
                .load_generation,
            1
        );
        assert!(matches!(
            browser
                .expanded_directories
                .get(&active_dir)
                .expect("active expanded directory")
                .status,
            ExpandedDirectoryStatus::Loading
        ));

        let inactive_pane = browser.pane_by_id(BrowserPaneId(1)).expect("inactive pane");
        assert_eq!(inactive_pane.directory_load_generation, 1);
        assert!(inactive_pane.directory_collection_phase.is_discovering());
        assert_eq!(
            inactive_pane
                .expanded_directories
                .get(&inactive_dir)
                .expect("inactive expanded directory")
                .load_generation,
            1
        );
        assert!(matches!(
            inactive_pane
                .expanded_directories
                .get(&inactive_dir)
                .expect("inactive expanded directory")
                .status,
            ExpandedDirectoryStatus::Loading
        ));

        for path in [&active_dir, &active_child, &inactive_dir, &inactive_child] {
            assert!(
                browser
                    .list_directory_summary_cache
                    .summary_for_path(path)
                    .is_some(),
                "expected cached summary for {} to survive sort reload",
                path.display()
            );
        }
    }

    #[test]
    fn hiding_current_list_sort_column_preserves_directory_summaries() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = PathBuf::from("/workspace");
        let directory = root.join("project");

        browser.current_dir = root;
        browser.view_mode = BrowserViewMode::List;
        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
        browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
        remember_summary(&mut browser, &directory, 2, 2048);

        drop(browser.select_list_sort_column(ListColumnKind::Size));
        drop(browser.toggle_list_column_visibility(ListColumnKind::Size));

        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&directory)
            .is_some());
    }

    #[test]
    fn list_header_hover_is_pane_scoped_and_ignores_stale_exit() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let secondary = BrowserPaneId(1);

        drop(browser.enter_list_header_column(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        assert_eq!(
            browser.hovered_list_header_column(BrowserPaneId::PRIMARY),
            Some(ListColumnKind::Size)
        );
        assert_eq!(browser.hovered_list_header_column(secondary), None);

        drop(browser.enter_list_header_column(BrowserPaneId::PRIMARY, ListColumnKind::Modified));
        drop(browser.exit_list_header_column(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        assert_eq!(
            browser.hovered_list_header_column(BrowserPaneId::PRIMARY),
            Some(ListColumnKind::Modified)
        );
        assert_eq!(browser.list_column_reorder_insertion_target(), None);

        drop(browser.exit_list_header_column(BrowserPaneId::PRIMARY, ListColumnKind::Modified));
        assert!(browser
            .hovered_list_header_column(BrowserPaneId::PRIMARY)
            .is_none());
    }

    #[test]
    fn clicking_list_header_without_drag_sorts_by_that_column() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Descending);

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
    }

    #[test]
    fn dragging_list_header_without_reorder_does_not_change_sort() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
    }

    #[test]
    fn dragging_list_header_keeps_column_order_stable_until_release() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Kind));
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));

        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
            ]
        );

        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Size,
                ListColumnKind::Modified,
                ListColumnKind::Kind,
            ]
        );
    }

    #[test]
    fn leaving_list_header_target_before_release_cancels_reorder() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));
        drop(browser.exit_list_column_reorder_target(ListColumnKind::Modified));
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
            ]
        );
    }

    #[test]
    fn list_column_reorder_feedback_only_appears_after_drag_activation() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));

        assert_eq!(browser.list_column_being_reordered(), None);
        assert_eq!(browser.list_column_reorder_insertion_target(), None);

        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));

        assert_eq!(browser.list_column_being_reordered(), None);
        assert_eq!(browser.list_column_reorder_insertion_target(), None);

        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });

        assert_eq!(
            browser.list_column_being_reordered(),
            Some(ListColumnKind::Size)
        );
        assert_eq!(
            browser.list_column_reorder_insertion_target(),
            Some(ListColumnKind::Modified)
        );

        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));

        assert_eq!(
            browser.list_column_reorder_insertion_target(),
            Some(ListColumnKind::Modified)
        );

        drop(browser.exit_list_column_reorder_target(ListColumnKind::Modified));

        assert_eq!(browser.list_column_reorder_insertion_target(), None);
    }

    #[test]
    fn entering_target_before_drag_activation_still_reorders_after_activation() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));

        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Size,
                ListColumnKind::Modified,
                ListColumnKind::Kind,
            ]
        );
        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
    }

    #[test]
    fn entering_target_without_drag_activation_still_sorts_instead_of_reordering() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(browser.options.sort_field, SortField::Size);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
            ]
        );
    }

    #[test]
    fn leaving_target_before_drag_activation_does_not_leave_stale_reorder_target() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let origin = browser.cursor_position;

        drop(browser.start_list_column_reorder_drag(BrowserPaneId::PRIMARY, ListColumnKind::Size));
        drop(browser.enter_list_column_reorder_target(ListColumnKind::Modified));
        drop(browser.exit_list_column_reorder_target(ListColumnKind::Modified));

        browser.update_list_column_reorder_drag(iced::Point {
            x: origin.x + POINTER_DRAG_ACTIVATION_DISTANCE,
            y: origin.y,
        });
        drop(browser.finish_list_column_reorder_drag_command());

        assert_eq!(
            browser
                .user_config
                .list_view_preferences
                .visible_columns()
                .map(|column| column.kind)
                .collect::<Vec<_>>(),
            vec![
                ListColumnKind::Name,
                ListColumnKind::Modified,
                ListColumnKind::Size,
                ListColumnKind::Kind,
            ]
        );
        assert_eq!(browser.options.sort_field, SortField::Name);
        assert_eq!(browser.options.sort_direction, SortDirection::Ascending);
    }
}
