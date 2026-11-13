use std::path::Path;

use iced::Point;

use super::super::sidebar_bookmarks::{
    sidebar_bookmark_row_pointer_target, SidebarBookmarkRowPointerTarget,
};
use super::FileBrowser;
use crate::model::{
    FileDragHitTestBounds, FileDropEntryTargetBounds, FileDropHitTestBounds, FileDropTarget,
    SidebarBookmarkDropSlot,
};

pub(super) fn freeze_file_drop_hit_test_bounds(
    browser: &FileBrowser,
    measured: FileDragHitTestBounds,
) -> FileDropHitTestBounds {
    FileDropHitTestBounds {
        tabs: measured.tabs,
        entries: measured
            .entries
            .into_iter()
            .filter_map(|entry| {
                browser
                    .pane_accepts_file_drag(entry.pane_id)
                    .then(|| {
                        browser.directory_drop_target_for_entry_in_pane(entry.pane_id, &entry.path)
                    })
                    .flatten()
                    .map(|directory| FileDropEntryTargetBounds {
                        pane_id: entry.pane_id,
                        directory,
                        path: entry.path,
                        bounds: entry.bounds,
                    })
            })
            .collect(),
        breadcrumbs: measured
            .breadcrumbs
            .into_iter()
            .filter(|target| browser.pane_accepts_file_drag(target.pane_id))
            .collect(),
        directory_targets: measured
            .directory_targets
            .into_iter()
            .filter(|target| browser.pane_accepts_file_drag(target.pane_id))
            .collect(),
        blocked_directories: measured
            .blocked_directories
            .into_iter()
            .filter(|target| browser.pane_accepts_file_drag(target.pane_id))
            .collect(),
        sidebar_directories: measured.sidebar_directories,
        empty_sidebar_bookmarks: measured.empty_sidebar_bookmarks,
        local_send: measured.local_send,
    }
}

pub(super) fn resolve_file_drop_target(
    bounds: &FileDropHitTestBounds,
    position: Point,
    bookmark_source: Option<&Path>,
) -> Option<FileDropTarget> {
    if let Some(tab) = bounds
        .tabs
        .iter()
        .rev()
        .find(|tab| tab.bounds.contains(position))
    {
        return Some(FileDropTarget::Tab(tab.target.clone()));
    }
    if bounds
        .local_send
        .is_some_and(|area| area.contains(position))
    {
        return Some(FileDropTarget::LocalSend);
    }

    if let Some(bookmark_source) = bookmark_source {
        if bounds
            .empty_sidebar_bookmarks
            .is_some_and(|area| area.contains(position))
        {
            return Some(FileDropTarget::SidebarBookmarkSlot(
                SidebarBookmarkDropSlot::Insert { index: 0 },
            ));
        }
        if let Some(target) = bounds
            .sidebar_directories
            .iter()
            .rev()
            .find(|target| target.bounds.contains(position))
        {
            let Some(favorite_index) = target.favorite_index else {
                return Some(FileDropTarget::Directory(target.directory.clone()));
            };
            return match sidebar_bookmark_row_pointer_target(
                position.y,
                target.bounds.y,
                target.bounds.height,
            ) {
                SidebarBookmarkRowPointerTarget::InsertBefore => Some(
                    FileDropTarget::SidebarBookmarkSlot(SidebarBookmarkDropSlot::Insert {
                        index: favorite_index,
                    }),
                ),
                SidebarBookmarkRowPointerTarget::Directory => (target.directory
                    != *bookmark_source)
                    .then(|| FileDropTarget::Directory(target.directory.clone())),
                SidebarBookmarkRowPointerTarget::InsertAfter => Some(
                    FileDropTarget::SidebarBookmarkSlot(SidebarBookmarkDropSlot::Insert {
                        index: favorite_index + 1,
                    }),
                ),
            };
        }
    } else if let Some(target) = bounds
        .sidebar_directories
        .iter()
        .rev()
        .find(|target| target.bounds.contains(position))
    {
        return Some(FileDropTarget::Directory(target.directory.clone()));
    }

    if let Some(target) = bounds
        .breadcrumbs
        .iter()
        .filter(|target| {
            target.viewport_bounds.contains(position) && target.item_bounds.contains(position)
        })
        .max_by_key(|target| target.directory.components().count())
    {
        return Some(FileDropTarget::Directory(target.directory.clone()));
    }
    if bounds
        .blocked_directories
        .iter()
        .any(|target| target.bounds.contains(position))
    {
        return None;
    }
    if let Some(target) = bounds
        .entries
        .iter()
        .rev()
        .find(|target| target.bounds.contains(position))
    {
        return Some(FileDropTarget::Directory(target.directory.clone()));
    }
    bounds
        .directory_targets
        .iter()
        .rev()
        .find(|target| target.bounds.contains(position))
        .map(|target| FileDropTarget::Directory(target.directory.clone()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use iced::{Rectangle, Size};

    use super::*;
    use crate::model::{
        BreadcrumbDropTargetBounds, TabDropDestination, TabFileDropTarget, TabFileDropTargetBounds,
    };

    #[test]
    fn tab_target_has_priority_over_page_content() {
        let tab_target = TabFileDropTarget {
            pane_id: crate::model::BrowserPaneId(0),
            tab_id: 7,
            destination: TabDropDestination::Directory(PathBuf::from("/target")),
        };
        let bounds = FileDropHitTestBounds {
            tabs: vec![TabFileDropTargetBounds {
                target: tab_target.clone(),
                bounds: Rectangle::new(Point::new(10.0, 10.0), Size::new(100.0, 30.0)),
            }],
            directory_targets: vec![crate::model::DirectoryFileDragTargetBounds {
                pane_id: crate::model::BrowserPaneId(0),
                directory: PathBuf::from("/content"),
                bounds: Rectangle::new(Point::new(0.0, 0.0), Size::new(200.0, 200.0)),
            }],
            ..FileDropHitTestBounds::default()
        };

        assert_eq!(
            resolve_file_drop_target(&bounds, Point::new(20.0, 20.0), None),
            Some(FileDropTarget::Tab(tab_target))
        );
    }

    #[test]
    fn sidebar_local_send_entry_resolves_to_local_send() {
        let bounds = FileDropHitTestBounds {
            local_send: Some(Rectangle::new(
                Point::new(0.0, 40.0),
                Size::new(180.0, 36.0),
            )),
            directory_targets: vec![crate::model::DirectoryFileDragTargetBounds {
                pane_id: crate::model::BrowserPaneId(0),
                directory: PathBuf::from("/content"),
                bounds: Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 400.0)),
            }],
            ..FileDropHitTestBounds::default()
        };

        assert_eq!(
            resolve_file_drop_target(&bounds, Point::new(20.0, 50.0), None),
            Some(FileDropTarget::LocalSend)
        );
        assert_eq!(
            resolve_file_drop_target(&bounds, Point::new(20.0, 100.0), None),
            Some(FileDropTarget::Directory(PathBuf::from("/content")))
        );
    }

    #[test]
    fn overlapping_breadcrumbs_choose_deepest_directory() {
        let pane_id = crate::model::BrowserPaneId(0);
        let hit_area = Rectangle::new(Point::new(0.0, 0.0), Size::new(200.0, 40.0));
        let deepest = PathBuf::from("/workspace/project/src");
        let bounds = FileDropHitTestBounds {
            breadcrumbs: vec![
                BreadcrumbDropTargetBounds {
                    pane_id,
                    directory: deepest.clone(),
                    item_bounds: hit_area,
                    viewport_bounds: hit_area,
                },
                BreadcrumbDropTargetBounds {
                    pane_id,
                    directory: PathBuf::from("/workspace"),
                    item_bounds: hit_area,
                    viewport_bounds: hit_area,
                },
            ],
            ..FileDropHitTestBounds::default()
        };

        assert_eq!(
            resolve_file_drop_target(&bounds, Point::new(20.0, 20.0), None),
            Some(FileDropTarget::Directory(deepest))
        );
    }
}
