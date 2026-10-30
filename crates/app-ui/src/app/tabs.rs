use std::path::{Path, PathBuf};
use std::time::Instant;

use iced::Task;

use super::FileBrowser;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode, FileDragPhase,
    Message, NavigationMode, SplitAxis, SplitRegion, TabDragMode, TabDragState, TabSplitTarget,
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

        let tab = self
            .pane_by_id(drag.source_pane_id)?
            .tabs
            .iter()
            .find(|tab| tab.id == drag.tab_id)?;

        Some(TabDragPreview {
            directory: tab.current_dir.as_path(),
            is_trash_view: tab.is_trash_view,
        })
    }

    pub(super) fn open_directory_in_new_tab(&mut self, directory: PathBuf) -> Task<Message> {
        let cancel_address_editing = self.add_tab_and_activate(|id, view_mode| {
            let mut tab = BrowserTab::directory(id, directory.clone());
            tab.view_mode = view_mode;
            tab
        });
        Task::batch([
            cancel_address_editing,
            self.navigate_to(directory, NavigationMode::KeepHistory),
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn open_trash_in_new_tab(&mut self) -> Task<Message> {
        let cancel_address_editing = self.add_tab_and_activate(|id, view_mode| {
            let mut tab = BrowserTab::trash(id);
            tab.view_mode = view_mode;
            tab
        });
        Task::batch([
            cancel_address_editing,
            self.open_trash_view(NavigationMode::KeepHistory),
            self.request_browser_session_save(),
        ])
    }

    /// 在活动窗格末尾新增标签并切过去。新标签沿用当前标签的视图模式，
    /// 目录由调用方随后导航。
    fn add_tab_and_activate(
        &mut self,
        new_tab: impl FnOnce(usize, BrowserViewMode) -> BrowserTab,
    ) -> Task<Message> {
        self.clear_icon_grid_expansion_for_context_change();
        let cancel_address_editing = self.cancel_address_editing();
        self.context_menu = None;
        self.preview_engine.clear_preview();

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let view_mode = self.tab().view_mode;
        let pane = self.pane_mut();
        pane.tabs.push(new_tab(tab_id, view_mode));
        pane.activate_tab(tab_id);
        self.start_tab_intro_animation(tab_id);
        self.sync_tab_bar_visibility();
        self.follow_terminal_panel_directory();
        cancel_address_editing
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
        if tab_id == self.pane().active_tab_id {
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
        if self.tab_is_closing(tab_id) || !self.pane().tabs.iter().any(|tab| tab.id == tab_id) {
            return None;
        }

        self.clear_icon_grid_expansion_for_context_change();
        let cancel_address_editing = self.cancel_address_editing();
        self.pane_mut().activate_tab(tab_id);
        self.entries_replaced();
        self.clear_column_interaction_context();
        self.follow_terminal_panel_directory();
        Some(cancel_address_editing)
    }

    pub(super) fn close_tab(&mut self, tab_id: usize) -> Task<Message> {
        if self.open_tab_count() <= 1 || self.tab_is_closing(tab_id) {
            return Task::none();
        }

        let Some(closing_index) = self.pane().tabs.iter().position(|tab| tab.id == tab_id) else {
            return Task::none();
        };
        let was_active = tab_id == self.pane().active_tab_id;
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
            || !self.pane().tabs.iter().any(|tab| tab.id == tab_id)
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
                origin: self.cursor_or_origin(),
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

        let tabs = &self.pane().tabs;
        let Some(dragged_index) = tabs.iter().position(|tab| tab.id == dragged_tab_id) else {
            return self.finish_tab_drag();
        };
        let Some(entered_index) = tabs.iter().position(|tab| tab.id == entered_tab_id) else {
            return Task::none();
        };

        let shifted_tab_ids = self.shifted_tab_ids_for_reorder(dragged_index, entered_index);
        let shift_offset = if dragged_index < entered_index {
            self.tab_reorder_slot_width()
        } else {
            -self.tab_reorder_slot_width()
        };

        let tabs = &mut self.pane_mut().tabs;
        let dragged_tab = tabs.remove(dragged_index);
        tabs.insert(entered_index, dragged_tab);
        self.start_tab_reorder_animations(shifted_tab_ids, shift_offset);
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
        self.panes.push(BrowserPane::new(destination_id, tab));

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
        self.reset_active_pane_context();
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
                    source.activate_tab(tab_id);
                }
            }
        } else {
            let Some(tab) = self.detach_or_copy_tab(source_pane_id, tab_id) else {
                return;
            };
            if let Some(destination) = self.pane_by_id_mut(destination_id) {
                let tab_id = tab.id;
                destination.tabs.push(tab);
                destination.activate_tab(tab_id);
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

        self.reset_active_pane_context();
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
                    let adjacent_id = source.tabs[adjacent_index].id;
                    source.activate_tab(adjacent_id);
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
        if self.pane().tabs.len() > 1 {
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
            let tabs = &mut self.pane_mut().tabs;
            if let Some(tab_index) = tabs.iter().position(|tab| tab.id == tab_id) {
                tabs.remove(tab_index);
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
        // 标签关闭后存活树变化：裁剪摘要缓存（内存上界机制）。
        self.prune_list_directory_summaries_to_live_roots();
    }

    fn shifted_tab_ids_for_reorder(
        &self,
        dragged_index: usize,
        entered_index: usize,
    ) -> Vec<usize> {
        let tabs = &self.pane().tabs;
        if dragged_index < entered_index {
            tabs[dragged_index + 1..=entered_index]
                .iter()
                .map(|tab| tab.id)
                .collect()
        } else {
            tabs[entered_index..dragged_index]
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
        self.pane()
            .tabs
            .iter()
            .filter(|tab| !self.tab_is_closing(tab.id))
            .count()
    }

    fn adjacent_open_tab_id(&self, closing_index: usize, closing_tab_id: usize) -> Option<usize> {
        let tabs = &self.pane().tabs;
        tabs[..closing_index]
            .iter()
            .rev()
            .chain(tabs[closing_index + 1..].iter())
            .find(|tab| tab.id != closing_tab_id && !self.tab_is_closing(tab.id))
            .map(|tab| tab.id)
    }

    fn prune_tab_animations(&mut self) {
        let visible_tab_ids = self
            .pane()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        self.tab_animations.retain(|tab_id, animation| {
            visible_tab_ids.contains(tab_id) && animation.is_animating()
        });
    }

    fn tab_reorder_slot_width(&self) -> f32 {
        let tab_count = self.pane().tabs.len().max(1) as f32;
        let available_width = self.pane_content_width()
            - TAB_REORDER_HORIZONTAL_PADDING
            - TAB_REORDER_SPACING * (tab_count - 1.0);
        (available_width / tab_count).max(TAB_REORDER_MIN_SLOT_WIDTH)
    }
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
        let pane = browser.pane_mut();
        pane.active_tab_id = tabs[0].id;
        pane.tabs = tabs;
        browser
    }

    #[test]
    fn finishing_split_tab_drag_requests_session_save() {
        let mut browser = save_enabled_browser(vec![BrowserTab::directory(
            0,
            PathBuf::from("/workspace/left"),
        )]);
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

        assert!(matches!(
            browser.pane_layout,
            BrowserPaneLayout::Split { .. }
        ));
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn finishing_tab_drag_without_split_target_does_not_request_session_save() {
        let mut browser = save_enabled_browser(vec![BrowserTab::directory(
            0,
            PathBuf::from("/workspace/left"),
        )]);
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

        assert_eq!(browser.pane().tabs[0].id, 1);
        assert!(browser.pending_browser_session_save);
    }
}
