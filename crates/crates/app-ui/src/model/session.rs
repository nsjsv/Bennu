use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_operation_store::{
    StoredBrowserPane, StoredBrowserPaneLayout, StoredBrowserSession, StoredBrowserTab,
    StoredBrowserViewMode, StoredColumnBrowserViewport, StoredColumnViewport, StoredPath,
    StoredSplitAxis,
};

use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode,
    ColumnBrowserViewport, ExpandedDirectory, ExpandedDirectoryStatus, SplitAxis,
};
use crate::thumbnail_cache::ColumnViewport;

#[derive(Debug, Clone)]
pub(crate) struct BrowserSessionSnapshot {
    pub(crate) panes: Vec<BrowserPaneSession>,
    pub(crate) layout: BrowserPaneLayout,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserPaneSession {
    pub(crate) id: BrowserPaneId,
    pub(crate) tabs: Vec<BrowserTabSession>,
    pub(crate) active_tab_id: usize,
    pub(crate) column_browser_viewport: ColumnBrowserViewport,
    pub(crate) column_viewports: HashMap<PathBuf, ColumnViewport>,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserTabSession {
    pub(crate) id: usize,
    pub(crate) directory: PathBuf,
    pub(crate) is_trash_view: bool,
    pub(crate) selected: Option<PathBuf>,
    pub(crate) selected_paths: HashSet<PathBuf>,
    pub(crate) deepest_open_column_directory: Option<PathBuf>,
    pub(crate) expanded_directories: Vec<PathBuf>,
    pub(crate) view_mode: BrowserViewMode,
    pub(crate) back_stack: Vec<PathBuf>,
    pub(crate) forward_stack: Vec<PathBuf>,
}

impl BrowserTabSession {
    pub(crate) fn to_browser_tab(&self) -> BrowserTab {
        let mut tab = if self.is_trash_view {
            BrowserTab::trash(self.id)
        } else {
            BrowserTab::directory(self.id, self.directory.clone())
        };
        tab.selected = self.selected.clone();
        tab.selected_paths = self.selected_paths.clone();
        tab.selection_anchor = self.selected.clone();
        tab.deepest_open_column_directory = self.deepest_open_column_directory.clone();
        tab.expanded_directories = self.restored_expanded_directories();
        tab.view_mode = self.view_mode;
        tab.back_stack = self.back_stack.clone();
        tab.forward_stack = self.forward_stack.clone();
        tab
    }

    pub(crate) fn restored_expanded_directories(&self) -> HashMap<PathBuf, ExpandedDirectory> {
        let mut directories = self.expanded_directories.clone();
        if self.view_mode == BrowserViewMode::Columns && !self.is_trash_view {
            if let Some(open_directory) = &self.deepest_open_column_directory {
                directories.extend(restored_column_chain_directories(
                    self.directory.as_path(),
                    open_directory.as_path(),
                ));
            }
        }

        directories
            .into_iter()
            .filter(|path| path != &self.directory && path.starts_with(self.directory.as_path()))
            .map(|path| (path, restored_expanded_directory()))
            .collect()
    }
}

pub(crate) fn snapshot_from_stored(stored: StoredBrowserSession) -> Option<BrowserSessionSnapshot> {
    let panes = stored
        .panes
        .into_iter()
        .filter_map(pane_from_stored)
        .collect::<Vec<_>>();
    if panes.is_empty() {
        return None;
    }
    Some(BrowserSessionSnapshot {
        panes,
        layout: layout_from_stored(stored.layout),
    })
}

pub(crate) fn snapshot_to_stored(snapshot: BrowserSessionSnapshot) -> Option<StoredBrowserSession> {
    let panes = snapshot
        .panes
        .into_iter()
        .map(pane_to_stored)
        .collect::<Option<Vec<_>>>()?;
    Some(StoredBrowserSession {
        panes,
        layout: layout_to_stored(snapshot.layout),
    })
}

pub(crate) fn pane_session_from_live(pane: &BrowserPane) -> BrowserPaneSession {
    BrowserPaneSession {
        id: pane.id,
        tabs: pane.tabs.iter().map(tab_session_from_live).collect(),
        active_tab_id: pane.active_tab_id,
        column_browser_viewport: pane.column_browser_viewport,
        column_viewports: pane.column_viewports.clone(),
    }
}

pub(crate) fn tab_session_from_live(tab: &BrowserTab) -> BrowserTabSession {
    BrowserTabSession {
        id: tab.id,
        directory: tab.directory.clone(),
        is_trash_view: tab.is_trash_view,
        selected: tab.selected.clone(),
        selected_paths: tab.selected_paths.clone(),
        deepest_open_column_directory: tab.deepest_open_column_directory.clone(),
        expanded_directories: tab.expanded_directories.keys().cloned().collect(),
        view_mode: tab.view_mode,
        back_stack: tab.back_stack.clone(),
        forward_stack: tab.forward_stack.clone(),
    }
}

fn pane_from_stored(stored: StoredBrowserPane) -> Option<BrowserPaneSession> {
    let id = BrowserPaneId(stored.id);
    let active_tab_id = usize::try_from(stored.active_tab_id).ok()?;
    let tabs = stored
        .tabs
        .into_iter()
        .filter_map(tab_from_stored)
        .collect::<Vec<_>>();
    if tabs.is_empty() {
        return None;
    }
    let column_viewports = stored
        .column_viewports
        .into_iter()
        .map(|viewport| {
            (
                viewport.directory.to_path_buf(),
                ColumnViewport {
                    offset_y: viewport.offset_y,
                    height: viewport.height,
                },
            )
        })
        .collect();
    Some(BrowserPaneSession {
        id,
        tabs,
        active_tab_id,
        column_browser_viewport: ColumnBrowserViewport {
            offset_x: stored.column_browser_viewport.offset_x,
            width: stored.column_browser_viewport.width,
        },
        column_viewports,
    })
}

fn tab_from_stored(stored: StoredBrowserTab) -> Option<BrowserTabSession> {
    Some(BrowserTabSession {
        id: usize::try_from(stored.id).ok()?,
        directory: stored.directory.to_path_buf(),
        is_trash_view: stored.is_trash_view,
        selected: stored.selected.map(|path| path.to_path_buf()),
        selected_paths: stored
            .selected_paths
            .into_iter()
            .map(|path| path.to_path_buf())
            .collect(),
        deepest_open_column_directory: stored
            .deepest_open_column_directory
            .map(|path| path.to_path_buf()),
        expanded_directories: stored
            .expanded_directories
            .into_iter()
            .map(|path| path.to_path_buf())
            .collect(),
        view_mode: view_mode_from_stored(stored.view_mode),
        back_stack: stored
            .back_stack
            .into_iter()
            .map(|path| path.to_path_buf())
            .collect(),
        forward_stack: stored
            .forward_stack
            .into_iter()
            .map(|path| path.to_path_buf())
            .collect(),
    })
}

fn pane_to_stored(pane: BrowserPaneSession) -> Option<StoredBrowserPane> {
    Some(StoredBrowserPane {
        id: pane.id.key(),
        tabs: pane
            .tabs
            .into_iter()
            .map(tab_to_stored)
            .collect::<Option<Vec<_>>>()?,
        active_tab_id: u64::try_from(pane.active_tab_id).ok()?,
        column_browser_viewport: StoredColumnBrowserViewport {
            offset_x: pane.column_browser_viewport.offset_x,
            width: pane.column_browser_viewport.width,
        },
        column_viewports: pane
            .column_viewports
            .into_iter()
            .map(|(directory, viewport)| StoredColumnViewport {
                directory: StoredPath::from_path(&directory),
                offset_y: viewport.offset_y,
                height: viewport.height,
            })
            .collect(),
    })
}

fn tab_to_stored(tab: BrowserTabSession) -> Option<StoredBrowserTab> {
    Some(StoredBrowserTab {
        id: u64::try_from(tab.id).ok()?,
        directory: StoredPath::from_path(&tab.directory),
        is_trash_view: tab.is_trash_view,
        selected: tab.selected.as_deref().map(StoredPath::from_path),
        selected_paths: tab
            .selected_paths
            .iter()
            .map(|path| StoredPath::from_path(path))
            .collect(),
        deepest_open_column_directory: tab
            .deepest_open_column_directory
            .as_deref()
            .map(StoredPath::from_path),
        expanded_directories: tab
            .expanded_directories
            .iter()
            .map(|path| StoredPath::from_path(path))
            .collect(),
        view_mode: view_mode_to_stored(tab.view_mode),
        back_stack: tab
            .back_stack
            .iter()
            .map(|path| StoredPath::from_path(path))
            .collect(),
        forward_stack: tab
            .forward_stack
            .iter()
            .map(|path| StoredPath::from_path(path))
            .collect(),
    })
}

fn layout_from_stored(layout: StoredBrowserPaneLayout) -> BrowserPaneLayout {
    match layout {
        StoredBrowserPaneLayout::Single { active } => BrowserPaneLayout::Single {
            active: BrowserPaneId(active),
        },
        StoredBrowserPaneLayout::Split {
            axis,
            first,
            second,
            active,
            first_portion,
        } => BrowserPaneLayout::Split {
            axis: match axis {
                StoredSplitAxis::Horizontal => SplitAxis::Horizontal,
                StoredSplitAxis::Vertical => SplitAxis::Vertical,
            },
            first: BrowserPaneId(first),
            second: BrowserPaneId(second),
            active: BrowserPaneId(active),
            first_portion: super::split_layout::normalized_split_portion(first_portion),
        },
    }
}

fn layout_to_stored(layout: BrowserPaneLayout) -> StoredBrowserPaneLayout {
    match layout {
        BrowserPaneLayout::Single { active } => StoredBrowserPaneLayout::Single {
            active: active.key(),
        },
        BrowserPaneLayout::Split {
            axis,
            first,
            second,
            active,
            first_portion,
        } => StoredBrowserPaneLayout::Split {
            axis: match axis {
                SplitAxis::Horizontal => StoredSplitAxis::Horizontal,
                SplitAxis::Vertical => StoredSplitAxis::Vertical,
            },
            first: first.key(),
            second: second.key(),
            active: active.key(),
            first_portion,
        },
    }
}

fn view_mode_from_stored(view_mode: StoredBrowserViewMode) -> BrowserViewMode {
    match view_mode {
        StoredBrowserViewMode::Columns => BrowserViewMode::Columns,
        StoredBrowserViewMode::List => BrowserViewMode::List,
        StoredBrowserViewMode::Icons => BrowserViewMode::Icons,
    }
}

fn view_mode_to_stored(view_mode: BrowserViewMode) -> StoredBrowserViewMode {
    match view_mode {
        BrowserViewMode::Columns => StoredBrowserViewMode::Columns,
        BrowserViewMode::List => StoredBrowserViewMode::List,
        BrowserViewMode::Icons => StoredBrowserViewMode::Icons,
    }
}

fn restored_column_chain_directories(current_dir: &Path, open_directory: &Path) -> Vec<PathBuf> {
    if open_directory == current_dir || !open_directory.starts_with(current_dir) {
        return Vec::new();
    }

    let mut ancestors = Vec::new();
    let mut cursor = Some(open_directory);
    while let Some(path) = cursor {
        if path == current_dir {
            break;
        }
        if !path.starts_with(current_dir) {
            break;
        }
        ancestors.push(path.to_path_buf());
        cursor = path.parent();
    }
    ancestors.reverse();
    ancestors
}

fn restored_expanded_directory() -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loading,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_view_mode_round_trips_through_stored_session_value() {
        assert_eq!(
            view_mode_from_stored(StoredBrowserViewMode::Icons),
            BrowserViewMode::Icons
        );
        assert_eq!(
            view_mode_to_stored(BrowserViewMode::Icons),
            StoredBrowserViewMode::Icons
        );
    }

    #[test]
    fn split_portion_round_trips_through_stored_session_value() {
        let layout = BrowserPaneLayout::Split {
            axis: SplitAxis::Vertical,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId(1),
            first_portion: 731,
        };

        assert_eq!(layout_from_stored(layout_to_stored(layout)), layout);
    }

    #[test]
    fn stored_split_portion_is_normalized_at_restore_boundary() {
        let layout = layout_from_stored(StoredBrowserPaneLayout::Split {
            axis: StoredSplitAxis::Horizontal,
            first: 0,
            second: 1,
            active: 0,
            first_portion: u16::MAX,
        });

        assert_eq!(layout.first_portion(), 999);
    }
}
