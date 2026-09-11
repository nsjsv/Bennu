use std::path::Path;

use iced::{event, Point, Task};

use super::tabs::apply_active_tab_to_pane;
use super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use crate::app::panes::SplitOverlayBounds;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, FileDragNativeDndState, FileDragPhase,
    Message, PaneDragPointerPress, PaneDragState, PaneDropTarget, SelectionMarqueePhase,
    SplitRegion,
};

const PANE_DROP_CENTER_FRACTION: f32 = 0.28;

#[derive(Debug, Clone, Copy)]
struct PaneDragContentBounds {
    top_left: Point,
    width: f32,
    height: f32,
}

#[derive(Debug, Clone, Copy)]
struct PaneDragPromotion {
    source_pane_id: BrowserPaneId,
    phase: FileDragPhase,
}

pub(crate) struct PaneDragPreview<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) is_trash_view: bool,
}

impl FileBrowser {
    pub(super) fn ctrl_shift_pane_drag_shortcut_is_pressed(&self) -> bool {
        self.keyboard_modifiers.control()
            && self.keyboard_modifiers.shift()
            && matches!(self.pane_layout, BrowserPaneLayout::Split { .. })
    }

    pub(super) fn start_ctrl_shift_pane_drag(&mut self, status: event::Status) -> bool {
        if !self.ctrl_shift_pane_drag_shortcut_is_pressed() {
            return false;
        }
        if !self.pane_drag_can_start(status) {
            return false;
        }

        let Some(source_pane_id) = self.hovered_pane_id else {
            return false;
        };
        if self.pane_by_id(source_pane_id).is_none() {
            return false;
        }

        self.sync_active_tab_state();
        self.pane_drag_pointer_press = None;
        self.pane_drag = Some(PaneDragState {
            source_pane_id,
            phase: FileDragPhase::WaitingForMovement {
                origin: self.cursor_position,
            },
            target: None,
        });
        true
    }

    pub(super) fn promote_ctrl_shift_pane_drag_from_active_pointer_drag(&mut self) -> bool {
        if !self.ctrl_shift_pane_drag_shortcut_is_pressed()
            || !self.pane_drag_can_take_over_active_pointer_drag()
        {
            return false;
        }

        let Some(promotion) = self.active_pointer_drag_for_pane_drag() else {
            return false;
        };
        let source_pane_id = promotion.source_pane_id;
        if self.pane_by_id(source_pane_id).is_none() {
            return false;
        }

        self.sync_active_tab_state();
        self.pane_drag_pointer_press = None;
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.drag_selection_anchor = None;
        self.sidebar_bookmark_drop_slot = None;
        self.pane_drag = Some(PaneDragState {
            source_pane_id,
            phase: promotion.phase,
            target: None,
        });
        true
    }

    pub(super) fn record_pane_drag_pointer_press(&mut self) {
        let source_pane_id = self.active_pane_id();
        self.pane_drag_pointer_press =
            self.pane_by_id(source_pane_id)
                .is_some()
                .then_some(PaneDragPointerPress {
                    source_pane_id,
                    origin: self.cursor_position,
                });
    }

    pub(super) fn update_pane_drag(&mut self, position: Point) {
        let mut source_pane_id = None;
        if let Some(drag) = &mut self.pane_drag {
            if let FileDragPhase::WaitingForMovement { origin } = drag.phase {
                let delta_x = position.x - origin.x;
                let delta_y = position.y - origin.y;
                if delta_x * delta_x + delta_y * delta_y
                    >= POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE
                {
                    drag.phase = FileDragPhase::Dragging;
                }
            }

            if drag.is_dragging() {
                source_pane_id = Some(drag.source_pane_id);
            }
        }

        let Some(source_pane_id) = source_pane_id else {
            return;
        };
        let target = self.pane_drop_target_at(source_pane_id, position);
        if let Some(drag) = &mut self.pane_drag {
            drag.target = target;
        }
    }

    pub(super) fn finish_pane_drag(&mut self) -> Task<Message> {
        self.pane_drag_pointer_press = None;
        let Some(drag) = self.pane_drag.take() else {
            return Task::none();
        };
        if !drag.is_dragging() {
            return Task::none();
        }

        match drag.target {
            Some(PaneDropTarget::Merge(target_pane_id)) => {
                self.merge_dragged_pane_into_target(drag.source_pane_id, target_pane_id);
                // 合并把布局改回 Single 并增删 panes；与会话保存不变量对齐。
                self.request_browser_session_save()
            }
            Some(PaneDropTarget::Split(region)) => {
                self.move_dragged_pane_to_split_region(drag.source_pane_id, region);
                self.request_browser_session_save()
            }
            None => Task::none(),
        }
    }

    pub(crate) fn pane_drag_preview(&self) -> Option<PaneDragPreview<'_>> {
        let drag = self.pane_drag.as_ref()?;
        if !drag.is_dragging() {
            return None;
        }

        let pane = self.pane_by_id(drag.source_pane_id)?;
        let tab = pane.tabs.iter().find(|tab| tab.id == pane.active_tab_id)?;
        Some(PaneDragPreview {
            directory: tab.directory.as_path(),
            is_trash_view: tab.is_trash_view,
        })
    }

    pub(crate) fn pane_drag_overlay_bounds(&self) -> Option<SplitOverlayBounds> {
        let drag = self.pane_drag.as_ref()?;
        if !drag.is_dragging() {
            return None;
        }

        match drag.target? {
            PaneDropTarget::Split(region) => Some(self.split_region_overlay_bounds(region)),
            PaneDropTarget::Merge(_) => {
                Some(self.pane_drag_content_bounds().center_overlay_bounds())
            }
        }
    }

    fn pane_drag_can_start(&self, status: event::Status) -> bool {
        let pointer_status_allows_start = match status {
            event::Status::Ignored => true,
            event::Status::Captured => {
                self.is_cursor_over_column_browser && self.renaming.is_none()
            }
        };
        pointer_status_allows_start
            && self.pane_drag.is_none()
            && self.pane_drag_shared_state_allows_start()
            && self.file_drag.is_none()
            && self.selection_marquee.is_none()
    }

    fn pane_drag_can_take_over_active_pointer_drag(&self) -> bool {
        self.pane_drag.is_none()
            && self.pane_drag_shared_state_allows_start()
            && self.active_pointer_drag_for_pane_drag().is_some()
    }

    fn pane_drag_shared_state_allows_start(&self) -> bool {
        self.destructive_action_confirmation.is_none()
            && self.transfer_conflict.is_none()
            && self.context_menu.is_none()
            && self.open_with.is_none()
            && self.address_editing.is_none()
            && !self.operation_queue_interaction_is_open()
            && self.tab_drag.is_none()
            && self.sidebar_bookmark_drag.is_none()
            && self.sidebar_resize_drag.is_none()
            && self.column_resize_drag.is_none()
    }

    fn active_pointer_drag_for_pane_drag(&self) -> Option<PaneDragPromotion> {
        if let Some(file_drag) = &self.file_drag {
            return (file_drag.native_dnd == FileDragNativeDndState::NotRequested).then_some(
                PaneDragPromotion {
                    source_pane_id: self.active_pane_id(),
                    phase: file_drag.phase,
                },
            );
        }

        if let Some(marquee) = &self.selection_marquee {
            return Some(PaneDragPromotion {
                source_pane_id: self.active_pane_id(),
                phase: match marquee.phase {
                    SelectionMarqueePhase::WaitingForMovement => {
                        FileDragPhase::WaitingForMovement {
                            origin: marquee.gesture_origin,
                        }
                    }
                    SelectionMarqueePhase::Selecting => FileDragPhase::Dragging,
                },
            });
        }

        self.pane_drag_pointer_press.map(|press| PaneDragPromotion {
            source_pane_id: press.source_pane_id,
            phase: FileDragPhase::WaitingForMovement {
                origin: press.origin,
            },
        })
    }

    fn operation_queue_interaction_is_open(&self) -> bool {
        self.operation_queue.is_panel_open()
    }

    fn pane_drop_target_at(
        &self,
        source_pane_id: BrowserPaneId,
        position: Point,
    ) -> Option<PaneDropTarget> {
        let BrowserPaneLayout::Split { first, second, .. } = self.pane_layout else {
            return None;
        };
        let merge_target_pane_id = other_split_pane_id(source_pane_id, first, second)?;
        let bounds = self.pane_drag_content_bounds();
        bounds
            .contains(position)
            .then(|| bounds.drop_target_at(position, merge_target_pane_id))
    }

    fn move_dragged_pane_to_split_region(
        &mut self,
        source_pane_id: BrowserPaneId,
        region: SplitRegion,
    ) {
        let BrowserPaneLayout::Split {
            axis,
            first,
            second,
            first_portion,
            ..
        } = self.pane_layout
        else {
            return;
        };
        let Some(other_pane_id) = other_split_pane_id(source_pane_id, first, second) else {
            return;
        };

        self.sync_active_tab_state();
        let (first, second) = if region.places_dragged_first() {
            (source_pane_id, other_pane_id)
        } else {
            (other_pane_id, source_pane_id)
        };
        self.pane_layout = BrowserPaneLayout::Split {
            axis: region.axis(),
            first,
            second,
            active: source_pane_id,
            first_portion: if axis == region.axis() {
                first_portion
            } else {
                500
            },
        };

        if let Some(source) = self.pane_by_id(source_pane_id).cloned() {
            self.restore_pane_snapshot(source);
        }
    }

    fn merge_dragged_pane_into_target(
        &mut self,
        source_pane_id: BrowserPaneId,
        target_pane_id: BrowserPaneId,
    ) {
        if source_pane_id == target_pane_id {
            return;
        }

        self.sync_active_tab_state();
        let Some(source) = self.pane_by_id(source_pane_id).cloned() else {
            return;
        };
        let Some(mut target) = self.pane_by_id(target_pane_id).cloned() else {
            return;
        };

        merge_pane_tabs(&mut target, source);
        self.panes.retain(|pane| pane.id != source_pane_id);
        self.icon_grid_viewports.remove(&source_pane_id);
        if let Some(target_slot) = self.pane_by_id_mut(target_pane_id) {
            *target_slot = target.clone();
        } else {
            self.panes.push(target.clone());
        }

        self.pane_layout = BrowserPaneLayout::Single {
            active: target_pane_id,
        };
        self.restore_pane_snapshot(target);
    }

    fn pane_drag_content_bounds(&self) -> PaneDragContentBounds {
        let sidebar_width = self.sidebar_width;
        PaneDragContentBounds {
            top_left: Point::new(sidebar_width, 0.0),
            width: (self.main_window_width - sidebar_width).max(1.0),
            height: self.main_window_height.max(1.0),
        }
    }

    fn split_region_overlay_bounds(&self, region: SplitRegion) -> SplitOverlayBounds {
        let sidebar_width = self.sidebar_width;
        let content_width = (self.main_window_width - sidebar_width).max(1.0);
        let content_height = self.main_window_height.max(1.0);
        let half_width = content_width / 2.0;
        let half_height = content_height / 2.0;

        match region {
            SplitRegion::Left => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, 0.0),
                width: half_width,
                height: content_height,
            },
            SplitRegion::Right => SplitOverlayBounds {
                top_left: Point::new(sidebar_width + half_width, 0.0),
                width: half_width,
                height: content_height,
            },
            SplitRegion::Top => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, 0.0),
                width: content_width,
                height: half_height,
            },
            SplitRegion::Bottom => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, half_height),
                width: content_width,
                height: half_height,
            },
        }
    }
}

impl PaneDragContentBounds {
    fn contains(self, position: Point) -> bool {
        position.x >= self.top_left.x
            && position.x <= self.top_left.x + self.width
            && position.y >= self.top_left.y
            && position.y <= self.top_left.y + self.height
    }

    fn drop_target_at(
        self,
        position: Point,
        merge_target_pane_id: BrowserPaneId,
    ) -> PaneDropTarget {
        let local_x = ((position.x - self.top_left.x) / self.width).clamp(0.0, 1.0);
        let local_y = ((position.y - self.top_left.y) / self.height).clamp(0.0, 1.0);
        let center_min = PANE_DROP_CENTER_FRACTION;
        let center_max = 1.0 - PANE_DROP_CENTER_FRACTION;
        if local_x >= center_min
            && local_x <= center_max
            && local_y >= center_min
            && local_y <= center_max
        {
            return PaneDropTarget::Merge(merge_target_pane_id);
        }

        PaneDropTarget::Split(closest_split_region(local_x, local_y))
    }

    fn center_overlay_bounds(self) -> SplitOverlayBounds {
        let inset_x = self.width * PANE_DROP_CENTER_FRACTION;
        let inset_y = self.height * PANE_DROP_CENTER_FRACTION;
        SplitOverlayBounds {
            top_left: Point::new(self.top_left.x + inset_x, self.top_left.y + inset_y),
            width: self.width - inset_x * 2.0,
            height: self.height - inset_y * 2.0,
        }
    }
}

fn closest_split_region(local_x: f32, local_y: f32) -> SplitRegion {
    let mut closest_distance = local_x;
    let mut region = SplitRegion::Left;
    let right_distance = 1.0 - local_x;
    if right_distance < closest_distance {
        closest_distance = right_distance;
        region = SplitRegion::Right;
    }
    if local_y < closest_distance {
        closest_distance = local_y;
        region = SplitRegion::Top;
    }
    if 1.0 - local_y < closest_distance {
        region = SplitRegion::Bottom;
    }
    region
}

fn other_split_pane_id(
    source_pane_id: BrowserPaneId,
    first: BrowserPaneId,
    second: BrowserPaneId,
) -> Option<BrowserPaneId> {
    if source_pane_id == first {
        Some(second)
    } else if source_pane_id == second {
        Some(first)
    } else {
        None
    }
}

fn merge_pane_tabs(target: &mut BrowserPane, source: BrowserPane) {
    let source_active_tab_id = source.active_tab_id;
    target.tabs.extend(source.tabs);
    target.active_tab_id = source_active_tab_id;
    apply_active_tab_to_pane(target);
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    use file_core::TrashEntry;
    use iced::{keyboard, Point};

    use super::*;
    use crate::config;
    use crate::model::{
        BrowserTab, BrowserViewMode, ColumnBrowserViewport, FileDragState, Message,
        SelectionMarquee, SelectionMarqueeSource, SplitAxis,
    };
    use crate::thumbnail_cache::ColumnViewport;

    fn split_browser_for_test() -> FileBrowser {
        split_browser_for_test_with(config::default_user_config())
    }

    fn split_browser_for_test_with(config: config::UserConfig) -> FileBrowser {
        let left_directory = PathBuf::from("/workspace/left");
        let right_directory = PathBuf::from("/workspace/right");
        let left_tab = BrowserTab::directory(0, left_directory.clone());
        let right_tab = BrowserTab::directory(1, right_directory);
        let (mut browser, _) = FileBrowser::new(config);

        browser.sidebar_width = 0.0;
        browser.main_window_width = 200.0;
        browser.main_window_height = 100.0;
        browser.current_dir = left_directory;
        browser.tabs = vec![left_tab.clone()];
        browser.active_tab_id = left_tab.id;
        browser.pane_layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 500,
        };
        browser.panes = vec![
            pane_from_tab_for_test(BrowserPaneId::PRIMARY, left_tab),
            pane_from_tab_for_test(BrowserPaneId(1), right_tab),
        ];
        browser
    }

    fn pane_from_tab_for_test(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
        BrowserPane {
            id: pane_id,
            current_dir: tab.directory.clone(),
            is_trash_view: tab.is_trash_view,
            entries: tab.entries.clone(),
            directory_discovery: tab.directory_discovery.clone(),
            directory_loading_placeholder: None,
            trash_entries: Vec::<TrashEntry>::new(),
            selected: tab.selected.clone(),
            selected_paths: tab.selected_paths.clone(),
            selection_anchor: tab.selection_anchor.clone(),
            deepest_open_column_directory: tab.deepest_open_column_directory.clone(),
            expanded_directories: tab.expanded_directories.clone(),
            view_mode: BrowserViewMode::Columns,
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

    fn ctrl_shift_modifiers() -> keyboard::Modifiers {
        keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT
    }

    fn assert_overlay_bounds(
        bounds: SplitOverlayBounds,
        top_left_x: f32,
        top_left_y: f32,
        width: f32,
        height: f32,
    ) {
        const EPSILON: f32 = 0.01;
        assert!((bounds.top_left.x - top_left_x).abs() < EPSILON);
        assert!((bounds.top_left.y - top_left_y).abs() < EPSILON);
        assert!((bounds.width - width).abs() < EPSILON);
        assert!((bounds.height - height).abs() < EPSILON);
    }

    #[test]
    fn pane_drag_merge_overlay_covers_content_center_region() {
        let mut browser = split_browser_for_test();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: Some(PaneDropTarget::Merge(BrowserPaneId(1))),
        });

        let bounds = browser
            .pane_drag_overlay_bounds()
            .expect("merge target has overlay bounds");

        assert_overlay_bounds(bounds, 56.0, 28.0, 88.0, 44.0);
    }

    #[test]
    fn pane_drag_content_center_targets_merge() {
        let mut browser = split_browser_for_test();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: None,
        });

        browser.update_pane_drag(Point::new(100.0, 50.0));

        assert_eq!(
            browser.pane_drag.as_ref().and_then(|drag| drag.target),
            Some(PaneDropTarget::Merge(BrowserPaneId(1)))
        );
    }

    #[test]
    fn pane_drag_split_overlay_keeps_result_region_bounds() {
        let mut browser = split_browser_for_test();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: Some(PaneDropTarget::Split(SplitRegion::Right)),
        });

        let bounds = browser
            .pane_drag_overlay_bounds()
            .expect("split target has overlay bounds");

        assert_overlay_bounds(bounds, 100.0, 0.0, 100.0, 100.0);
    }

    #[test]
    fn ctrl_shift_during_file_drag_promotes_to_pane_drag() {
        let mut browser = split_browser_for_test();
        let source = PathBuf::from("/workspace/left/report.txt");
        browser.cursor_position = Point::new(190.0, 50.0);
        browser.file_drag = Some(FileDragState {
            gesture_id: crate::model::FileDragGestureId(1),
            source_pane_id: browser.active_pane_id(),
            source_tab_id: browser.active_tab_id,
            sources: vec![source.clone()],
            pressed_path: source,
            bookmark_source: None,
            stationary_action: crate::model::FileDragStationaryAction::SelectionOnly,
            phase: FileDragPhase::Dragging,
            native_dnd: FileDragNativeDndState::NotRequested,
            column_directories_snapshot: Vec::new(),
            press_origin: iced::Point::ORIGIN,
            preview_entries: Vec::new(),
        });

        drop(browser.update(Message::KeyboardModifiersChanged(ctrl_shift_modifiers())));

        assert!(browser.file_drag.is_none());
        let pane_drag = browser.pane_drag.as_ref().expect("pane drag starts");
        assert_eq!(pane_drag.source_pane_id, BrowserPaneId::PRIMARY);
        assert!(pane_drag.is_dragging());
        assert_eq!(
            pane_drag.target,
            Some(PaneDropTarget::Split(SplitRegion::Right))
        );
    }

    #[test]
    fn ctrl_shift_during_marquee_drag_promotes_to_pane_drag() {
        let mut browser = split_browser_for_test();
        let anchor = PathBuf::from("/workspace/left/report.txt");
        browser.cursor_position = Point::new(190.0, 50.0);
        browser.drag_selection_anchor = Some(anchor);
        browser.selection_marquee = Some(SelectionMarquee {
            gesture_origin: Point::new(10.0, 10.0),
            start: Point::new(10.0, 10.0),
            current: browser.cursor_position,
            source: SelectionMarqueeSource::PaneBlank,
            phase: SelectionMarqueePhase::Selecting,
            scroll_anchor: crate::model::SelectionMarqueeScrollAnchor::List {
                pane_id: BrowserPaneId::PRIMARY,
                offset_y: 0.0,
            },
            base_selection: HashSet::new(),
            preserve_existing: false,
        });

        drop(browser.update(Message::KeyboardModifiersChanged(ctrl_shift_modifiers())));

        assert!(browser.selection_marquee.is_none());
        assert!(browser.drag_selection_anchor.is_none());
        let pane_drag = browser.pane_drag.as_ref().expect("pane drag starts");
        assert_eq!(pane_drag.source_pane_id, BrowserPaneId::PRIMARY);
        assert!(pane_drag.is_dragging());
        assert_eq!(
            pane_drag.target,
            Some(PaneDropTarget::Split(SplitRegion::Right))
        );
    }

    #[test]
    fn ctrl_shift_during_column_placeholder_press_promotes_to_pane_drag() {
        let mut browser = split_browser_for_test();
        let selected_path = PathBuf::from("/workspace/left/report.txt");
        browser.selected = Some(selected_path.clone());
        browser.selected_paths = HashSet::from([selected_path.clone()]);
        browser.selection_anchor = Some(selected_path);
        browser.deepest_open_column_directory = Some(PathBuf::from("/workspace/left/src"));
        browser.cursor_position = Point::new(10.0, 50.0);

        drop(browser.update(Message::ColumnPlaceholderPressed(BrowserPaneId::PRIMARY)));
        drop(browser.update(Message::CursorMoved {
            window: browser.main_window,
            position: Point::new(190.0, 50.0),
        }));

        assert!(browser.selection_marquee.is_none());
        assert!(browser.pane_drag.is_none());

        drop(browser.update(Message::KeyboardModifiersChanged(ctrl_shift_modifiers())));

        assert!(browser.pane_drag_pointer_press.is_none());
        assert!(browser.selection_marquee.is_none());
        assert_eq!(
            browser.deepest_open_column_directory,
            Some(PathBuf::from("/workspace/left/src"))
        );
        assert!(browser
            .selected_paths
            .contains(&PathBuf::from("/workspace/left/report.txt")));
        let pane_drag = browser.pane_drag.as_ref().expect("pane drag starts");
        assert_eq!(pane_drag.source_pane_id, BrowserPaneId::PRIMARY);
        assert!(pane_drag.is_dragging());
        assert_eq!(
            pane_drag.target,
            Some(PaneDropTarget::Split(SplitRegion::Right))
        );
    }

    fn split_browser_with_save_enabled() -> FileBrowser {
        let mut config = config::ui_thread_startup_config();
        config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
        config.save_view_state = config.startup_location_policy.saves_view_state();
        split_browser_for_test_with(config)
    }

    #[test]
    fn pane_drag_merge_finish_requests_session_save() {
        let mut browser = split_browser_with_save_enabled();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: Some(PaneDropTarget::Merge(BrowserPaneId(1))),
        });

        drop(browser.finish_pane_drag());

        assert!(matches!(
            browser.pane_layout,
            BrowserPaneLayout::Single { .. }
        ));
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn pane_drag_split_region_finish_requests_session_save() {
        let mut browser = split_browser_with_save_enabled();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: Some(PaneDropTarget::Split(SplitRegion::Right)),
        });

        drop(browser.finish_pane_drag());

        let BrowserPaneLayout::Split { first, second, .. } = browser.pane_layout else {
            panic!("expected split pane layout");
        };
        // 拖到 Right 区域：源窗格排第二位，另一栏排第一。
        assert_eq!(second, BrowserPaneId::PRIMARY);
        assert_eq!(first, BrowserPaneId(1));
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn pane_drag_without_drop_target_does_not_request_session_save() {
        let mut browser = split_browser_with_save_enabled();
        browser.pane_drag = Some(PaneDragState {
            source_pane_id: BrowserPaneId::PRIMARY,
            phase: FileDragPhase::Dragging,
            target: None,
        });

        drop(browser.finish_pane_drag());

        assert!(!browser.pending_browser_session_save);
    }
}
