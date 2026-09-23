use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationEvent, DesktopClipboardContent, FileClipboardOperation,
    OpenWithApplicationList, StorageDeviceId, StorageDeviceSnapshot, TerminalEmulator,
    WaylandDndFileDrop, WaylandDndWindowHandle, WaylandFileDragIcon, WaylandFileDragSourceEvent,
    WaylandFileDropTargetEvent, WaylandFileDropTargetSessionId,
};
use file_core::FileOperationVerification;
use file_core::{
    DirectoryDiscovery, DirectoryDiscoveryBatch, DirectoryEntry, DirectoryMetadataResolution,
    TrashRestoreEntry, TrashScan,
};
use file_operation_store::TaskQueueStore;
use file_search::{
    SearchHit, SearchPathConfigurationStatus, SearchServiceStatus, SearchTextScope,
    VersionedSearchPathPreferences,
};
use iced::keyboard;
use iced::{event, mouse, window, Point, Theme};

use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePreview};
use crate::app::archive_creation::ArchiveCreationMessage;
use crate::app::archive_extraction::ArchiveExtractionMessage;
use crate::app::checksum::ChecksumMessage;
use crate::app::convert::ConvertMessage;
use crate::audio_preview::AudioPreviewRuntime;
use crate::config::{RenderingGpuPreference, UiLanguage, UiLanguageSetting, UserConfig};
use crate::document_preview::DocumentPreviewMessage;
use crate::matugen_theme::{ColorSchemeFamily, ColorSchemePreset, ThemeMode};
use crate::network_connections::{
    NetworkConnectionMessage, SidebarNetworkConnectionContextMenuState,
};
use crate::operation_history::FileOperationCompletion;
use crate::operation_queue::QueuedTransfer;
use crate::shortcuts::{ShortcutAction, ShortcutBindingId};
use crate::sidebar_devices::{
    SidebarDeviceAction, SidebarDeviceActionRequest, SidebarDeviceContextMenuState,
};
use crate::startup_rendering::StartupRenderingEnvironmentStatus;
use crate::thumbnail_cache::ThumbnailLoadOutcome;

pub(crate) use crate::text_preview::{MarkdownPreviewMode, TextPreviewChunk, TextPreviewDocument};
pub(crate) use file_core::{TransferConflictItem, TransferConflictMetadata};

mod address_bar;
pub(crate) use address_bar::{
    breadcrumb_segments, displayed_address_directory, AddressEditingSession,
    AddressEditingSessionId, AddressSuggestionRequest, BreadcrumbSegment, BreadcrumbSegmentKind,
    PaneAddressBarTransition, PaneAddressEditingSession,
};
mod entry_naming;
pub(crate) use entry_naming::{
    entry_exists, suffixed_name_candidates, unique_duplicated_directory_name,
    unique_duplicated_file_name, unique_gathered_folder_directory, unique_symlink_directory_name,
    unique_symlink_file_name, GATHERED_FOLDER_BASE_NAME,
};
mod browser_panes;
pub(crate) use browser_panes::{
    empty_directory_entry_snapshot, retain_direct_entry_selection, BrowserPane, BrowserPaneId,
    BrowserPaneLayout, BrowserTab, BrowserViewMode, ColumnBrowserViewport,
    DirectoryCollectionPhase, DirectoryEntrySnapshot, DirectoryExpansionLoadContext,
    DirectoryLoadFailure, DirectoryLoadRequest, DirectoryLoadingPlaceholder,
    DirectoryLoadingPlaceholderEntry, DirectoryMetadataLoadContext, DirectoryMetadataLoadFailure,
    DirectoryMetadataLoadRequest, DirectoryOrderPhase, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, IconGridExpansionSessionId,
    IconGridViewport, ListExpansionFollowSessionId, SplitAxis, SplitRegion,
};
mod split_layout;
pub(crate) use split_layout::{SPLIT_DIVIDER_WIDTH, SPLIT_PORTION_TOTAL};
mod trash;
pub(crate) use trash::{TrashRefreshCompletionDecision, TrashRefreshState};
mod selection;
pub(crate) use selection::{
    ColumnEntryBounds, SelectionMarquee, SelectionMarqueePhase, SelectionMarqueeScrollAnchor,
    SelectionMarqueeSource,
};
mod icon_grid_expansion;
#[cfg(test)]
mod icon_grid_expansion_tests;
pub(crate) use icon_grid_expansion::{
    IconGridAnchorReconciliation, IconGridChildSwitch, IconGridExpandedDirectory,
    IconGridExpansionAnchor, IconGridExpansionContext, IconGridExpansionFollowAdvance,
    IconGridExpansionMigration, IconGridExpansionState, IconGridRemovedPathReconciliation,
};
mod list_view_preferences;
pub(crate) use list_view_preferences::{
    list_column_kind_config_value, list_column_kind_from_config_value, ListColumnConfig,
    ListColumnKind, ListSortPreference, ListViewPreferences,
};
mod file_grouping;
pub(crate) use file_grouping::{
    active_file_group_index, date_group_key, dynamic_size_buckets, file_group_rail_visible,
    file_group_scroll_target_offset, file_grouping_entry_visible, kind_category_group_key,
    name_initial_group_key, partition_files_into_groups, size_group_bucket_index, FileGroupKey,
    FileGroupRailEntry, FileGroupSection, FileGroupingContext, FileGroupingMode,
};
mod list_directory_summary;
pub(crate) use list_directory_summary::{
    ListDirectorySizeDisplayMode, ListDirectorySummary, ListDirectorySummaryCache,
    ListDirectorySummaryLoadRequest,
};
mod file_entry_content_modifier;
pub(crate) use file_entry_content_modifier::FileEntryContentModifier;
mod batch_rename;
pub(crate) use batch_rename::{
    same_parent, BatchRenameCaseRule, BatchRenameExtensionMode, BatchRenameInsertMode,
    BatchRenameInsertRule, BatchRenameMessage, BatchRenamePreviewRow, BatchRenamePreviewStatus,
    BatchRenameRandomMode, BatchRenameRemoveClass, BatchRenameRemoveMode, BatchRenameRemoveRule,
    BatchRenameReplaceRule, BatchRenameReplaceScope, BatchRenameRule, BatchRenameRuleKind,
    BatchRenameRuleParams, BatchRenameSequenceRule, BatchRenameSliceMode, BatchRenameSliceRule,
    BatchRenameSortMode, BatchRenameSource, BatchRenameSourceNameError, BatchRenameState,
    BatchRenameTemplateRule, BatchRenameTemplateToken,
};
mod properties;
pub(crate) use properties::{
    FilePropertiesAggregateSnapshot, FilePropertiesCategory, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesIdentity, FilePropertiesLoadState,
    FilePropertiesMessage, FilePropertiesPermissionAccess, FilePropertiesPermissionBaseline,
    FilePropertiesPermissionClass, FilePropertiesPermissionUpdate,
    FilePropertiesPermissionWriteOutcome, FilePropertiesPermissions, FilePropertiesPresentation,
    FilePropertiesRequest, FilePropertiesSnapshot, FilePropertiesState, FilePropertiesTargetSet,
    PermissionBatchOutcome, PermissionBatchPathFailure,
};
// 纯搬移：SQLite 预览模型类型已下沉 bennu-preview，此处 re-export 维持
// crate::model::SqlitePreviewMessage 等既有调用路径不变。
pub(crate) use bennu_preview::sqlite_preview::{SqlitePreviewMessage, SqlitePreviewTab};
// 纯搬移：预览聚合模型已下沉 bennu-preview，此处 re-export 维持
// crate::model::PreviewState 等既有调用路径不变。
pub(crate) use bennu_preview::preview::{
    AudioPreviewPlayback, ImagePreviewContent, PreviewContent, PreviewSize, PreviewState,
    PreviewWindowProfile, RemotePreviewCacheFinished, RemotePreviewCacheMessage,
    RightPreviewPanelInfoSnapshot, VideoPreviewFrame, VideoPreviewPlayback,
};
// 纯搬移：视口模型已下沉 bennu-preview，此处 re-export 维持
// crate::model::ImagePreviewViewport 既有调用路径不变。
pub(crate) use bennu_preview::image_preview_viewport::{
    ImagePreviewViewport, PreviewImageViewportMessage,
};
// 预览子系统自有消息类型（命令层/状态机的输出契约）经此 re-export，
// 宿主经 Message::Preview 单一包裹变体路由。
pub(crate) use bennu_preview::preview_message::PreviewMessage;
mod settings;
pub(crate) use settings::{SettingsCategory, SettingsSubpage};
mod context_menu_items;
mod context_menu_layout;
#[cfg(test)]
mod context_menu_layout_tests;
pub(crate) use context_menu_items::{FileAreaMenuItem, SearchResultMenuItem, TrashMenuItem};
pub(crate) use context_menu_layout::{
    ContextMenuLayoutConfigValues, ContextMenuPreferences, ContextMenuSettingsDragState,
    ContextMenuSettingsPage, ContextMenuSettingsPageStep, ContextMenuSettingsRow,
    FileEntryMenuEntry, CONTEXT_MENU_SETTINGS_PAGES, CONTEXT_MENU_SETTINGS_ROW_PITCH,
};
mod window_controls;
pub(crate) use window_controls::{
    WindowChromeLayout, WindowControlKind, WindowControlMoveDirection, WindowControlPlacement,
    WindowControlSide, WindowControlVisibility, WindowControlsConfig, WindowFrameState,
    MAIN_TOOLBAR_ROW_HEIGHT, WINDOW_TITLE_BAR_HEIGHT, WINDOW_TOP_BAR_HEIGHT,
};
mod application_logs;
pub(crate) use application_logs::{
    bounded_application_log_message, sanitized_application_log_detail, ApplicationLogEntry,
    ApplicationLogLevel, ApplicationLogRequest, ApplicationLogSource, ApplicationLogViewState,
    APPLICATION_LOG_ENTRY_LIMIT, APP_JOURNAL_IDENTIFIER, SEARCH_JOURNAL_IDENTIFIER,
};
pub(crate) mod search;
pub(crate) use search::{
    DirectoryFallbackOutcome, IndexedSearchOutcome, IndexedSearchRequest, LastSearchScope,
    SearchDateField, SearchDatePreset, SearchDirectoryScope, SearchEntryTypePreset, SearchHistory,
    SearchHistoryInteraction, SearchInputFocus, SearchInputFocusCheckOrigin,
    SearchInputFocusCheckRequest, SearchInputStabilizationRequest, SearchInputStabilizationSubject,
    SearchKeyboardSelection, SearchResultCompletion, SearchSelectionGesture, SearchSelectionStep,
    SearchSizePreset, SearchWorkspaceSessionId, SearchWorkspaceState, SEARCH_RESULT_TOTAL_LIMIT,
};
mod search_service;
pub(crate) use search_service::{
    SearchEndpointState, SearchPathConfigureRequest, SearchPathEntryKind, SearchServiceDiagnostic,
    SearchServiceDiagnosticKind, SearchServiceIncident, SearchServiceIncidentState,
    SearchServiceRecoveryAction, SearchServiceRecoveryState, SearchServiceState,
    SearchServiceStatusRequest,
};
mod session;
pub(crate) use session::{
    pane_session_from_live, snapshot_from_stored, snapshot_to_stored, BrowserPaneSession,
    BrowserSessionSnapshot, BrowserTabSession,
};
mod file_drop;
pub(crate) use file_drop::{
    FileDragGestureId, FileDropLayoutRequest, FileDropLayoutState, FileDropOrigin,
    FileDropSessionIdentity, FileDropSessionPhase, FileDropSessionState, FileDropTarget,
    FrozenFileDropTarget, InternalFileDragSnapshot, TabDropDestination, TabDropHover,
    TabFileDropTarget, TabFileDropTargetBounds,
};
mod x11_dnd;
pub(crate) use x11_dnd::X11DndMessage;
mod drag;
pub(crate) use drag::{
    BreadcrumbDropTargetBounds, DirectoryFileDragTargetBounds, FileDragBlockedDirectoryBounds,
    FileDragDropIntent, FileDragHitTestBounds, FileDragNativeDndState, FileDragPhase,
    FileDragPreviewEntry, FileDragSpringHover, FileDragSpringSource, FileDragState,
    FileDragStationaryAction, FileDropEntryTargetBounds, FileDropHitTestBounds,
    LastActivationClick, PaneDragPointerPress, PaneDragState, PaneDropTarget,
    SidebarBookmarkDragState, SidebarBookmarkDropSlot, SidebarFileDragTargetBounds, TabDragMode,
    TabDragState, TabSplitTarget,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ScrollbarRegion {
    Sidebar,
    AddressBar(BrowserPaneId),
    PaneList(BrowserPaneId),
    PaneIcons(BrowserPaneId),
    ColumnBrowser(BrowserPaneId),
    Column {
        pane_id: BrowserPaneId,
        directory: PathBuf,
    },
    Settings,
    Properties,
    OpenWithApplications,
    OperationQueue,
    BatchRenamePreview,
    SearchHistory,
    SearchResults,
    PreviewDirectory,
    PreviewArchive,
    PreviewDocument,
    TextPreview,
    MarkdownPreview,
    PreviewSqliteTables,
    PreviewSqliteData,
}
// 视口快照模型随滚动条视觉层迁至共享 crate `bennu-theme::scrollbar`；
// 重导出保持 `crate::model::ScrollbarViewport` 路径不变。
// SCROLLBAR_MIN_THUMB_LENGTH 亦随之下沉，但 crate 内已无直接消费者
// （thumb 几何在共享层计算），不再经 model 转发以免死重导出。
pub use bennu_theme::scrollbar::ScrollbarViewport;

// 滚动条可见性模型迁至共享 crate `bennu-theme::styles`；重导出保持调用点不变。
pub(crate) use bennu_theme::styles::{ScrollbarVisibility, SCROLLBAR_HOVER_WIDTH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupDirectoryValidationRequest {
    pub(crate) generation: u64,
    pub(crate) input: String,
    pub(crate) directory: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupDirectoryAvailability {
    Usable,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupSessionSource {
    Home,
    CustomDirectory(PathBuf),
    PreviousSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupSessionPlanRequest {
    pub(crate) home: PathBuf,
    pub(crate) source: StartupSessionSource,
}

#[derive(Debug, Clone)]
pub(crate) enum StartupSessionPlan {
    Directory {
        directory: PathBuf,
        error: Option<String>,
    },
    Session(BrowserSessionSnapshot),
}

#[derive(Debug, Clone)]
pub(crate) struct ClassifiedStartupSession {
    pub(crate) request: StartupSessionPlanRequest,
    pub(crate) plan: StartupSessionPlan,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedOperationStore {
    pub(crate) task_queue_store: TaskQueueStore,
    pub(crate) column_width_overrides: HashMap<usize, f32>,
    /// 上次保存栏宽时的参考内容宽度;None 表示旧数据未记录过(按当前窗口兜底)。
    pub(crate) column_width_reference_content_width: Option<f32>,
    pub(crate) classified_startup_session: Option<ClassifiedStartupSession>,
}

#[derive(Debug, Clone)]
// 权威目录发现 + 预构建显示条目：display 深拷贝在 discovery 命令任务内完成，
// update 线程只做整体替换——目录提交路径不在 UI 线程逐条深拷贝。
pub(crate) struct PrebuiltDirectoryDiscovery {
    pub(crate) discovery: DirectoryDiscovery,
    pub(crate) display_entries: std::sync::Arc<Vec<DirectoryEntry>>,
}

impl PrebuiltDirectoryDiscovery {
    pub(crate) fn build(discovery: DirectoryDiscovery) -> Self {
        let display_entries = std::sync::Arc::new(display_entries_in_discovery_order(&discovery));
        Self {
            discovery,
            display_entries,
        }
    }
}

pub(crate) fn display_entries_in_discovery_order(
    discovery: &DirectoryDiscovery,
) -> Vec<DirectoryEntry> {
    discovery
        .order
        .iter()
        .map(|index| {
            discovery.entries[*index]
                .display_entry()
                .with_discovery_index(*index)
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    StartupEnvironmentLoaded(Box<StartupEnvironment>),
    /// 预览子系统包裹变体：下沉后的命令层/状态机输出 PreviewMessage，
    /// 宿主经它路由（迁移期机械映射回既有散装变体，视图组迁移后收拢）。
    Preview(PreviewMessage),
    TerminalPanel(crate::terminal_panel::TerminalPanelMessage),
    SidebarLocationsLoaded(Vec<SidebarLocation>),
    SidebarDevicesLoaded(StorageDeviceSnapshot),
    SidebarDevicesRefreshRequested,
    SidebarDeviceHovered(StorageDeviceId),
    SidebarDeviceHoverCleared(StorageDeviceId),
    SidebarDevicePressed(StorageDeviceId),
    SidebarDeviceMiddlePressed(BrowserPaneId, StorageDeviceId),
    SidebarDeviceRightClicked(StorageDeviceId),
    SidebarDeviceActionSelected(StorageDeviceId, SidebarDeviceAction),
    SidebarDeviceActionFinished(
        SidebarDeviceActionRequest,
        SidebarDeviceAction,
        Result<Option<PathBuf>, String>,
    ),
    NetworkConnection(NetworkConnectionMessage),
    OperationStoreLoaded(Result<LoadedOperationStore, String>),
    DirectoryDiscoveryBatch(DirectoryLoadRequest, DirectoryDiscoveryBatch),
    DirectoryEntriesReady(
        DirectoryLoadRequest,
        Result<PrebuiltDirectoryDiscovery, DirectoryLoadFailure>,
    ),
    DirectoryMetadataResolved(
        DirectoryMetadataLoadRequest,
        Result<DirectoryMetadataResolution, DirectoryMetadataLoadFailure>,
    ),
    TrashLoaded(u64, Result<TrashScan, String>),
    TrashWarningsToggled,
    OpenFileFinished(PathBuf, Result<(), String>),
    OpenWithRequested(PathBuf),
    OpenWithApplicationsLoaded(PathBuf, Result<OpenWithApplicationList, String>),
    OpenWithDefaultApplicationToggled(bool),
    OpenWithApplicationSelected(String),
    OpenWithApplicationFinished(Result<(), String>),
    OpenTerminalFinished(Result<(), String>),
    PreviewLoaded(PathBuf, Result<PreviewContent, String>),
    DocumentPreview(DocumentPreviewMessage),
    SqlitePreview(SqlitePreviewMessage),
    RemotePreviewCache(RemotePreviewCacheMessage),
    AnimatedImagePreviewLoaded(PathBuf, u64, Result<AnimatedImagePreview, String>),
    OriginalImagePreviewLoaded(
        PathBuf,
        u64,
        Result<crate::original_image_preview::OriginalImagePreview, String>,
    ),
    RetryImagePreview(PathBuf),
    FileProperties(FilePropertiesMessage),
    PreviewDirectoryChildrenLoaded(PathBuf, Result<Vec<DirectoryEntry>, String>),
    TextPreviewContentScrolled {
        lines: i32,
        viewport_height: f32,
    },
    TextPreviewViewerScrolled {
        lines: i32,
        offset_y: f32,
        viewport_height: f32,
    },
    TextPreviewViewportSynced {
        offset_y: f32,
        viewport_height: f32,
    },
    TextPreviewContentHeightChanged(f32),
    TextPreviewChunkLoaded {
        path: PathBuf,
        generation: u64,
        start_offset: u64,
        outcome: Result<TextPreviewChunk, String>,
    },
    MarkdownPreviewScrolled {
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    },
    MarkdownPreviewModeSelected(MarkdownPreviewMode),
    ImagePreviewDimensionsLoaded(PathBuf, u64, Result<(u32, u32), String>),
    PreviewImageViewport(PreviewImageViewportMessage),
    AnimatedImageFrameLoaded(AnimatedImageFrame),
    AnimatedImagePreviewFinished(PathBuf, u64),
    AnimatedImagePreviewFailed(PathBuf, u64, String),
    AnimatedImageSeekRequested(f32),
    AnimatedImageSeekCommitted,
    AudioPreviewPlaybackToggled,
    AudioPreviewStarted(PathBuf, Result<AudioPreviewRuntime, String>),
    AudioPreviewSeekRequested(f32),
    AudioPreviewVolumeChanged(f32),
    AudioPreviewTick,
    VideoPreviewPlaybackToggled,
    VideoPreviewAudioStarted(PathBuf, u64, Result<AudioPreviewRuntime, String>),
    VideoPreviewMetadataLoaded(PathBuf, Result<Option<Duration>, String>),
    VideoPreviewSeekRequested(f32),
    VideoPreviewSeekCommitted,
    VideoPreviewVolumeChanged(f32),
    VideoPreviewTick,
    VideoPreviewFrameLoaded(VideoPreviewFrame),
    VideoPreviewSeekFrameFailed(PathBuf, u64, Duration, String),
    VideoPreviewFinished(PathBuf, u64),
    VideoPreviewFailed(PathBuf, u64, String),
    FileOperationProgressed(
        u64,
        crate::operation_progress::FileOperationProgressUpdate,
        Vec<crate::operation_progress::TransferEntrySnapshot>,
    ),
    FileOperationDirectMovesCommitted {
        task_id: u64,
        commits: Vec<crate::commands::DurableDirectMoveCommit>,
    },
    FileOperationMovesRenamed {
        task_id: u64,
        moves: Vec<crate::operation_history::CompletedTransfer>,
    },
    /// 第二个参数是驱动者 generation:换代重启后,被取消的旧驱动者
    /// 迟到的终态消息凭它被拒,不会把已重启的任务提前终结。
    FileOperationFinished(u64, u64, FileOperationCompletion),
    FileOperationPersistenceFinished(crate::operation_queue::FileOperationPersistenceOutcome),
    OperationProgressAnimationTick,
    OperationSupervisionTick,
    FileDragSpringOpenTick,
    BreadcrumbDropTargetHovered(BrowserPaneId, PathBuf),
    BreadcrumbDropTargetHoverCleared(BrowserPaneId, PathBuf),
    DesktopNotificationPublished(Result<(), String>),
    FileOperationIndicatorPressed,
    FileOperationPauseToggled(u64),
    FileOperationCancelRequested(u64),
    FileOperationDetailsCopyRequested(u64),
    FileOperationClearRequested(u64),
    FileOperationClearFinishedRequested,
    PreviewTreeDirectoryToggled(usize),
    PreviewTreeAnimationTick,
    ThumbnailRefreshRequested(BrowserPaneId, PathBuf),
    ThumbnailBatchLoaded(Vec<ThumbnailLoadOutcome>),
    BrowserViewModeSelected(BrowserPaneId, BrowserViewMode),
    IconGridDirectoryToggled(BrowserPaneId, IconGridExpansionAnchor),
    IconGridPanelPressed(BrowserPaneId, PathBuf),
    ListDirectoryToggled(BrowserPaneId, PathBuf),
    FlatEntryClicked(BrowserPaneId, PathBuf),
    ListHeaderRightClicked(BrowserPaneId),
    ListColumnVisibilityToggled(ListColumnKind),
    FileGroupingModeSelected(FileGroupingMode),
    /// 分组索引栏的点击/扫动落点:按下与按住换档都发同一条消息,目标
    /// 组序在更新层换算成滚动偏移并主动回写视口状态(scroll_to 不回发
    /// on_scroll)。扫动的按住状态由索引栏组件局部持有,不进全局状态。
    FileGroupingRailTargetSelected {
        pane: BrowserPaneId,
        group_index: usize,
    },
    ListColumnResizeStarted(BrowserPaneId, ListColumnKind),
    ListColumnReorderStarted(BrowserPaneId, ListColumnKind),
    ListHeaderColumnEntered(BrowserPaneId, ListColumnKind),
    ListHeaderColumnExited(BrowserPaneId, ListColumnKind),
    ListDirectorySummaryLoaded(
        ListDirectorySummaryLoadRequest,
        Result<ListDirectorySummary, String>,
    ),
    ColumnEntryClicked(BrowserPaneId, PathBuf),
    ColumnBlankClicked(BrowserPaneId, PathBuf),
    ColumnBlankRightClicked(BrowserPaneId, PathBuf),
    ColumnPlaceholderPressed(BrowserPaneId),
    EntryReleased(BrowserPaneId, PathBuf),
    EntryRightClicked(BrowserPaneId, PathBuf),
    EntryHovered(BrowserPaneId, PathBuf),
    EntryHoverCleared(BrowserPaneId, PathBuf),
    /// 分组索引栏进入/离开:栏是事件屏障,光标在栏内时下层行收不到
    /// exit、栏外滚动重算也不得命中,两个方向都要显式入账。
    FileGroupingRailCursorEntered(BrowserPaneId),
    FileGroupingRailCursorExited(BrowserPaneId),
    DropTargetHovered(BrowserPaneId, PathBuf),
    DropTargetHoverCleared(BrowserPaneId, PathBuf),
    DropTargetReleased(BrowserPaneId, PathBuf),
    BlankAreaPressed(BrowserPaneId),
    BlankAreaRightClicked(BrowserPaneId, PathBuf),
    SidebarHovered(PathBuf),
    SidebarHoverCleared(PathBuf),
    SidebarPointerMoved(Point),
    SidebarPointerExited,
    SidebarBookmarkPressed(PathBuf),
    SidebarBookmarkRightClicked(PathBuf),
    SidebarBookmarkEntered(PathBuf),
    SidebarBookmarkReleased,
    SidebarBookmarkDeleteRequested(PathBuf),
    SidebarResizeStarted,
    RightPreviewPanelResizeStarted,
    RightPreviewPanelRatioResizeStarted,
    RightPreviewPanelInfoLoaded {
        path: PathBuf,
        snapshot: Result<Box<crate::model::RightPreviewPanelInfoSnapshot>, String>,
    },
    SqliteTablesResizeStarted,
    SplitResizeStarted,
    ToggleRightPreviewPanel,
    CursorMoved {
        window: window::Id,
        position: Point,
    },
    CursorLeft {
        window: window::Id,
    },
    ColumnBrowserCursorEntered(BrowserPaneId),
    ColumnBrowserCursorExited(BrowserPaneId),
    ColumnEntryBoundsMeasured(Vec<ColumnEntryBounds>, Vec<iced::Rectangle>),
    BreadcrumbDropTargetBoundsMeasured(u64, Vec<BreadcrumbDropTargetBounds>),
    FileDropLayoutMeasured(FileDropLayoutRequest, FileDragHitTestBounds),
    PaneCursorEntered(BrowserPaneId),
    PaneCursorExited(BrowserPaneId),
    KeyboardModifiersChanged(keyboard::Modifiers),
    KeyboardKeyPressed {
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        status: event::Status,
    },
    FileContentShortcutRouted(ShortcutAction),
    ShortcutCaptureStarted(ShortcutBindingId),
    ShortcutCaptureCanceled,
    ShortcutBindingReset(ShortcutBindingId),
    DragSelectionFinished,
    GlobalErrorNotificationElapsed(u64),
    GlobalErrorNotificationPointerEntered(u64),
    GlobalErrorNotificationPointerExited(u64),
    GlobalErrorNotificationDismissed(u64),
    DismissFloating,
    ArchiveCreation(ArchiveCreationMessage),
    Convert(ConvertMessage),
    Checksum(ChecksumMessage),
    ArchiveExtraction(ArchiveExtractionMessage),
    BatchRename(BatchRenameMessage),
    /// 本地文件传输:二维码下载会话、LocalSend 直推、接收确认与设置。
    Transfer(crate::app::transfer::TransferMessage),
    FileContextMenuExpansionChanged(FileContextMenuExpansion),
    DeleteSelectedPermanently,
    ContextMenuPreviewExpansionChanged(Option<FileAreaMenuItem>),
    DestructiveActionConfirmed,
    DestructiveActionCanceled,
    AuxiliaryWindowCloseRequested(window::Id),
    PreviewWindowPinToggled,
    AuxiliaryWindowResized(window::Id, f32, f32),
    X11Dnd(X11DndMessage),
    WindowMinimizeRequested(window::Id),
    WindowMaximizeToggled(window::Id),
    WindowMaximizedObserved(window::Id, WindowFrameState),
    WindowDragRequested(window::Id),
    WindowResizeRequested(window::Id, window::Direction),
    WindowFocused(window::Id),
    WindowUnfocused(window::Id),
    WindowPointerPressed {
        window: window::Id,
        button: mouse::Button,
        status: event::Status,
    },
    WindowPointerReleased {
        window: window::Id,
        status: event::Status,
    },
    AddressEditingRequested(BrowserPaneId),
    BreadcrumbSegmentPressed(BrowserPaneId, PathBuf),
    AddressDraftChanged(BrowserPaneId, String),
    AddressEditingSubmitted(BrowserPaneId),
    AddressSuggestionSelected(BrowserPaneId, PathBuf),
    AddressSuggestionInputStabilized(AddressSuggestionRequest),
    AddressSuggestionsLoaded(AddressSuggestionRequest, Vec<PathBuf>),
    AddressBarScrolled(BrowserPaneId),
    SearchInputChanged(String),
    SearchInputStabilized(SearchInputStabilizationRequest),
    SearchSubmitted,
    SearchHistoryKeywordSelected(String),
    SearchHistoryKeywordRemoved(String),
    SearchHistoryCleared,
    SearchInputFocusChecked(SearchInputFocusCheckRequest, SearchInputFocus),
    SearchHistoryPopupPointerEntered,
    SearchHistoryPopupPointerExited,
    SearchEntryTypesMenuOpened,
    SearchEntryTypeToggled(SearchEntryTypePreset),
    SearchDirectoryScopeSelected(SearchDirectoryScope),
    SearchTextScopeSelected(SearchTextScope),
    SearchRegexToggled,
    SearchCustomExtensionsToggled,
    SearchCustomExtensionsChanged(String),
    SearchDateFieldSelected(SearchDateField),
    SearchDatePresetSelected(SearchDatePreset),
    SearchSizePresetSelected(SearchSizePreset),
    SearchCustomSizeMinChanged(String),
    SearchCustomSizeMaxChanged(String),
    SearchFiltersReset,
    SearchKeywordCleared,
    SearchWorkspaceClosed,
    SearchResultsLoaded(IndexedSearchRequest, IndexedSearchOutcome),
    SearchScopeRootValidated(
        IndexedSearchRequest,
        file_search::SearchQuery,
        tokio_util::sync::CancellationToken,
        bool,
    ),
    SearchDirectoryBatchLoaded(u64, Vec<SearchHit>),
    SearchDirectoryFinished(u64, DirectoryFallbackOutcome),
    SearchResultPressed(PathBuf),
    SearchResultRightClicked(PathBuf),
    SearchOpenContainingDirectory(PathBuf),
    SearchDeletePermanentlySelected,
    SearchResultsScrolled {
        offset_y: f32,
        viewport_height: f32,
    },
    SearchServiceEnsured(
        SearchServiceStatusRequest,
        Result<SearchServiceStatus, SearchServiceDiagnostic>,
    ),
    SearchServiceStatusRefreshRequested,
    SearchServiceStatusLoaded(
        SearchServiceStatusRequest,
        Result<SearchServiceStatus, SearchServiceDiagnostic>,
    ),
    SearchPathConfigurationLoaded(
        Result<
            (
                VersionedSearchPathPreferences,
                SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ),
    SearchPathConfigurationApplied(
        Result<
            (
                VersionedSearchPathPreferences,
                SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ),
    SearchDirectoryFallbackConfigurationLoaded(
        u64,
        String,
        Result<
            (
                VersionedSearchPathPreferences,
                SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ),
    SearchPathInputChanged(SearchPathEntryKind, String),
    SearchPathInputCommitted(SearchPathEntryKind),
    SearchPathDirectoryChooserPressed(SearchPathEntryKind),
    SearchPathDirectoryChosen(SearchPathEntryKind, Result<Option<PathBuf>, String>),
    SearchPathEntryRemoved(SearchPathEntryKind, PathBuf),
    SearchPathConfigurationRetryPressed,
    SearchServiceRestartRequested,
    SearchServiceForceRestartPressed,
    SearchServiceRecoveryFinished(
        SearchServiceRecoveryAction,
        Result<SearchServiceStatus, SearchServiceDiagnostic>,
    ),
    SearchServiceIncidentDetailsToggled(SearchServiceDiagnosticKind),
    SearchServiceIncidentDetailsCopyRequested(SearchServiceDiagnosticKind),
    SystemThemeDetected(Theme),
    MatugenThemeUpdated(Result<Option<Theme>, String>),
    UserPreferencesSaved(Result<(), String>),
    AppConfigSaved(Result<(), String>),
    ColumnWidthOverrideSaved(Result<(), String>),
    ExpandedDirectoryDiscoveryBatch(ExpandedDirectoryLoadRequest, DirectoryDiscoveryBatch),
    ExpandedDirectoryEntriesReady(
        ExpandedDirectoryLoadRequest,
        Result<PrebuiltDirectoryDiscovery, DirectoryLoadFailure>,
    ),
    ObservedDirectoryChanges(file_core::DirectoryEntryChanges),
    SettingsOpened,
    SettingsCategorySelected(SettingsCategory),
    SettingsSubpageOpened(SettingsSubpage),
    SettingsSubpageClosed,
    AboutRepositoryLinkPressed,
    AboutRepositoryLinkOpened(Result<(), String>),
    ContextMenuSettingsPageShifted(ContextMenuSettingsPageStep),
    ContextMenuSettingsItemToggled {
        page: ContextMenuSettingsPage,
        index: usize,
    },
    ContextMenuSettingsDragStarted {
        page: ContextMenuSettingsPage,
        index: usize,
    },
    ContextMenuSettingsResetRequested(ContextMenuSettingsPage),
    ContextMenuSettingsResetConfirmed(ContextMenuSettingsPage),
    ThemeModeSelected(ThemeMode),
    ColorSchemeFamilySelected(ColorSchemeFamily),
    ColorSchemePresetSelected(ColorSchemePreset),
    CustomColorSchemeImportPressed,
    CustomColorSchemeImportCompleted(Result<Option<String>, String>),
    WindowChromeLayoutSelected(WindowChromeLayout),
    WindowControlVisibilityToggled(WindowControlKind),
    WindowControlSideSelected(WindowControlKind, WindowControlSide),
    WindowControlMoveRequested(WindowControlKind, WindowControlMoveDirection),
    WindowControlsReset,
    ApplicationLogsRefreshRequested,
    ApplicationLogsLoaded(
        ApplicationLogRequest,
        Result<Vec<ApplicationLogEntry>, String>,
    ),
    ApplicationLogThresholdSelected(ApplicationLogLevel),
    ShowHiddenFilesToggled,
    ListDirectorySizeDisplayModeToggled,
    VisibleColumnCountSelected(usize),
    ColumnWidthAdjustModeSelected(crate::config::ColumnWidthAdjustMode),
    NetworkListThumbnailDownloadsToggled,
    SearchContentIndexingToggled,
    PreviewSizeLimitInputChanged(usize, String),
    PreviewSizeLimitInputCommitted(usize),
    PreviewDirectoryExpandLevelsInputChanged(String),
    PreviewDirectoryExpandLevelsInputCommitted,
    PreviewExtensionInputChanged(usize, String),
    PreviewExtensionInputCommitted(usize),
    PreviewExtensionExpandToggled(usize),
    PreviewExtensionRemoved(usize, String),
    PreviewExtensionResetRequested(usize),
    PreviewExtensionResetConfirmed(usize),
    LanguageSettingSelected(UiLanguageSetting),
    StartupLocationPolicySelected(crate::config::StartupLocationPolicy),
    LaunchWindowPolicySelected(crate::config::LaunchWindowPolicy),
    StartupSessionClassified(ClassifiedStartupSession),
    StartupCustomDirectoryInputChanged(String),
    StartupCustomDirectoryCommitted,
    StartupCustomDirectoryValidated(
        StartupDirectoryValidationRequest,
        StartupDirectoryAvailability,
    ),
    BrowserSessionSaved(Result<(), String>),
    BrowserSessionSaveDelayElapsed,
    ApplicationWindowClosed(window::Id),
    ApplicationWindowCloseCommandsFinished,
    ApplicationShutdownPersisted(Result<(), String>),
    FileOperationVerificationSelected(FileOperationVerification),
    TerminalEmulatorSelected(TerminalEmulator),
    TerminalShellSelected(String),
    RenderingGpuPreferenceSelected(RenderingGpuPreference),
    RendererRestartRequested,
    RendererRestartNoticeDismissed,
    SmoothScrollWheel(ScrollbarRegion, mouse::ScrollDelta),
    ScrollbarViewportChanged {
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
        event: Box<Message>,
    },
    ScrollbarLayoutVerified {
        region: ScrollbarRegion,
        viewport: ScrollbarViewport,
    },
    ScrollbarAutoHideElapsed(u64),
    WindowChromeAnimationTick,
    PreviewWindowInitialChromeElapsed(u64),
    SqlitePreviewTablesScrolled,
    SqlitePreviewDataScrolled,
    SidebarScrolled,
    SettingsScrolled,
    PropertiesScrolled,
    OpenWithApplicationsScrolled,
    OperationQueueScrolled,
    BatchRenamePreviewScrolled,
    SearchHistoryScrolled,
    PreviewDirectoryScrolled,
    PreviewArchiveScrolled,
    ColumnBrowserScrolled(BrowserPaneId, f32, f32),
    ColumnScrolled(BrowserPaneId, PathBuf, f32, f32),
    /// 列表滚动帧:offset_y 为内容偏移,viewport 是 iced 实测的可视区
    /// 窗口坐标矩形——hover 滚动补偿用它把存量光标位置换算成行流内
    /// 落点,与渲染同源,不另推布局。
    ListScrolled(BrowserPaneId, f32, iced::Rectangle),
    /// 大图滚动帧:语义同 ListScrolled,网格无表头,flow 顶点即内容偏移。
    IconGridScrolled(BrowserPaneId, f32, iced::Rectangle),
    ColumnResizeStarted(BrowserPaneId, usize),
    OpenDirectoryFromMiddleClick(BrowserPaneId, PathBuf),
    OpenTrashInNewTab(BrowserPaneId),
    TabPressed(BrowserPaneId, usize),
    TabCloseRequested(BrowserPaneId, usize),
    TabDragEntered(BrowserPaneId, usize),
    TabDragFinished,
    TabFileDropEntered(FileDragGestureId, TabFileDropTarget),
    TabFileDropExited(FileDragGestureId, TabFileDropTarget),
    TabFileDropReleased(FileDragGestureId, TabFileDropTarget),
    TabFileDropHoverElapsed(TabDropHover),
    PaneBack(BrowserPaneId),
    PaneForward(BrowserPaneId),
    PaneUp(BrowserPaneId),
    NavigateTo(PathBuf),
    OpenPath(PathBuf),
    /// 右键「智能解压到当前文件夹」：对选中集合中的每个归档做单根判定后入队。
    SmartExtractSelected,
    /// 智能解压的单根判定回流：据此计算目的地并启动既有解压流。
    SmartExtractDestinationResolved {
        archive: PathBuf,
        single_root: Result<Option<String>, String>,
    },
    /// 右键「解压到 <包名>/」：无条件在归档旁建包名文件夹后逐个解压。
    ExtractSelectedToArchiveFolder,
    TrashOpened,
    Back,
    Forward,
    AddressInputFocusChecked(BrowserPaneId, bool),
    RenameInputFocusChecked(bool),
    RenameInputChanged(String),
    RenameInputUndoRequested,
    RenameInputRedoRequested,
    BeginRename(PathBuf),
    OpenTerminalHere(PathBuf),
    RenameSelected,
    CreateDirectory(PathBuf),
    CreateEmptyFile(PathBuf),
    TrashSelected,
    RestoreSelected,
    EmptyTrashRequested,
    CopySelected,
    DuplicateSelected,
    NewFolderFromSelection,
    CopyPathSelected,
    CreateSymlinkSelected,
    PathTextCopied(Result<(), String>),
    MoveSelected,
    PastePending,
    FileClipboardWriteFinished(Result<(), String>),
    DesktopClipboardReadFinished {
        paste_directory: PathBuf,
        fallback_operation: Option<PendingOperation>,
        content: Result<Option<DesktopClipboardContent>, String>,
    },
    ClipboardFileCreated(Result<PathBuf, String>),
    DesktopActivationReceived(DesktopActivationEvent),
    DesktopActivationRuntimeFailed(String),
    WaylandDndWindowHandleLoaded(Result<Option<WaylandDndWindowHandle>, String>),
    WaylandFilesDropped(WaylandDndFileDrop),
    WaylandFileDropFailed(WaylandFileDropTargetSessionId, String),
    WaylandFileDragSourceEvent(WaylandFileDragSourceEvent),
    WaylandFileDropTargetEvent(WaylandFileDropTargetEvent),
    WaylandDndRuntimeFailed(String),
    /// 拖出位图的后台预渲染完成;gesture_id 用于校验结果归属,拖拽已
    /// 取消或手势更替时迟到结果直接丢弃。
    WaylandDragIconReady {
        gesture_id: FileDragGestureId,
        icon: Result<WaylandFileDragIcon, String>,
    },
    FileDropOperationSelected(FileClipboardOperation),
    FileDropCancelled,
    TransferConflictsChecked {
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
        conflicts: Vec<TransferConflictItem>,
    },
    /// 「合并」把冲突目录展开成子项传输后的回执;随后走统一的冲突复查。
    TransferConflictMergesExpanded {
        mode: TransferConflictMode,
        expansion: Result<(Vec<QueuedTransfer>, Vec<TransferConflictItem>), String>,
    },
    TransferConflictChoiceSelected(TransferConflictChoice),
    TransferConflictApplyToAllToggled,
    TransferConflictCancelRequested,
}

// 迁移期机械映射：Message::Preview(pm) 在 update 入口解包后经本函数
// 转发到既有散装变体的处理分支（零行为变化）；PreviewEngine 状态机组
// 与视图组迁移完成后，处理分支改为直接吃 PreviewMessage，本函数随之
// 删除。变体与 design.md 附录 A 基线一一对应
// （ContextMenuPreviewExpansionChanged 属右键菜单设置域，不在此列）。
impl From<PreviewMessage> for Message {
    fn from(message: PreviewMessage) -> Self {
        match message {
            PreviewMessage::PreviewLoaded(path, outcome) => Message::PreviewLoaded(path, outcome),
            PreviewMessage::DocumentPreview(inner) => Message::DocumentPreview(inner),
            PreviewMessage::SqlitePreview(inner) => Message::SqlitePreview(inner),
            PreviewMessage::RemotePreviewCache(inner) => Message::RemotePreviewCache(inner),
            PreviewMessage::AnimatedImagePreviewLoaded(path, generation, outcome) => {
                Message::AnimatedImagePreviewLoaded(path, generation, outcome)
            }
            PreviewMessage::OriginalImagePreviewLoaded(path, generation, outcome) => {
                Message::OriginalImagePreviewLoaded(path, generation, outcome)
            }
            PreviewMessage::RetryImagePreview(path) => Message::RetryImagePreview(path),
            PreviewMessage::PreviewDirectoryChildrenLoaded(path, outcome) => {
                Message::PreviewDirectoryChildrenLoaded(path, outcome)
            }
            PreviewMessage::TextPreviewContentScrolled {
                lines,
                viewport_height,
            } => Message::TextPreviewContentScrolled {
                lines,
                viewport_height,
            },
            PreviewMessage::TextPreviewViewerScrolled {
                lines,
                offset_y,
                viewport_height,
            } => Message::TextPreviewViewerScrolled {
                lines,
                offset_y,
                viewport_height,
            },
            PreviewMessage::TextPreviewViewportSynced {
                offset_y,
                viewport_height,
            } => Message::TextPreviewViewportSynced {
                offset_y,
                viewport_height,
            },
            PreviewMessage::TextPreviewContentHeightChanged(content_height) => {
                Message::TextPreviewContentHeightChanged(content_height)
            }
            PreviewMessage::TextPreviewChunkLoaded {
                path,
                generation,
                start_offset,
                outcome,
            } => Message::TextPreviewChunkLoaded {
                path,
                generation,
                start_offset,
                outcome,
            },
            PreviewMessage::MarkdownPreviewScrolled {
                offset_y,
                viewport_height,
                content_height,
            } => Message::MarkdownPreviewScrolled {
                offset_y,
                viewport_height,
                content_height,
            },
            PreviewMessage::MarkdownPreviewModeSelected(mode) => {
                Message::MarkdownPreviewModeSelected(mode)
            }
            PreviewMessage::ImagePreviewDimensionsLoaded(path, generation, outcome) => {
                Message::ImagePreviewDimensionsLoaded(path, generation, outcome)
            }
            PreviewMessage::PreviewImageViewport(inner) => Message::PreviewImageViewport(inner),
            PreviewMessage::AnimatedImageFrameLoaded(frame) => {
                Message::AnimatedImageFrameLoaded(frame)
            }
            PreviewMessage::AnimatedImagePreviewFinished(path, generation) => {
                Message::AnimatedImagePreviewFinished(path, generation)
            }
            PreviewMessage::AnimatedImagePreviewFailed(path, generation, error) => {
                Message::AnimatedImagePreviewFailed(path, generation, error)
            }
            PreviewMessage::AnimatedImageSeekRequested(position) => {
                Message::AnimatedImageSeekRequested(position)
            }
            PreviewMessage::AnimatedImageSeekCommitted => Message::AnimatedImageSeekCommitted,
            PreviewMessage::AudioPreviewPlaybackToggled => Message::AudioPreviewPlaybackToggled,
            PreviewMessage::AudioPreviewStarted(path, outcome) => {
                Message::AudioPreviewStarted(path, outcome)
            }
            PreviewMessage::AudioPreviewSeekRequested(position) => {
                Message::AudioPreviewSeekRequested(position)
            }
            PreviewMessage::AudioPreviewVolumeChanged(volume) => {
                Message::AudioPreviewVolumeChanged(volume)
            }
            PreviewMessage::AudioPreviewTick => Message::AudioPreviewTick,
            PreviewMessage::VideoPreviewPlaybackToggled => Message::VideoPreviewPlaybackToggled,
            PreviewMessage::VideoPreviewAudioStarted(path, generation, outcome) => {
                Message::VideoPreviewAudioStarted(path, generation, outcome)
            }
            PreviewMessage::VideoPreviewMetadataLoaded(path, outcome) => {
                Message::VideoPreviewMetadataLoaded(path, outcome)
            }
            PreviewMessage::VideoPreviewSeekRequested(position) => {
                Message::VideoPreviewSeekRequested(position)
            }
            PreviewMessage::VideoPreviewSeekCommitted => Message::VideoPreviewSeekCommitted,
            PreviewMessage::VideoPreviewVolumeChanged(volume) => {
                Message::VideoPreviewVolumeChanged(volume)
            }
            PreviewMessage::VideoPreviewTick => Message::VideoPreviewTick,
            PreviewMessage::VideoPreviewFrameLoaded(frame) => {
                Message::VideoPreviewFrameLoaded(frame)
            }
            PreviewMessage::VideoPreviewSeekFrameFailed(path, generation, position, error) => {
                Message::VideoPreviewSeekFrameFailed(path, generation, position, error)
            }
            PreviewMessage::VideoPreviewFinished(path, generation) => {
                Message::VideoPreviewFinished(path, generation)
            }
            PreviewMessage::VideoPreviewFailed(path, generation, error) => {
                Message::VideoPreviewFailed(path, generation, error)
            }
            PreviewMessage::PreviewTreeDirectoryToggled(entry_id) => {
                Message::PreviewTreeDirectoryToggled(entry_id)
            }
            PreviewMessage::PreviewTreeAnimationTick => Message::PreviewTreeAnimationTick,
            PreviewMessage::RightPreviewPanelResizeStarted => {
                Message::RightPreviewPanelResizeStarted
            }
            PreviewMessage::RightPreviewPanelRatioResizeStarted => {
                Message::RightPreviewPanelRatioResizeStarted
            }
            PreviewMessage::RightPreviewPanelInfoLoaded { path, snapshot } => {
                Message::RightPreviewPanelInfoLoaded { path, snapshot }
            }
            PreviewMessage::SqliteTablesResizeStarted => Message::SqliteTablesResizeStarted,
            PreviewMessage::ToggleRightPreviewPanel => Message::ToggleRightPreviewPanel,
            PreviewMessage::PreviewWindowPinToggled => Message::PreviewWindowPinToggled,
            PreviewMessage::PreviewSizeLimitInputChanged(kind_index, value) => {
                Message::PreviewSizeLimitInputChanged(kind_index, value)
            }
            PreviewMessage::PreviewSizeLimitInputCommitted(kind_index) => {
                Message::PreviewSizeLimitInputCommitted(kind_index)
            }
            PreviewMessage::PreviewDirectoryExpandLevelsInputChanged(value) => {
                Message::PreviewDirectoryExpandLevelsInputChanged(value)
            }
            PreviewMessage::PreviewDirectoryExpandLevelsInputCommitted => {
                Message::PreviewDirectoryExpandLevelsInputCommitted
            }
            PreviewMessage::PreviewExtensionInputChanged(kind_index, value) => {
                Message::PreviewExtensionInputChanged(kind_index, value)
            }
            PreviewMessage::PreviewExtensionInputCommitted(kind_index) => {
                Message::PreviewExtensionInputCommitted(kind_index)
            }
            PreviewMessage::PreviewExtensionExpandToggled(kind_index) => {
                Message::PreviewExtensionExpandToggled(kind_index)
            }
            PreviewMessage::PreviewExtensionRemoved(kind_index, extension) => {
                Message::PreviewExtensionRemoved(kind_index, extension)
            }
            PreviewMessage::PreviewExtensionResetRequested(kind_index) => {
                Message::PreviewExtensionResetRequested(kind_index)
            }
            PreviewMessage::PreviewExtensionResetConfirmed(kind_index) => {
                Message::PreviewExtensionResetConfirmed(kind_index)
            }
            PreviewMessage::PreviewWindowInitialChromeElapsed(generation) => {
                Message::PreviewWindowInitialChromeElapsed(generation)
            }
            PreviewMessage::SqlitePreviewTablesScrolled => Message::SqlitePreviewTablesScrolled,
            PreviewMessage::SqlitePreviewDataScrolled => Message::SqlitePreviewDataScrolled,
            PreviewMessage::PreviewDirectoryScrolled => Message::PreviewDirectoryScrolled,
            PreviewMessage::PreviewArchiveScrolled => Message::PreviewArchiveScrolled,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PendingOperation {
    Copy(Vec<PathBuf>),
    Move(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileDropPrompt {
    pub(crate) paste_directory: PathBuf,
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) enum DestructiveActionConfirmation {
    DeleteTrashEntries { entries: Vec<TrashRestoreEntry> },
    DeletePermanently { paths: Vec<PathBuf> },
    EmptyTrash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferConflictMode {
    Copy,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferConflictChoice {
    Replace,
    Skip,
    Rename,
    /// 保留两者:按共享命名规则给冲突目标起新名后原样传输。
    KeepBoth,
    /// 合并:仅源与目标都是目录时提供,展开成子项传输后再逐项走冲突流程。
    Merge,
}

#[derive(Debug, Clone)]
pub(crate) struct TransferConflictState {
    pub(crate) mode: TransferConflictMode,
    pub(crate) transfers: Vec<QueuedTransfer>,
    pub(crate) conflicts: Vec<TransferConflictItem>,
    pub(crate) current_index: usize,
    pub(crate) apply_to_all: bool,
}

impl TransferConflictState {
    pub(crate) fn current_conflict(&self) -> Option<&TransferConflictItem> {
        self.conflicts.get(self.current_index)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StartupEnvironment {
    pub(crate) home: PathBuf,
    pub(crate) system_language: UiLanguage,
    pub(crate) user_config: UserConfig,
    pub(crate) state_database_path: PathBuf,
    pub(crate) rendering_environment_status: StartupRenderingEnvironmentStatus,
}

#[derive(Debug, Clone)]
pub(crate) enum ContextMenuState {
    FileArea(FileContextMenuState),
    Search(SearchContextMenuState),
    SearchEntryTypes(SearchEntryTypeMenuState),
    ListColumns(ListColumnMenuState),
    SidebarBookmark(SidebarBookmarkContextMenuState),
    SidebarDevice(SidebarDeviceContextMenuState),
    NetworkConnection(SidebarNetworkConnectionContextMenuState),
}

impl ContextMenuState {
    pub(crate) fn position(&self) -> Point {
        match self {
            Self::FileArea(menu) => menu.position,
            Self::Search(menu) => menu.position,
            Self::SearchEntryTypes(menu) => menu.position,
            Self::ListColumns(menu) => menu.position,
            Self::SidebarBookmark(menu) => menu.position,
            Self::SidebarDevice(menu) => menu.position,
            Self::NetworkConnection(menu) => menu.position,
        }
    }

    pub(crate) fn paste_directory(&self) -> Option<&PathBuf> {
        match self {
            Self::FileArea(menu) => Some(&menu.paste_directory),
            Self::Search(_) => None,
            Self::SearchEntryTypes(_) => None,
            Self::ListColumns(_) => None,
            Self::SidebarBookmark(_) => None,
            Self::SidebarDevice(_) => None,
            Self::NetworkConnection(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchContextMenuState {
    pub(crate) target: PathBuf,
    pub(crate) position: Point,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchEntryTypeMenuState {
    pub(crate) position: Point,
}

#[derive(Debug, Clone)]
pub(crate) struct ListColumnMenuState {
    pub(crate) position: Point,
}

#[derive(Debug, Clone)]
pub(crate) struct FileContextMenuState {
    pub(crate) target: Option<PathBuf>,
    pub(crate) target_is_directory: bool,
    pub(crate) paste_directory: PathBuf,
    pub(crate) can_batch_rename: bool,
    /// 选中项含远程挂载路径时为 false:gvfs 上的 symlink 不可靠,菜单隐藏「创建符号链接」。
    pub(crate) can_create_symlink: bool,
    pub(crate) delete_action: FileDeleteAction,
    pub(crate) position: Point,
    pub(crate) expansion: FileContextMenuExpansion,
    /// 打开菜单时按 pane 视图模式求值的分组方式入口门控
    /// （仅空白菜单为真）；渲染层只读该结果，不再感知视图模式。
    pub(crate) grouping_entry_visible: bool,
    /// 打开菜单那一刻选中集合里的压缩包条目（真实目录内），解压两项的数据源。
    pub(crate) selection_archives: Vec<PathBuf>,
    /// 菜单发起位置在压缩包内部（含包根）：条目菜单切换为只读子集。
    pub(crate) inside_archive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDeleteAction {
    MoveToTrash,
    DeletePermanently,
    MixedSelection,
}

impl FileDeleteAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MoveToTrash => "Move to Trash",
            Self::DeletePermanently => "Delete Permanently",
            Self::MixedSelection => "Delete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileContextMenuExpansion {
    None,
    /// 展开锚点组的子菜单;锚点是组内一级行对应的菜单项。
    Group(FileAreaMenuItem),
    /// 空白菜单末尾固定的「分组方式」子菜单;不进可配置菜单体系,
    /// 与组锚点共用悬停展开状态。
    FileGrouping,
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarBookmarkContextMenuState {
    pub(crate) path: PathBuf,
    pub(crate) position: Point,
}

// 纯搬移：SidebarLocation/SidebarLocationKind 已下沉 bennu-sidebar
// （主程序与 portal 共用）；re-export 维持 crate::model::* 既有路径。
pub(crate) use bennu_sidebar::{SidebarLocation, SidebarLocationKind};

pub(crate) const TRASH_LOCATION_LABEL: &str = "Trash";

pub(crate) fn trash_location_path() -> PathBuf {
    PathBuf::from("trash:///")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigationMode {
    RecordHistory,
    KeepHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathSuggestionDirection {
    Next,
    Previous,
}
