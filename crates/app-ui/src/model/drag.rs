use std::path::PathBuf;
use std::time::Instant;

use desktop_linux::WaylandFileDragSessionId;
use iced::{Point, Rectangle};

use super::{SplitRegion, TransferConflictMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDragMode {
    Reorder,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabSplitTarget {
    pub(crate) region: SplitRegion,
}

#[derive(Debug, Clone)]
pub(crate) struct TabDragState {
    pub(crate) source_pane_id: super::BrowserPaneId,
    pub(crate) tab_id: usize,
    pub(crate) phase: FileDragPhase,
    pub(crate) mode: TabDragMode,
    pub(crate) split_target: Option<TabSplitTarget>,
}

impl TabDragState {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, FileDragPhase::Dragging)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneDropTarget {
    Split(SplitRegion),
    Merge(super::BrowserPaneId),
}

#[derive(Debug, Clone)]
pub(crate) struct PaneDragState {
    pub(crate) source_pane_id: super::BrowserPaneId,
    pub(crate) phase: FileDragPhase,
    pub(crate) target: Option<PaneDropTarget>,
}

impl PaneDragState {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, FileDragPhase::Dragging)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneDragPointerPress {
    pub(crate) source_pane_id: super::BrowserPaneId,
    pub(crate) origin: Point,
}

/// 拖拽预览中的一个条目:原点相对按下点(提起瞬间的光标位置)的偏移,
/// 拖动期间保持不变——预览组是被"提起"的瞬时快照,不随源视图滚动重排。
#[derive(Debug, Clone)]
pub(crate) struct FileDragPreviewEntry {
    pub(crate) path: PathBuf,
    pub(crate) offset: iced::Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDragStationaryAction {
    SelectionOnly,
    ActivateColumnEntry,
}

/// 拖放意图:动作胶囊文案与落地传输共用的单一判定结果。创建链接没有
/// 冲突策略(共享命名规则保证唯一),不并入 TransferConflictMode。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDragDropIntent {
    Move,
    Copy,
    CreateLink,
}

impl FileDragDropIntent {
    /// 传输冲突模式;CreateLink 在入口就分流到链接入队,不走此转换。
    pub(crate) fn conflict_mode(self) -> TransferConflictMode {
        match self {
            Self::Copy => TransferConflictMode::Copy,
            Self::Move | Self::CreateLink => TransferConflictMode::Move,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FileDragState {
    pub(crate) gesture_id: super::FileDragGestureId,
    pub(crate) source_pane_id: super::BrowserPaneId,
    pub(crate) source_tab_id: usize,
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) pressed_path: PathBuf,
    pub(crate) bookmark_source: Option<PathBuf>,
    pub(crate) stationary_action: FileDragStationaryAction,
    pub(crate) phase: FileDragPhase,
    pub(crate) native_dnd: FileDragNativeDndState,
    pub(crate) column_directories_snapshot: Vec<PathBuf>,
    /// 按下瞬间的光标位置(窗口坐标),预览偏移以此为基准。
    pub(crate) press_origin: Point,
    /// 提起时的条目偏移快照;空表示尚未由 bounds 测量填充。
    pub(crate) preview_entries: Vec<FileDragPreviewEntry>,
}

impl FileDragState {
    pub(crate) fn source_column_directories(
        &self,
        pane_id: super::BrowserPaneId,
        tab_id: usize,
    ) -> Option<&[PathBuf]> {
        (self.source_pane_id == pane_id
            && self.source_tab_id == tab_id
            && !self.column_directories_snapshot.is_empty())
        .then_some(self.column_directories_snapshot.as_slice())
    }

    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, FileDragPhase::Dragging)
    }

    pub(crate) fn can_start_native_dnd(&self) -> bool {
        self.native_dnd == FileDragNativeDndState::NotRequested && self.is_dragging()
    }

    pub(crate) fn displays_iced_drag_preview(&self) -> bool {
        self.is_dragging()
            && matches!(
                self.native_dnd,
                FileDragNativeDndState::NotRequested | FileDragNativeDndState::Requested(_)
            )
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileDragHitTestBounds {
    pub(crate) tabs: Vec<super::TabFileDropTargetBounds>,
    pub(crate) entries: Vec<super::ColumnEntryBounds>,
    pub(crate) breadcrumbs: Vec<BreadcrumbDropTargetBounds>,
    pub(crate) directory_targets: Vec<DirectoryFileDragTargetBounds>,
    pub(crate) blocked_directories: Vec<FileDragBlockedDirectoryBounds>,
    pub(crate) sidebar_directories: Vec<SidebarFileDragTargetBounds>,
    pub(crate) empty_sidebar_bookmarks: Option<Rectangle>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectoryFileDragTargetBounds {
    pub(crate) pane_id: super::BrowserPaneId,
    pub(crate) directory: PathBuf,
    pub(crate) bounds: Rectangle,
}

#[derive(Debug, Clone)]
pub(crate) struct FileDragBlockedDirectoryBounds {
    pub(crate) pane_id: super::BrowserPaneId,
    pub(crate) bounds: Rectangle,
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarFileDragTargetBounds {
    pub(crate) directory: PathBuf,
    pub(crate) favorite_index: Option<usize>,
    pub(crate) bounds: Rectangle,
}

#[derive(Debug, Clone)]
pub(crate) struct FileDropEntryTargetBounds {
    pub(crate) directory: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) bounds: Rectangle,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileDropHitTestBounds {
    pub(crate) tabs: Vec<super::TabFileDropTargetBounds>,
    pub(crate) entries: Vec<FileDropEntryTargetBounds>,
    pub(crate) breadcrumbs: Vec<BreadcrumbDropTargetBounds>,
    pub(crate) directory_targets: Vec<DirectoryFileDragTargetBounds>,
    pub(crate) blocked_directories: Vec<FileDragBlockedDirectoryBounds>,
    pub(crate) sidebar_directories: Vec<SidebarFileDragTargetBounds>,
    pub(crate) empty_sidebar_bookmarks: Option<Rectangle>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BreadcrumbDropTargetBounds {
    pub(crate) pane_id: super::BrowserPaneId,
    pub(crate) directory: PathBuf,
    pub(crate) item_bounds: Rectangle,
    pub(crate) viewport_bounds: Rectangle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDragNativeDndState {
    NotRequested,
    Requested(WaylandFileDragSessionId),
    Started(WaylandFileDragSessionId),
    Dropped(WaylandFileDragSessionId),
}

impl FileDragNativeDndState {
    pub(crate) fn session_id(self) -> Option<WaylandFileDragSessionId> {
        match self {
            Self::NotRequested => None,
            Self::Requested(session_id) | Self::Started(session_id) | Self::Dropped(session_id) => {
                Some(session_id)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarBookmarkDropSlot {
    Insert { index: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarBookmarkDragState {
    pub(crate) path: PathBuf,
    pub(crate) origin: Point,
    pub(crate) source_index: usize,
    pub(crate) phase: FileDragPhase,
    pub(crate) order_changed: bool,
}

impl SidebarBookmarkDragState {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, FileDragPhase::Dragging)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FileDragPhase {
    WaitingForMovement { origin: Point },
    Dragging,
}

#[derive(Debug, Clone)]
pub(crate) struct LastActivationClick {
    pub(crate) path: PathBuf,
    pub(crate) at: Instant,
}
