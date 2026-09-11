use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_core::{DirectoryDiscovery, DirectoryEntry, EntryMetadata};
use iced::Point;

use super::tabs::{TabAnimationState, TabBarReveal};
use super::FileBrowser;
use crate::model::{
    displayed_address_directory, AddressEditingSession, BrowserPane, BrowserPaneId,
    BrowserPaneLayout, BrowserTab, BrowserViewMode, DirectoryCollectionPhase,
    DirectoryLoadingPlaceholder, ExpandedDirectory, FileDragState, IconGridViewport, SplitAxis,
    SplitRegion,
};
use crate::thumbnail_cache::ColumnViewport;

const SPLIT_TARGET_MIN_CONTENT_WIDTH: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub(crate) enum DirectoryContentAvailability<'a> {
    Pending,
    Available(&'a [DirectoryEntry]),
}

#[derive(Clone, Copy)]
pub(crate) struct BrowserPaneView<'a> {
    pub(crate) id: BrowserPaneId,
    pub(crate) current_dir: &'a PathBuf,
    pub(crate) is_trash_view: bool,
    pub(crate) entries: &'a [DirectoryEntry],
    pub(crate) directory_discovery: Option<&'a DirectoryDiscovery>,
    pub(crate) directory_loading_placeholder: Option<&'a DirectoryLoadingPlaceholder>,
    pub(crate) selected: Option<&'a PathBuf>,
    pub(crate) selected_paths: &'a HashSet<PathBuf>,
    pub(crate) deepest_open_column_directory: Option<&'a PathBuf>,
    pub(crate) hovered_entry: Option<&'a PathBuf>,
    pub(crate) expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
    pub(crate) column_viewports: &'a HashMap<PathBuf, ColumnViewport>,
    pub(crate) icon_grid_viewport: IconGridViewport,
    pub(crate) view_mode: BrowserViewMode,
    pub(crate) tabs: &'a [BrowserTab],
    pub(crate) active_tab_id: usize,
    pub(crate) tab_animations: Option<&'a HashMap<usize, TabAnimationState>>,
    pub(crate) address_editing: Option<&'a AddressEditingSession>,
    pub(crate) address_transition_fraction: f32,
    pub(crate) address_exit_snapshot: Option<&'a str>,
    pub(crate) directory_collection_phase: DirectoryCollectionPhase,
    pub(crate) renaming: Option<&'a PathBuf>,
    pub(crate) rename_input: &'a str,
    pub(crate) file_drag: Option<&'a FileDragState>,
    pub(crate) tab_bar_reveal_fraction: f32,
}

impl<'a> BrowserPaneView<'a> {
    pub(crate) fn metadata_for_entry(&self, entry: &DirectoryEntry) -> EntryMetadata {
        let Some(index) = entry.discovery_index else {
            return entry.metadata.clone();
        };
        let discovery = entry.path.parent().and_then(|parent| {
            if parent == self.current_dir.as_path() {
                self.directory_discovery
            } else {
                self.expanded_directories
                    .get(parent)
                    .and_then(|expanded| expanded.directory_discovery.as_ref())
            }
        });
        discovery
            .and_then(|discovery| discovery.entries.get(index))
            .map(|discovered| discovered.display_entry().metadata)
            .unwrap_or_else(|| entry.metadata.clone())
    }

    pub(crate) fn current_directory_content(&self) -> DirectoryContentAvailability<'a> {
        if self.directory_collection_phase.is_discovering() && self.entries.is_empty() {
            DirectoryContentAvailability::Pending
        } else {
            DirectoryContentAvailability::Available(self.entries)
        }
    }

    pub(crate) fn address_bar_directory(&self) -> &Path {
        displayed_address_directory(
            self.current_dir,
            self.view_mode,
            self.deepest_open_column_directory,
        )
    }

    pub(crate) fn is_path_selected(&self, path: &std::path::Path) -> bool {
        self.selected_paths.contains(path)
    }

    pub(crate) fn tab_bar_should_occupy_layout(&self) -> bool {
        self.tabs.len() > 1 || self.tab_bar_reveal_fraction > f32::EPSILON
    }

    pub(crate) fn tab_width_fraction(&self, tab_id: usize) -> f32 {
        self.tab_animations
            .and_then(|animations| animations.get(&tab_id))
            .map(|animation| animation.width_fraction())
            .unwrap_or(1.0)
    }

    pub(crate) fn tab_shift_offset(&self, tab_id: usize) -> f32 {
        self.tab_animations
            .and_then(|animations| animations.get(&tab_id))
            .map(|animation| animation.shift_offset())
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SplitOverlayBounds {
    pub(crate) top_left: Point,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl FileBrowser {
    pub(crate) fn active_pane_id(&self) -> BrowserPaneId {
        self.pane_layout.active()
    }

    /// 窗格区的窗口 y 起点:全窗工具栏顶栏横贯后,窗格从它下方开始。
    pub(super) fn main_panes_area_top(&self) -> f32 {
        crate::model::MAIN_TOOLBAR_ROW_HEIGHT
    }

    pub(super) fn pane_id_at_position(&self, position: Point) -> Option<BrowserPaneId> {
        let sidebar_width = self.sidebar_width;
        let content_width = self.split_content_width();
        let panes_top = self.main_panes_area_top();
        let content_height = (self.main_window_height - panes_top).max(1.0);
        if position.x < sidebar_width
            || position.x > sidebar_width + content_width
            || position.y < panes_top
            || position.y > panes_top + content_height
        {
            return None;
        }

        match self.pane_layout {
            BrowserPaneLayout::Single { active } => Some(active),
            BrowserPaneLayout::Split {
                axis: crate::model::SplitAxis::Horizontal,
                first,
                second,
                ..
            } => {
                let local_x = position.x - sidebar_width;
                let boundary = self.pane_layout.split_divider_center(content_width);
                if local_x < boundary {
                    Some(first)
                } else {
                    Some(second)
                }
            }
            BrowserPaneLayout::Split {
                axis: crate::model::SplitAxis::Vertical,
                first,
                second,
                ..
            } => {
                let boundary = panes_top + self.pane_layout.split_divider_center(content_height);
                if position.y < boundary {
                    Some(first)
                } else {
                    Some(second)
                }
            }
        }
    }

    pub(crate) fn pane_view(&self, pane_id: BrowserPaneId) -> Option<BrowserPaneView<'_>> {
        let (address_editing, address_transition_fraction, address_exit_snapshot) =
            self.address_bar_presentation(pane_id);
        if pane_id == self.active_pane_id() {
            return Some(BrowserPaneView {
                id: pane_id,
                current_dir: &self.current_dir,
                is_trash_view: self.is_trash_view,
                entries: &self.entries,
                directory_discovery: self.directory_discovery.as_ref(),
                directory_loading_placeholder: self.directory_loading_placeholder.as_ref(),
                selected: self.selected.as_ref(),
                selected_paths: &self.selected_paths,
                deepest_open_column_directory: self.deepest_open_column_directory.as_ref(),
                hovered_entry: self.hovered_entry.as_ref(),
                expanded_directories: &self.expanded_directories,
                column_viewports: &self.column_viewports,
                icon_grid_viewport: self.icon_grid_viewport_for(pane_id, &self.current_dir),
                view_mode: self.view_mode,
                tabs: &self.tabs,
                active_tab_id: self.active_tab_id,
                tab_animations: Some(&self.tab_animations),
                address_editing,
                address_transition_fraction,
                address_exit_snapshot,
                directory_collection_phase: self.directory_collection_phase,
                renaming: self.renaming.as_ref(),
                rename_input: &self.rename_input,
                file_drag: self.file_drag.as_ref(),
                tab_bar_reveal_fraction: self.tab_bar_reveal_fraction(),
            });
        }

        let pane = self.pane_by_id(pane_id)?;
        Some(BrowserPaneView {
            id: pane.id,
            current_dir: &pane.current_dir,
            is_trash_view: pane.is_trash_view,
            entries: &pane.entries,
            directory_discovery: pane.directory_discovery.as_ref(),
            directory_loading_placeholder: pane.directory_loading_placeholder.as_ref(),
            selected: pane.selected.as_ref(),
            selected_paths: &pane.selected_paths,
            deepest_open_column_directory: pane.deepest_open_column_directory.as_ref(),
            hovered_entry: None,
            expanded_directories: &pane.expanded_directories,
            column_viewports: &pane.column_viewports,
            icon_grid_viewport: self.icon_grid_viewport_for(pane_id, &pane.current_dir),
            view_mode: pane.view_mode,
            tabs: &pane.tabs,
            active_tab_id: pane.active_tab_id,
            tab_animations: None,
            address_editing,
            address_transition_fraction,
            address_exit_snapshot,
            directory_collection_phase: pane.directory_collection_phase,
            renaming: None,
            rename_input: "",
            file_drag: None,
            tab_bar_reveal_fraction: if pane.tabs.len() > 1 { 1.0 } else { 0.0 },
        })
    }

    fn address_bar_presentation(
        &self,
        pane_id: BrowserPaneId,
    ) -> (Option<&AddressEditingSession>, f32, Option<&str>) {
        let editing = self
            .address_editing
            .as_ref()
            .filter(|session| session.pane_id == pane_id);
        let transition = self
            .address_bar_transition
            .as_ref()
            .filter(|transition| transition.pane_id == pane_id);
        let editing_fraction = transition
            .map(|transition| transition.fraction())
            .unwrap_or_else(|| if editing.is_some() { 1.0 } else { 0.0 });
        let exit_snapshot = editing
            .is_none()
            .then(|| transition.and_then(|transition| transition.exit_snapshot.as_deref()))
            .flatten();

        (editing, editing_fraction, exit_snapshot)
    }

    pub(super) fn pane_by_id(&self, pane_id: BrowserPaneId) -> Option<&BrowserPane> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    pub(super) fn pane_by_id_mut(&mut self, pane_id: BrowserPaneId) -> Option<&mut BrowserPane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    pub(crate) fn pane_content_width(&self) -> f32 {
        self.pane_content_width_for(self.active_pane_id())
    }

    pub(crate) fn pane_content_width_for(&self, pane_id: BrowserPaneId) -> f32 {
        let content_width = self.split_content_width();
        match self.pane_layout {
            BrowserPaneLayout::Split {
                axis: SplitAxis::Horizontal,
                ..
            } => self.pane_layout.pane_extent(pane_id, content_width),
            BrowserPaneLayout::Single { .. } | BrowserPaneLayout::Split { .. } => content_width,
        }
    }

    pub(super) fn icon_grid_viewport_for(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
    ) -> IconGridViewport {
        let mut viewport = self
            .icon_grid_viewports
            .get(&pane_id)
            .filter(|state| state.directory == directory)
            .map(|state| state.viewport)
            .unwrap_or_default();
        let pane_width = self.pane_content_width_for(pane_id);
        if viewport.width <= f32::EPSILON || (viewport.width - pane_width).abs() > 1.0 {
            viewport.width = pane_width;
        }
        viewport
    }

    pub(super) fn sync_active_pane_state(&mut self) {
        let snapshot = self.capture_active_pane_snapshot();
        if let Some(pane) = self.pane_by_id_mut(snapshot.id) {
            *pane = snapshot;
        } else {
            self.panes.push(snapshot);
        }
    }

    pub(super) fn activate_pane(&mut self, pane_id: BrowserPaneId) {
        if pane_id == self.active_pane_id() || self.pane_by_id(pane_id).is_none() {
            return;
        }

        self.clear_icon_grid_expansion_for_context_change();
        self.sync_active_tab_state();
        let Some(next_pane) = self.pane_by_id(pane_id).cloned() else {
            return;
        };
        let _ = self.cancel_address_editing();
        self.pane_layout = self.pane_layout.with_active(pane_id);
        self.restore_pane_snapshot(next_pane);
    }

    pub(super) fn new_pane_id(&mut self) -> BrowserPaneId {
        let pane_id = BrowserPaneId(self.next_pane_id);
        self.next_pane_id += 1;
        pane_id
    }

    pub(super) fn split_region_at(&self, position: Point) -> Option<SplitRegion> {
        let content_width = self.split_content_width();
        if content_width <= SPLIT_TARGET_MIN_CONTENT_WIDTH || self.main_window_height <= 1.0 {
            return None;
        }
        let sidebar_width = self.sidebar_width;
        if position.x < sidebar_width || position.y < 0.0 {
            return None;
        }
        if position.x > sidebar_width + content_width || position.y > self.main_window_height {
            return None;
        }

        let local_x = ((position.x - sidebar_width) / content_width).clamp(0.0, 1.0);
        let local_y = (position.y / self.main_window_height).clamp(0.0, 1.0);
        let horizontal_bias = (local_x - 0.5).abs();
        let vertical_bias = (local_y - 0.5).abs();

        if horizontal_bias >= vertical_bias {
            if local_x < 0.5 {
                Some(SplitRegion::Left)
            } else {
                Some(SplitRegion::Right)
            }
        } else if local_y < 0.5 {
            Some(SplitRegion::Top)
        } else {
            Some(SplitRegion::Bottom)
        }
    }

    pub(crate) fn tab_split_overlay_bounds(&self) -> Option<SplitOverlayBounds> {
        let drag = self.tab_drag.as_ref()?;
        if !drag.is_dragging() {
            return None;
        }
        let target = drag.split_target?;
        let content_width = self.split_content_width();
        let panes_top = self.main_panes_area_top();
        let content_height = (self.main_window_height - panes_top).max(1.0);
        let sidebar_width = self.sidebar_width;
        let half_width = content_width / 2.0;
        let half_height = content_height / 2.0;

        let bounds = match target.region {
            SplitRegion::Left => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, panes_top),
                width: half_width,
                height: content_height,
            },
            SplitRegion::Right => SplitOverlayBounds {
                top_left: Point::new(sidebar_width + half_width, panes_top),
                width: half_width,
                height: content_height,
            },
            SplitRegion::Top => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, panes_top),
                width: content_width,
                height: half_height,
            },
            SplitRegion::Bottom => SplitOverlayBounds {
                top_left: Point::new(sidebar_width, panes_top + half_height),
                width: content_width,
                height: half_height,
            },
        };
        Some(bounds)
    }

    pub(super) fn capture_active_pane_snapshot(&self) -> BrowserPane {
        BrowserPane {
            id: self.active_pane_id(),
            current_dir: self.current_dir.clone(),
            is_trash_view: self.is_trash_view,
            entries: self.entries.clone(),
            directory_discovery: self.directory_discovery.clone(),
            directory_loading_placeholder: self.directory_loading_placeholder.clone(),
            trash_entries: self.trash_entries.clone(),
            selected: self.selected.clone(),
            selected_paths: self.selected_paths.clone(),
            selection_anchor: self.selection_anchor.clone(),
            deepest_open_column_directory: self.deepest_open_column_directory.clone(),
            expanded_directories: self.expanded_directories.clone(),
            view_mode: self.view_mode,
            column_browser_viewport: self.column_browser_viewport,
            column_viewports: self.column_viewports.clone(),
            tabs: self.tabs.clone(),
            active_tab_id: self.active_tab_id,
            directory_load_generation: self.directory_load_generation,
            directory_load_cancel: self.directory_load_cancel.clone(),
            back_stack: self.back_stack.clone(),
            forward_stack: self.forward_stack.clone(),
            directory_collection_phase: self.directory_collection_phase,
            directory_order_phase: self.directory_order_phase,
        }
    }

    pub(super) fn restore_pane_snapshot(&mut self, pane: BrowserPane) {
        self.clear_icon_grid_expansion();
        self.apply_pane_browsing_snapshot(pane);
        self.tab_animations.clear();
        self.hovered_entry = None;
        self.cursor_paste_directory = None;
        self.clear_column_interaction_context();
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.column_resize_drag = None;
        self.pending_keyboard_column_focus = None;
        self.last_activation_click = None;
    }

    pub(super) fn apply_pane_browsing_snapshot(&mut self, pane: BrowserPane) {
        self.current_dir = pane.current_dir;
        self.is_trash_view = pane.is_trash_view;
        self.set_entries(pane.entries);
        self.directory_discovery = pane.directory_discovery;
        self.directory_loading_placeholder = pane.directory_loading_placeholder;
        self.trash_entries = pane.trash_entries;
        self.selected = pane.selected;
        self.selected_paths = pane.selected_paths;
        self.selection_anchor = pane.selection_anchor;
        self.deepest_open_column_directory = pane.deepest_open_column_directory;
        self.expanded_directories = pane.expanded_directories;
        self.view_mode = pane.view_mode;
        self.column_browser_viewport = pane.column_browser_viewport;
        self.column_viewports = pane.column_viewports;
        self.tabs = pane.tabs;
        self.active_tab_id = pane.active_tab_id;
        self.directory_load_generation = pane.directory_load_generation;
        self.directory_load_cancel = pane.directory_load_cancel;
        self.back_stack = pane.back_stack;
        self.forward_stack = pane.forward_stack;
        self.directory_collection_phase = pane.directory_collection_phase;
        self.directory_order_phase = pane.directory_order_phase;
        self.tab_bar_reveal = if self.tabs.len() > 1 {
            TabBarReveal::Visible
        } else {
            TabBarReveal::Hidden
        };
    }

    pub(super) fn split_content_width(&self) -> f32 {
        (self.main_window_width - self.sidebar_width).max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::*;

    #[test]
    fn directory_content_distinguishes_pending_empty_and_streamed_entries() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let pane_id = browser.active_pane_id();

        Arc::make_mut(&mut browser.entries).clear();
        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Discovering;
        assert!(matches!(
            browser
                .pane_view(pane_id)
                .expect("active pane")
                .current_directory_content(),
            DirectoryContentAvailability::Pending
        ));

        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
        let DirectoryContentAvailability::Available(entries) = browser
            .pane_view(pane_id)
            .expect("active pane")
            .current_directory_content()
        else {
            panic!("loaded empty directory must be available");
        };
        assert!(entries.is_empty());

        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Discovering;
        Arc::make_mut(&mut browser.entries).push(DirectoryEntry::new(
            PathBuf::from("/streamed.txt"),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        ));
        let DirectoryContentAvailability::Available(entries) = browser
            .pane_view(pane_id)
            .expect("active pane")
            .current_directory_content()
        else {
            panic!("a streamed batch must become visible before completion");
        };
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn tab_and_pane_snapshots_share_directory_entries() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.entries = Arc::new(vec![DirectoryEntry::new(
            PathBuf::from("/report.txt"),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )]);

        browser.sync_active_tab_state();
        let active_tab = browser
            .tabs
            .iter()
            .find(|tab| tab.id == browser.active_tab_id)
            .expect("active tab");
        assert!(Arc::ptr_eq(&browser.entries, &active_tab.entries));

        let pane = browser.capture_active_pane_snapshot();
        assert!(Arc::ptr_eq(&browser.entries, &pane.entries));
    }

    #[test]
    fn unequal_split_uses_same_boundary_for_hit_test_and_pane_width() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.main_window_width = 1_000.0;
        browser.main_window_height = 800.0;
        browser.sidebar_width = 200.0;
        browser.pane_layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 700,
        };

        assert_eq!(
            browser.pane_id_at_position(Point::new(700.0, 400.0)),
            Some(BrowserPaneId::PRIMARY)
        );
        assert_eq!(
            browser.pane_id_at_position(Point::new(780.0, 400.0)),
            Some(BrowserPaneId(1))
        );
        assert!(
            browser.pane_content_width_for(BrowserPaneId::PRIMARY)
                > browser.pane_content_width_for(BrowserPaneId(1))
        );
    }

    #[test]
    fn unequal_vertical_split_uses_shared_boundary_for_hit_test() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.main_window_width = 1_000.0;
        browser.main_window_height = 800.0;
        browser.sidebar_width = 200.0;
        browser.pane_layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 300,
        };

        assert_eq!(
            browser.pane_id_at_position(Point::new(600.0, 200.0)),
            Some(BrowserPaneId::PRIMARY)
        );
        assert_eq!(
            browser.pane_id_at_position(Point::new(600.0, 300.0)),
            Some(BrowserPaneId(1))
        );
    }

    #[test]
    fn directory_content_views_never_render_loading_copy() {
        let directory_view_sources = [
            include_str!("../list_view.rs"),
            include_str!("../icon_grid_view.rs"),
            include_str!("../three_column_view.rs"),
            include_str!("../view/preview_panel.rs"),
        ];

        assert!(directory_view_sources
            .iter()
            .all(|source| !source.contains("\"Loading...\"")));
    }
}
