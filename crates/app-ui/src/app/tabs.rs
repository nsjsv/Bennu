use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use iced::Task;

use super::FileBrowser;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, ColumnBrowserViewport,
    FileDragPhase, Message, NavigationMode, SplitAxis, SplitRegion, TabDragMode, TabDragState,
    TabSplitTarget,
};

mod animation;
pub(crate) use animation::{TabAnimationState, TabBarReveal};
use animation::{TAB_REORDER_HORIZONTAL_PADDING, TAB_REORDER_MIN_SLOT_WIDTH, TAB_REORDER_SPACING};
mod folder_middle_click_split;
#[cfg(test)]
mod view_mode_follow_tests;
#[cfg(test)]
mod view_mode_tests;

pub(crate) struct TabDragPreview<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) is_trash_view: bool,
}

impl FileBrowser {
    pub(crate) fn tab_bar_reveal_fraction(&self) -> f32 {
        self.tab_bar_reveal.fraction()
    }

    pub(super) fn tab_bar_reveal_animation_is_active(&self) -> bool {
        self.tab_bar_reveal.is_animating()
    }

    pub(super) fn advance_tab_bar_reveal_animation(&mut self) -> Task<Message> {
        match self.tab_bar_reveal {
            TabBarReveal::Opening { .. } if self.tab_bar_reveal_fraction() >= 1.0 => {
                self.tab_bar_reveal = TabBarReveal::Visible;
            }
            TabBarReveal::Closing { .. } if self.tab_bar_reveal_fraction() <= f32::EPSILON => {
                self.tab_bar_reveal = TabBarReveal::Hidden;
            }
            _ => {}
        }
        Task::none()
    }

    pub(super) fn tab_animation_is_active(&self) -> bool {
        !self.tab_animations.is_empty()
    }

    pub(super) fn advance_tab_animations(&mut self) -> Task<Message> {
        self.remove_closed_tabs_after_animation();
        self.prune_tab_animations();
        Task::none()
    }

    pub(crate) fn tab_drag_preview(&self) -> Option<TabDragPreview<'_>> {
        let drag = self.tab_drag.as_ref()?;
        if !drag.is_dragging() {
            return None;
        }

        let tab = if drag.source_pane_id == self.active_pane_id() {
            self.tabs.iter().find(|tab| tab.id == drag.tab_id)?
        } else {
            self.pane_by_id(drag.source_pane_id)?
                .tabs
                .iter()
                .find(|tab| tab.id == drag.tab_id)?
        };

        Some(TabDragPreview {
            directory: tab.directory.as_path(),
            is_trash_view: tab.is_trash_view,
        })
    }

    pub(super) fn sync_active_tab_state(&mut self) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab_id)
        else {
            return;
        };

        tab.directory = self.current_dir.clone();
        tab.is_trash_view = self.is_trash_view;
        tab.entries = self.entries.clone();
        tab.trash_entries = self.trash_entries.clone();
        tab.selected = self.selected.clone();
        tab.selected_paths = self.selected_paths.clone();
        tab.selection_anchor = self.selection_anchor.clone();
        tab.deepest_open_column_directory = self.deepest_open_column_directory.clone();
        tab.expanded_directories = self.expanded_directories.clone();
        tab.view_mode = self.view_mode;
        tab.back_stack = self.back_stack.clone();
        tab.forward_stack = self.forward_stack.clone();
        self.sync_active_pane_state();
    }

    pub(super) fn open_directory_in_new_tab(&mut self, directory: PathBuf) -> Task<Message> {
        self.clear_icon_grid_expansion_for_context_change();
        let cancel_address_editing = self.cancel_address_editing();
        self.sync_active_tab_state();
        self.context_menu = None;
        self.clear_preview();

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs
            .push(BrowserTab::directory(tab_id, directory.clone()));
        self.start_tab_intro_animation(tab_id);

        self.sync_tab_bar_visibility();
        self.active_tab_id = tab_id;
        self.back_stack.clear();
        self.forward_stack.clear();
        Task::batch([
            cancel_address_editing,
            self.navigate_to(directory, NavigationMode::KeepHistory),
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn open_trash_in_new_tab(&mut self) -> Task<Message> {
        self.clear_icon_grid_expansion_for_context_change();
        let cancel_address_editing = self.cancel_address_editing();
        self.sync_active_tab_state();
        self.context_menu = None;
        self.clear_preview();

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(BrowserTab::trash(tab_id));
        self.start_tab_intro_animation(tab_id);

        self.sync_tab_bar_visibility();
        self.active_tab_id = tab_id;
        self.back_stack.clear();
        self.forward_stack.clear();
        Task::batch([
            cancel_address_editing,
            self.open_trash_view(NavigationMode::KeepHistory),
            self.request_browser_session_save(),
        ])
    }

    pub(in crate::app) fn finish_tab_drag_from_captured_release(
        &mut self,
        window: iced::window::Id,
    ) -> Task<Message> {
        if self.file_drop_session.is_some() {
            Task::none()
        } else {
            self.finish_pointer_drag_interactions(window)
        }
    }

    pub(super) fn select_tab(&mut self, tab_id: usize) -> Task<Message> {
        if tab_id == self.active_tab_id {
            self.sync_active_tab_state();
            return Task::none();
        }
        let Some(cancel_address_editing) = self.apply_tab_selection(tab_id) else {
            return Task::none();
        };
        Task::batch([
            cancel_address_editing,
            self.reload_current(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
    }

    pub(in crate::app) fn select_tab_for_file_drop(
        &mut self,
        pane_id: BrowserPaneId,
        tab_id: usize,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        let Some(cancel_address_editing) = self.apply_tab_selection(tab_id) else {
            return Task::none();
        };
        Task::batch([
            cancel_address_editing,
            self.reload_current_for_file_drop(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
    }

    fn apply_tab_selection(&mut self, tab_id: usize) -> Option<Task<Message>> {
        if self.tab_is_closing(tab_id) {
            return None;
        }
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id).cloned()?;

        self.clear_icon_grid_expansion_for_context_change();
        let cancel_address_editing = self.cancel_address_editing();
        self.sync_active_tab_state();
        self.active_tab_id = tab.id;
        self.clear_column_interaction_context();
        self.is_trash_view = tab.is_trash_view;
        self.set_entries(tab.entries);
        self.directory_discovery = tab.directory_discovery;
        self.directory_loading_placeholder = None;
        self.trash_entries = tab.trash_entries;
        self.selected = tab.selected;
        self.selected_paths = tab.selected_paths;
        self.selection_anchor = tab.selection_anchor;
        self.deepest_open_column_directory = tab.deepest_open_column_directory;
        self.expanded_directories = tab.expanded_directories;
        self.view_mode = tab.view_mode;
        self.back_stack = tab.back_stack;
        self.forward_stack = tab.forward_stack;
        self.current_dir = tab.directory;
        Some(cancel_address_editing)
    }

    pub(super) fn close_tab(&mut self, tab_id: usize) -> Task<Message> {
        if self.open_tab_count() <= 1 || self.tab_is_closing(tab_id) {
            return Task::none();
        }

        let Some(closing_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Task::none();
        };
        let was_active = tab_id == self.active_tab_id;
        self.invalidate_file_drop_for_tab_close();

        let drag_finish_task = if self
            .tab_drag
            .as_ref()
            .is_some_and(|drag| drag.tab_id == tab_id)
        {
            self.finish_tab_drag()
        } else {
            Task::none()
        };

        self.start_tab_close_animation(tab_id);

        if !was_active {
            return drag_finish_task;
        }

        let Some(adjacent_tab_id) = self.adjacent_open_tab_id(closing_index, tab_id) else {
            return drag_finish_task;
        };
        Task::batch([
            self.select_tab(adjacent_tab_id),
            drag_finish_task,
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn start_tab_drag(&mut self, pane_id: BrowserPaneId, tab_id: usize) {
        if pane_id != self.active_pane_id()
            || self.tab_is_closing(tab_id)
            || !self.tabs.iter().any(|tab| tab.id == tab_id)
        {
            return;
        }

        let mode = if self.keyboard_modifiers.control() && self.keyboard_modifiers.shift() {
            TabDragMode::Split
        } else {
            TabDragMode::Reorder
        };
        self.tab_drag = Some(TabDragState {
            source_pane_id: pane_id,
            tab_id,
            phase: FileDragPhase::WaitingForMovement {
                origin: self.cursor_position,
            },
            mode,
            split_target: None,
        });
    }

    pub(super) fn reorder_dragged_tab(
        &mut self,
        entered_pane_id: BrowserPaneId,
        entered_tab_id: usize,
    ) -> Task<Message> {
        let Some(drag) = self.tab_drag.as_ref() else {
            return Task::none();
        };
        if drag.mode != TabDragMode::Reorder
            || drag.source_pane_id != entered_pane_id
            || !drag.is_dragging()
        {
            return Task::none();
        }
        let dragged_tab_id = drag.tab_id;
        if dragged_tab_id == entered_tab_id
            || self.tab_is_closing(dragged_tab_id)
            || self.tab_is_closing(entered_tab_id)
        {
            return Task::none();
        }

        let Some(dragged_index) = self.tabs.iter().position(|tab| tab.id == dragged_tab_id) else {
            return self.finish_tab_drag();
        };
        let Some(entered_index) = self.tabs.iter().position(|tab| tab.id == entered_tab_id) else {
            return Task::none();
        };

        let shifted_tab_ids = self.shifted_tab_ids_for_reorder(dragged_index, entered_index);
        let shift_offset = if dragged_index < entered_index {
            self.tab_reorder_slot_width()
        } else {
            -self.tab_reorder_slot_width()
        };

        let dragged_tab = self.tabs.remove(dragged_index);
        self.tabs.insert(entered_index, dragged_tab);
        self.start_tab_reorder_animations(shifted_tab_ids, shift_offset);
        self.sync_active_pane_state();
        // 标签顺序属于会话模型；重排落点必须标记保存，否则重启后顺序回退。
        self.request_browser_session_save()
    }

    pub(super) fn finish_tab_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.tab_drag.take() else {
            return Task::none();
        };
        if drag.mode != TabDragMode::Split || !drag.is_dragging() {
            return Task::none();
        }
        let Some(target) = drag.split_target else {
            return Task::none();
        };
        self.finish_tab_split_drag(drag.source_pane_id, drag.tab_id, target.region);
        // 分屏/跨栏移动改变了 panes 与 pane_layout；与会话保存不变量对齐。
        self.request_browser_session_save()
    }

    pub(super) fn update_tab_drag(&mut self, position: iced::Point) {
        let mut should_update_split_target = false;
        if let Some(drag) = &mut self.tab_drag {
            if let FileDragPhase::WaitingForMovement { origin } = drag.phase {
                let delta_x = position.x - origin.x;
                let delta_y = position.y - origin.y;
                if delta_x * delta_x + delta_y * delta_y
                    >= super::POINTER_DRAG_ACTIVATION_DISTANCE
                        * super::POINTER_DRAG_ACTIVATION_DISTANCE
                {
                    drag.phase = FileDragPhase::Dragging;
                }
            }
            should_update_split_target = drag.mode == TabDragMode::Split && drag.is_dragging();
        }

        if !should_update_split_target {
            return;
        }
        let split_target = self
            .split_region_at(position)
            .map(|region| TabSplitTarget { region });
        if let Some(drag) = &mut self.tab_drag {
            drag.split_target = split_target;
        }
    }

    fn finish_tab_split_drag(
        &mut self,
        source_pane_id: BrowserPaneId,
        tab_id: usize,
        region: SplitRegion,
    ) {
        self.sync_active_tab_state();

        match self.pane_layout {
            BrowserPaneLayout::Single { .. } => {
                self.split_single_pane_with_tab(source_pane_id, tab_id, region)
            }
            BrowserPaneLayout::Split {
                axis,
                first,
                second,
                first_portion,
                ..
            } => self.move_tab_to_split_side(
                source_pane_id,
                tab_id,
                region,
                axis,
                first,
                second,
                first_portion,
            ),
        }
    }

    fn split_single_pane_with_tab(
        &mut self,
        source_pane_id: BrowserPaneId,
        tab_id: usize,
        region: SplitRegion,
    ) {
        let destination_id = self.new_pane_id();
        let Some(tab) = self.detach_or_copy_tab(source_pane_id, tab_id) else {
            return;
        };
        let destination = pane_from_tab(destination_id, tab);
        self.panes.push(destination.clone());

        let (first, second) = if region.places_dragged_first() {
            (destination_id, source_pane_id)
        } else {
            (source_pane_id, destination_id)
        };
        self.pane_layout = BrowserPaneLayout::Split {
            axis: region.axis(),
            first,
            second,
            active: destination_id,
            first_portion: 500,
        };
        self.restore_pane_snapshot(destination);
    }

    fn move_tab_to_split_side(
        &mut self,
        source_pane_id: BrowserPaneId,
        tab_id: usize,
        region: SplitRegion,
        current_axis: SplitAxis,
        first: BrowserPaneId,
        second: BrowserPaneId,
        current_first_portion: u16,
    ) {
        let current_destination = if region.places_dragged_first() {
            first
        } else {
            second
        };
        let destination_id = if current_destination == source_pane_id {
            source_pane_id
        } else {
            current_destination
        };

        if destination_id == source_pane_id {
            if let Some(source) = self.pane_by_id_mut(source_pane_id) {
                if source.tabs.iter().any(|tab| tab.id == tab_id) {
                    source.active_tab_id = tab_id;
                    apply_active_tab_to_pane(source);
                }
            }
        } else {
            let Some(tab) = self.detach_or_copy_tab(source_pane_id, tab_id) else {
                return;
            };
            if let Some(destination) = self.pane_by_id_mut(destination_id) {
                destination.tabs.push(tab.clone());
                destination.active_tab_id = tab.id;
                apply_tab_to_pane(destination, &tab);
            }
        }

        let other_id = if destination_id == first {
            second
        } else {
            first
        };
        let (layout_first, layout_second) = if region.places_dragged_first() {
            (destination_id, other_id)
        } else {
            (other_id, destination_id)
        };
        self.pane_layout = BrowserPaneLayout::Split {
            axis: region.axis(),
            first: layout_first,
            second: layout_second,
            active: destination_id,
            first_portion: if current_axis == region.axis() {
                current_first_portion
            } else {
                500
            },
        };

        if let Some(destination) = self.pane_by_id(destination_id).cloned() {
            self.restore_pane_snapshot(destination);
        }
    }

    fn detach_or_copy_tab(
        &mut self,
        source_pane_id: BrowserPaneId,
        tab_id: usize,
    ) -> Option<BrowserTab> {
        let copied_tab = {
            let source = self.pane_by_id_mut(source_pane_id)?;
            let tab_index = source.tabs.iter().position(|tab| tab.id == tab_id)?;
            if source.tabs.len() == 1 {
                Some(source.tabs[tab_index].clone())
            } else {
                let tab = source.tabs.remove(tab_index);
                if source.active_tab_id == tab_id {
                    let adjacent_index = tab_index.min(source.tabs.len() - 1);
                    source.active_tab_id = source.tabs[adjacent_index].id;
                    apply_active_tab_to_pane(source);
                }
                return Some(tab);
            }
        };

        let mut cloned = copied_tab?;
        cloned.id = self.next_tab_id;
        self.next_tab_id += 1;
        Some(cloned)
    }

    pub(super) fn sync_tab_bar_visibility(&mut self) {
        if self.tabs.len() > 1 {
            self.reveal_tab_bar();
        } else {
            self.hide_tab_bar();
        }
    }

    fn reveal_tab_bar(&mut self) {
        if matches!(
            self.tab_bar_reveal,
            TabBarReveal::Visible | TabBarReveal::Opening { .. }
        ) {
            return;
        }

        let initial_fraction = self.tab_bar_reveal_fraction();
        self.tab_bar_reveal = if (1.0 - initial_fraction) <= f32::EPSILON {
            TabBarReveal::Visible
        } else {
            TabBarReveal::Opening {
                started_at: Instant::now(),
                initial_fraction,
            }
        };
    }

    fn hide_tab_bar(&mut self) {
        if matches!(
            self.tab_bar_reveal,
            TabBarReveal::Hidden | TabBarReveal::Closing { .. }
        ) {
            return;
        }

        let initial_fraction = self.tab_bar_reveal_fraction();
        self.tab_bar_reveal = if initial_fraction <= f32::EPSILON {
            TabBarReveal::Hidden
        } else {
            TabBarReveal::Closing {
                started_at: Instant::now(),
                initial_fraction,
            }
        };
    }

    pub(super) fn start_tab_intro_animation(&mut self, tab_id: usize) {
        self.tab_animations.entry(tab_id).or_default().start_intro();
    }

    fn start_tab_close_animation(&mut self, tab_id: usize) {
        let initial_fraction = self
            .tab_animations
            .get(&tab_id)
            .map(|animation| animation.width_fraction())
            .unwrap_or(1.0);
        self.tab_animations
            .entry(tab_id)
            .or_default()
            .start_close(initial_fraction);
    }

    fn start_tab_reorder_animations(&mut self, tab_ids: Vec<usize>, shift_offset: f32) {
        for tab_id in tab_ids {
            let current_offset = self
                .tab_animations
                .get(&tab_id)
                .map(|animation| animation.shift_offset())
                .unwrap_or(0.0);
            self.tab_animations
                .entry(tab_id)
                .or_default()
                .start_shift(current_offset + shift_offset);
        }
    }

    fn remove_closed_tabs_after_animation(&mut self) {
        let closed_tab_ids = self
            .tab_animations
            .iter()
            .filter_map(|(tab_id, animation)| animation.close_is_finished().then_some(*tab_id))
            .collect::<Vec<_>>();
        if closed_tab_ids.is_empty() {
            return;
        }

        for tab_id in closed_tab_ids {
            if let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) {
                self.tabs.remove(tab_index);
            }
            self.tab_animations.remove(&tab_id);
            if self
                .tab_drag
                .as_ref()
                .is_some_and(|drag| drag.tab_id == tab_id)
            {
                // 被关闭标签的拖拽已在 close_tab 时结束；此处兜底不产生布局变更。
                let _ = self.finish_tab_drag();
            }
        }

        self.sync_tab_bar_visibility();
        self.sync_active_pane_state();
        // 标签关闭后存活树变化：裁剪摘要缓存（内存上界机制）。
        self.prune_list_directory_summaries_to_live_roots();
    }

    fn shifted_tab_ids_for_reorder(
        &self,
        dragged_index: usize,
        entered_index: usize,
    ) -> Vec<usize> {
        if dragged_index < entered_index {
            self.tabs[dragged_index + 1..=entered_index]
                .iter()
                .map(|tab| tab.id)
                .collect()
        } else {
            self.tabs[entered_index..dragged_index]
                .iter()
                .map(|tab| tab.id)
                .collect()
        }
    }

    pub(in crate::app) fn tab_is_closing_for_file_drop(&self, tab_id: usize) -> bool {
        self.tab_is_closing(tab_id)
    }

    fn tab_is_closing(&self, tab_id: usize) -> bool {
        self.tab_animations
            .get(&tab_id)
            .is_some_and(|animation| animation.is_closing())
    }

    fn open_tab_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|tab| !self.tab_is_closing(tab.id))
            .count()
    }

    fn adjacent_open_tab_id(&self, closing_index: usize, closing_tab_id: usize) -> Option<usize> {
        self.tabs[..closing_index]
            .iter()
            .rev()
            .chain(self.tabs[closing_index + 1..].iter())
            .find(|tab| tab.id != closing_tab_id && !self.tab_is_closing(tab.id))
            .map(|tab| tab.id)
    }

    fn prune_tab_animations(&mut self) {
        let visible_tab_ids = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        self.tab_animations.retain(|tab_id, animation| {
            visible_tab_ids.contains(tab_id) && animation.is_animating()
        });
    }

    fn tab_reorder_slot_width(&self) -> f32 {
        let tab_count = self.tabs.len().max(1) as f32;
        let available_width = self.pane_content_width()
            - TAB_REORDER_HORIZONTAL_PADDING
            - TAB_REORDER_SPACING * (tab_count - 1.0);
        (available_width / tab_count).max(TAB_REORDER_MIN_SLOT_WIDTH)
    }
}

fn pane_from_tab(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
    let mut pane = BrowserPane {
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
        column_viewports: HashMap::new(),
        tabs: vec![tab.clone()],
        active_tab_id: tab.id,
        directory_load_generation: 0,
        directory_load_cancel: None,
        back_stack: tab.back_stack.clone(),
        forward_stack: tab.forward_stack.clone(),
        directory_collection_phase: tab.directory_collection_phase,
        directory_order_phase: tab.directory_order_phase,
    };
    pane.sync_active_tab_state();
    pane
}

pub(super) fn apply_active_tab_to_pane(pane: &mut BrowserPane) {
    let Some(tab) = pane
        .tabs
        .iter()
        .find(|tab| tab.id == pane.active_tab_id)
        .cloned()
    else {
        return;
    };
    apply_tab_to_pane(pane, &tab);
}

pub(super) fn apply_tab_to_pane(pane: &mut BrowserPane, tab: &BrowserTab) {
    pane.current_dir = tab.directory.clone();
    pane.is_trash_view = tab.is_trash_view;
    pane.entries = tab.entries.clone();
    pane.directory_discovery = tab.directory_discovery.clone();
    pane.directory_loading_placeholder = None;
    pane.trash_entries = tab.trash_entries.clone();
    pane.selected = tab.selected.clone();
    pane.selected_paths = tab.selected_paths.clone();
    pane.selection_anchor = tab.selection_anchor.clone();
    pane.deepest_open_column_directory = tab.deepest_open_column_directory.clone();
    pane.expanded_directories = tab.expanded_directories.clone();
    pane.view_mode = tab.view_mode;
    pane.back_stack = tab.back_stack.clone();
    pane.forward_stack = tab.forward_stack.clone();
    pane.directory_collection_phase = tab.directory_collection_phase;
    pane.directory_order_phase = tab.directory_order_phase;
    pane.sync_active_tab_state();
}

#[cfg(test)]
mod drag_session_save_tests {
    use super::*;
    use crate::config;

    fn save_enabled_browser(tabs: Vec<BrowserTab>) -> FileBrowser {
        let mut config = config::ui_thread_startup_config();
        config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
        config.save_view_state = config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config);
        browser.active_tab_id = tabs[0].id;
        browser.tabs = tabs;
        browser
    }

    #[test]
    fn finishing_split_tab_drag_requests_session_save() {
        let mut browser =
            save_enabled_browser(vec![BrowserTab::directory(0, PathBuf::from("/workspace/left"))]);
        browser.tab_drag = Some(TabDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            tab_id: 0,
            phase: FileDragPhase::Dragging,
            mode: TabDragMode::Split,
            split_target: Some(TabSplitTarget {
                region: SplitRegion::Right,
            }),
        });

        drop(browser.finish_tab_drag());

        assert!(matches!(browser.pane_layout, BrowserPaneLayout::Split { .. }));
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn finishing_tab_drag_without_split_target_does_not_request_session_save() {
        let mut browser =
            save_enabled_browser(vec![BrowserTab::directory(0, PathBuf::from("/workspace/left"))]);
        browser.tab_drag = Some(TabDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            tab_id: 0,
            phase: FileDragPhase::Dragging,
            mode: TabDragMode::Split,
            split_target: None,
        });

        drop(browser.finish_tab_drag());

        assert!(matches!(
            browser.pane_layout,
            BrowserPaneLayout::Single { .. }
        ));
        assert!(!browser.pending_browser_session_save);
    }

    #[test]
    fn reordering_tab_requests_session_save() {
        let mut browser = save_enabled_browser(vec![
            BrowserTab::directory(0, PathBuf::from("/workspace/left")),
            BrowserTab::directory(1, PathBuf::from("/workspace/right")),
        ]);
        browser.tab_drag = Some(TabDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            tab_id: 1,
            phase: FileDragPhase::Dragging,
            mode: TabDragMode::Reorder,
            split_target: None,
        });

        drop(browser.reorder_dragged_tab(BrowserPaneId::PRIMARY, 0));

        assert_eq!(browser.tabs[0].id, 1);
        assert!(browser.pending_browser_session_save);
    }
}
