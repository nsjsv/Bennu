use std::path::PathBuf;
use std::time::Instant;

use desktop_linux::{
    FileClipboardSelection, WaylandFileDragSessionId, WaylandFileDropTargetSessionId,
    X11FileDropTargetSessionId,
};
use iced::Point;

use super::{BrowserPaneId, FileDropHitTestBounds, SidebarBookmarkDropSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileDragGestureId(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FileDropSessionIdentity {
    Iced(FileDragGestureId),
    Wayland(WaylandFileDropTargetSessionId),
    X11(X11FileDropTargetSessionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalFileDragSnapshot {
    pub(crate) source_session_id: Option<WaylandFileDragSessionId>,
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) bookmark_source: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDropOrigin {
    Internal(InternalFileDragSnapshot),
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabDropDestination {
    Directory(PathBuf),
    Trash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabFileDropTarget {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) tab_id: usize,
    pub(crate) destination: TabDropDestination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDropTarget {
    Directory(PathBuf),
    Trash,
    SidebarBookmarkSlot(SidebarBookmarkDropSlot),
    Tab(TabFileDropTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TabFileDropTargetBounds {
    pub(crate) target: TabFileDropTarget,
    pub(crate) bounds: iced::Rectangle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileDropLayoutRequest {
    pub(crate) identity: FileDropSessionIdentity,
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) tab_id: usize,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum FileDropLayoutState {
    Pending(FileDropLayoutRequest),
    Ready {
        request: FileDropLayoutRequest,
        hit_test_bounds: FileDropHitTestBounds,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TabDropHover {
    pub(crate) identity: FileDropSessionIdentity,
    pub(crate) target: TabFileDropTarget,
    pub(crate) layout_generation: u64,
    pub(crate) hover_generation: u64,
    pub(crate) started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDropSessionPhase {
    Hovering,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrozenFileDropTarget {
    Target(FileDropTarget),
    Rejected,
}

#[derive(Debug, Clone)]
pub(crate) struct FileDropSessionState {
    pub(crate) identity: FileDropSessionIdentity,
    pub(crate) origin: FileDropOrigin,
    pub(crate) phase: FileDropSessionPhase,
    pub(crate) layout: FileDropLayoutState,
    pub(crate) position: Option<Point>,
    pub(crate) hovered_target: Option<FileDropTarget>,
    pub(crate) tab_hover: Option<TabDropHover>,
    pub(crate) hover_generation: u64,
    pub(crate) pending_payload: Option<FileClipboardSelection>,
    pub(crate) frozen_drop_target: Option<FrozenFileDropTarget>,
    /// 滚动驱动的在途布局刷新:快照换代期间旧 Ready 快照仍然可用,
    /// 这里只登记最新在途代,accept 时按代校验并清除。
    pub(crate) scroll_refresh_in_flight: Option<FileDropLayoutRequest>,
}
