use std::any::Any;
use std::path::PathBuf;

use iced::advanced::widget;
use iced::advanced::widget::operation::Outcome;
use iced::{Rectangle, Task, Vector};

use crate::model::{
    BreadcrumbDropTargetBounds, BrowserPaneId, ColumnEntryBounds, DirectoryFileDragTargetBounds,
    FileDragBlockedDirectoryBounds, FileDragHitTestBounds, FileDropLayoutRequest, Message,
    SidebarFileDragTargetBounds, TabFileDropTarget, TabFileDropTargetBounds,
};

#[derive(Debug, Clone)]
pub(crate) enum FileDragHitTestMarker {
    Tab {
        target: TabFileDropTarget,
    },
    ColumnEntry {
        pane_id: BrowserPaneId,
        path: PathBuf,
    },
    BreadcrumbViewport {
        pane_id: BrowserPaneId,
    },
    BreadcrumbDirectory {
        pane_id: BrowserPaneId,
        directory: PathBuf,
    },
    DirectoryTarget {
        pane_id: BrowserPaneId,
        directory: PathBuf,
    },
    BlockedDirectoryTarget {
        pane_id: BrowserPaneId,
    },
    SidebarDirectory {
        directory: PathBuf,
        favorite_index: Option<usize>,
    },
    EmptySidebarBookmarks,
}

pub(crate) enum FileDragHitTestBoundsRequest {
    SelectionMarquee,
    Breadcrumbs(u64),
    FileDropLayout(FileDropLayoutRequest),
}

pub(crate) fn file_drag_hit_test_bounds_command(
    request: FileDragHitTestBoundsRequest,
) -> Task<Message> {
    widget::operate(FileDragHitTestBoundsOperation::new(request))
}

#[derive(Debug, Clone)]
struct MeasuredBreadcrumbDirectory {
    pane_id: BrowserPaneId,
    directory: PathBuf,
    bounds: Rectangle,
}

#[derive(Debug, Clone, Copy)]
struct HitTestCoordinateContext {
    content_translation: Vector,
    visible_bounds: Rectangle,
}

impl Default for HitTestCoordinateContext {
    fn default() -> Self {
        Self {
            content_translation: Vector::ZERO,
            visible_bounds: Rectangle::INFINITE,
        }
    }
}

impl HitTestCoordinateContext {
    fn surface_bounds(self, bounds: Rectangle) -> Option<Rectangle> {
        Rectangle {
            x: bounds.x - self.content_translation.x,
            y: bounds.y - self.content_translation.y,
            ..bounds
        }
        .intersection(&self.visible_bounds)
    }

    fn scrollable_content(self, bounds: Rectangle, translation: Vector) -> Self {
        let visible_bounds = self
            .surface_bounds(bounds)
            .unwrap_or_else(Rectangle::default);
        Self {
            content_translation: self.content_translation + translation,
            visible_bounds,
        }
    }
}

struct FileDragHitTestBoundsOperation {
    request: FileDragHitTestBoundsRequest,
    coordinates: HitTestCoordinateContext,
    pending_scrollable_coordinates: Option<HitTestCoordinateContext>,
    tabs: Vec<TabFileDropTargetBounds>,
    entries: Vec<ColumnEntryBounds>,
    breadcrumb_viewports: Vec<(BrowserPaneId, Rectangle)>,
    breadcrumb_directories: Vec<MeasuredBreadcrumbDirectory>,
    directory_targets: Vec<DirectoryFileDragTargetBounds>,
    blocked_directories: Vec<FileDragBlockedDirectoryBounds>,
    sidebar_directories: Vec<SidebarFileDragTargetBounds>,
    empty_sidebar_bookmark_bounds: Option<Rectangle>,
    /// 遍历中遇到的滚动可视区(surface 坐标),拖拽聚合判断用来区分
    /// "超出列表视口"与"超出窗口"。
    scrollable_viewports: Vec<Rectangle>,
}

impl FileDragHitTestBoundsOperation {
    fn new(request: FileDragHitTestBoundsRequest) -> Self {
        Self {
            request,
            coordinates: HitTestCoordinateContext::default(),
            pending_scrollable_coordinates: None,
            tabs: Vec::new(),
            entries: Vec::new(),
            breadcrumb_viewports: Vec::new(),
            breadcrumb_directories: Vec::new(),
            directory_targets: Vec::new(),
            blocked_directories: Vec::new(),
            sidebar_directories: Vec::new(),
            empty_sidebar_bookmark_bounds: None,
            scrollable_viewports: Vec::new(),
        }
    }

    fn breadcrumb_targets(&self) -> Vec<BreadcrumbDropTargetBounds> {
        self.breadcrumb_directories
            .iter()
            .filter_map(|directory| {
                let viewport_bounds =
                    self.breadcrumb_viewports
                        .iter()
                        .rev()
                        .find_map(|(pane_id, bounds)| {
                            (*pane_id == directory.pane_id).then_some(*bounds)
                        })?;
                Some(BreadcrumbDropTargetBounds {
                    pane_id: directory.pane_id,
                    directory: directory.directory.clone(),
                    item_bounds: directory.bounds,
                    viewport_bounds,
                })
            })
            .collect()
    }

    fn measured_bounds(&self) -> FileDragHitTestBounds {
        FileDragHitTestBounds {
            tabs: self.tabs.clone(),
            entries: self.entries.clone(),
            breadcrumbs: self.breadcrumb_targets(),
            directory_targets: self.directory_targets.clone(),
            blocked_directories: self.blocked_directories.clone(),
            sidebar_directories: self.sidebar_directories.clone(),
            empty_sidebar_bookmarks: self.empty_sidebar_bookmark_bounds,
        }
    }
}

impl widget::Operation<Message> for FileDragHitTestBoundsOperation {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation<Message>)) {
        let previous_coordinates = self.coordinates;
        if let Some(coordinates) = self.pending_scrollable_coordinates.take() {
            self.coordinates = coordinates;
        }
        operate(self);
        self.coordinates = previous_coordinates;
    }

    fn scrollable(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn widget::operation::Scrollable,
    ) {
        // Iced 以 content 坐标遍历滚动子树，命中快照必须还原到 surface 坐标。
        self.pending_scrollable_coordinates =
            Some(self.coordinates.scrollable_content(bounds, translation));
        if let Some(viewport) = self.pending_scrollable_coordinates.as_ref().map(|context| context.visible_bounds) {
            if viewport.width > 0.0 && viewport.height > 0.0 {
                self.scrollable_viewports.push(viewport);
            }
        }
    }

    fn custom(&mut self, _id: Option<&widget::Id>, bounds: Rectangle, state: &mut dyn Any) {
        let Some(marker) = state.downcast_ref::<FileDragHitTestMarker>() else {
            return;
        };
        let Some(bounds) = self.coordinates.surface_bounds(bounds) else {
            return;
        };

        match marker {
            FileDragHitTestMarker::Tab { target } => {
                self.tabs.push(TabFileDropTargetBounds {
                    target: target.clone(),
                    bounds,
                });
            }
            FileDragHitTestMarker::ColumnEntry { pane_id, path } => {
                self.entries.push(ColumnEntryBounds {
                    pane_id: *pane_id,
                    path: path.clone(),
                    bounds,
                });
            }
            FileDragHitTestMarker::BreadcrumbViewport { pane_id } => {
                self.breadcrumb_viewports.push((*pane_id, bounds));
            }
            FileDragHitTestMarker::BreadcrumbDirectory { pane_id, directory } => {
                self.breadcrumb_directories
                    .push(MeasuredBreadcrumbDirectory {
                        pane_id: *pane_id,
                        directory: directory.clone(),
                        bounds,
                    });
            }
            FileDragHitTestMarker::DirectoryTarget { pane_id, directory } => {
                self.directory_targets.push(DirectoryFileDragTargetBounds {
                    pane_id: *pane_id,
                    directory: directory.clone(),
                    bounds,
                });
            }
            FileDragHitTestMarker::BlockedDirectoryTarget { pane_id } => {
                self.blocked_directories
                    .push(FileDragBlockedDirectoryBounds {
                        pane_id: *pane_id,
                        bounds,
                    });
            }
            FileDragHitTestMarker::SidebarDirectory {
                directory,
                favorite_index,
            } => {
                self.sidebar_directories.push(SidebarFileDragTargetBounds {
                    directory: directory.clone(),
                    favorite_index: *favorite_index,
                    bounds,
                });
            }
            FileDragHitTestMarker::EmptySidebarBookmarks => {
                self.empty_sidebar_bookmark_bounds = Some(bounds);
            }
        }
    }

    fn finish(&self) -> Outcome<Message> {
        let message = match self.request {
            FileDragHitTestBoundsRequest::SelectionMarquee => {
                Message::ColumnEntryBoundsMeasured(
                    self.entries.clone(),
                    self.scrollable_viewports.clone(),
                )
            }
            FileDragHitTestBoundsRequest::Breadcrumbs(generation) => {
                Message::BreadcrumbDropTargetBoundsMeasured(generation, self.breadcrumb_targets())
            }
            FileDragHitTestBoundsRequest::FileDropLayout(request) => {
                Message::FileDropLayoutMeasured(request, self.measured_bounds())
            }
        };
        Outcome::Some(message)
    }
}

#[cfg(test)]
mod tests {
    use iced::{Point, Size};

    use super::*;

    #[test]
    fn scrollable_translation_converts_content_bounds_to_surface_bounds() {
        let coordinates = HitTestCoordinateContext::default().scrollable_content(
            Rectangle::new(Point::new(185.0, 55.0), Size::new(580.0, 500.0)),
            Vector::new(0.0, 7.5),
        );

        assert_eq!(
            coordinates.surface_bounds(Rectangle::new(
                Point::new(185.0, 76.5),
                Size::new(578.75, 24.0),
            )),
            Some(Rectangle::new(
                Point::new(185.0, 69.0),
                Size::new(578.75, 24.0),
            ))
        );
    }

    #[test]
    fn nested_scrollables_accumulate_translation_and_clip_to_ancestors() {
        let outer = HitTestCoordinateContext::default().scrollable_content(
            Rectangle::new(Point::new(10.0, 20.0), Size::new(100.0, 80.0)),
            Vector::new(4.0, 6.0),
        );
        let inner = outer.scrollable_content(
            Rectangle::new(Point::new(20.0, 30.0), Size::new(60.0, 50.0)),
            Vector::new(3.0, 5.0),
        );

        assert_eq!(
            inner.surface_bounds(Rectangle::new(
                Point::new(15.0, 25.0),
                Size::new(80.0, 80.0),
            )),
            Some(Rectangle::new(
                Point::new(16.0, 24.0),
                Size::new(60.0, 50.0),
            ))
        );
    }
}
