use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationEvent, DesktopClipboardContent, FileClipboardOperation,
    OpenWithApplicationList, StorageDeviceId, StorageDeviceSnapshot, TerminalEmulator,
    WaylandDndFileDrop, WaylandDndWindowHandle, WaylandFileDragSourceEvent,
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

pub(crate) use crate::text_preview::{
    MarkdownPreviewMode, TextPreviewChunk, TextPreviewDocument, TextPreviewFormat,
    TextPreviewLineLimitNotice,
};
pub(crate) use file_core::{TransferConflictItem, TransferConflictMetadata};

mod address_bar;
pub(crate) use address_bar::{
    allocate_breadcrumb_widths, breadcrumb_segments, displayed_address_directory,
    AddressBarTransition, AddressEditingSession, AddressEditingSessionId, AddressSuggestionRequest,
    BreadcrumbSegment, BreadcrumbSegmentKind,
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
mod sqlite_preview;
pub(crate) use sqlite_preview::{
    SqlQueryOutcome, SqliteCellValue, SqliteDatabasePreview, SqlitePreviewMessage,
    SqlitePreviewTab, SqliteTableData, SqliteTableSummary, SQLITE_ROW_LIMIT,
};
mod preview;
pub(crate) use preview::RightPreviewPanelInfoSnapshot;
pub(crate) use preview::{
    image_preview_zoom_multiplier, AudioPreviewPlayback, AudioPreviewPlaybackStatus,
    ImagePreviewContent, ImagePreviewViewport, PreviewContent, PreviewImageViewportMessage,
    PreviewSize, PreviewState, PreviewTreeDirectoryChildren, PreviewTreeEntry,
    PreviewWindowChromeState, PreviewWindowProfile, RemotePreviewCacheFinished,
    RemotePreviewCacheMessage, RemotePreviewCacheProgress, RemotePreviewDownload,
    VideoPreviewFrame, VideoPreviewPlayback, VideoPreviewPlaybackStatus,
    VideoPreviewSeekCompletion, PREVIEW_WINDOW_INITIAL_CONTROLS_DURATION,
};
mod settings;
pub(crate) use settings::{SettingsCategory, SettingsSubpage};
mod context_menu_items;
mod context_menu_layout;
#[cfg(test)]
mod context_menu_layout_tests;
pub(crate) use context_menu_items::{
    FileAreaMenuItem, SearchResultMenuItem, TrashMenuItem,
};
pub(crate) use context_menu_layout::{
    ContextMenuLayoutConfigValues, ContextMenuPreferences, ContextMenuSettingsDragState,
    ContextMenuSettingsPage, ContextMenuSettingsPageStep, ContextMenuSettingsRow,
    CONTEXT_MENU_SETTINGS_PAGES, CONTEXT_MENU_SETTINGS_ROW_PITCH,
};
mod window_controls;
pub(crate) use window_controls::{
    WindowChromeLayout, WindowControlKind, WindowControlMoveDirection, WindowControlPlacement,
    WindowControlSide, WindowControlVisibility, WindowControlsConfig, WindowFrameState,
    WINDOW_TITLE_BAR_HEIGHT, WINDOW_TOP_BAR_HEIGHT, MAIN_TOOLBAR_ROW_HEIGHT,
};
mod application_logs;
pub(crate) use application_logs::{
    bounded_application_log_message, sanitized_application_log_detail, ApplicationLogEntry,
    ApplicationLogLevel, ApplicationLogRequest, ApplicationLogSource, ApplicationLogViewState,
    APPLICATION_LOG_ENTRY_LIMIT, APP_JOURNAL_IDENTIFIER, SEARCH_JOURNAL_IDENTIFIER,
};
pub(crate) mod search;
pub(crate) use search::{
    DirectoryFallbackOutcome, IndexedSearchOutcome, IndexedSearchRequest, SearchDateField,
    SearchDatePreset, SearchDirectoryScope, SearchEntryTypePreset, SearchHistory,
    SearchHistoryInteraction, SearchInputFocus, SearchInputFocusCheckOrigin,
    SearchInputFocusCheckRequest, SearchInputStabilizationRequest, SearchInputStabilizationSubject,
    SearchKeyboardSelection, SearchResultCompletion, SearchSelectionGesture, SearchSelectionStep,
    SearchWorkspaceSessionId, SearchWorkspaceState, SEARCH_RESULT_TOTAL_LIMIT,
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
    FileDragHitTestBounds, FileDragNativeDndState, FileDragPhase, FileDragState,
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScrollbarViewport {
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) viewport_width: f32,
    pub(crate) viewport_height: f32,
    pub(crate) content_width: f32,
    pub(crate) content_height: f32,
}

pub(crate) const SCROLLBAR_HOVER_WIDTH: f32 = 14.0;
pub(crate) const SCROLLBAR_MIN_THUMB_LENGTH: f32 = 28.0;

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
    FileOperationProgressed(u64, crate::operation_progress::FileOperationProgressUpdate),
    FileOperationDirectMovesCommitted {
        task_id: u64,
        commits: Vec<crate::commands::DurableDirectMoveCommit>,
    },
    FileOperationMovesRenamed {
        task_id: u64,
        moves: Vec<crate::operation_history::CompletedTransfer>,
    },
    FileOperationFinished(u64, FileOperationCompletion),
    FileOperationPersistenceFinished(crate::operation_queue::FileOperationPersistenceOutcome),
    OperationProgressAnimationTick,
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
        snapshot: Result<Box<crate::model::preview::RightPreviewPanelInfoSnapshot>, String>,
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
    ColumnEntryBoundsMeasured(Vec<ColumnEntryBounds>),
    BreadcrumbDropTargetBoundsMeasured(u64, Vec<BreadcrumbDropTargetBounds>),
    FileDropLayoutMeasured(FileDropLayoutRequest, FileDragHitTestBounds),
    PasteTargetMeasured(FileDragHitTestBounds),
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
    FileContextMenuExpansionChanged(FileContextMenuExpansion),
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
    ContextMenuSettingsPageShifted(ContextMenuSettingsPageStep),
    ContextMenuSettingsItemToggled {
        page: ContextMenuSettingsPage,
        index: usize,
    },
    ContextMenuSettingsDragStarted { page: ContextMenuSettingsPage, index: usize },
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
    ListScrolled(BrowserPaneId, f32, f32),
    IconGridScrolled(BrowserPaneId, f32, f32, f32),
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
    FileDropOperationSelected(FileClipboardOperation),
    FileDropCancelled,
    TransferConflictsChecked {
        mode: TransferConflictMode,
        transfers: Vec<QueuedTransfer>,
        conflicts: Vec<TransferConflictItem>,
    },
    TransferConflictChoiceSelected(TransferConflictChoice),
    TransferConflictApplyToAllToggled,
    TransferConflictCancelRequested,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ScrollbarVisibility {
    Hidden,
    VisibleWithOpacity(f32),
    Visible,
}

impl ScrollbarVisibility {
    pub(crate) fn with_opacity(opacity: f32) -> Self {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= f32::EPSILON {
            Self::Hidden
        } else if (1.0 - opacity) <= f32::EPSILON {
            Self::Visible
        } else {
            Self::VisibleWithOpacity(opacity)
        }
    }

    pub(crate) fn opacity(self) -> f32 {
        match self {
            Self::Hidden => 0.0,
            Self::VisibleWithOpacity(opacity) => opacity,
            Self::Visible => 1.0,
        }
    }
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
    pub(crate) delete_action: FileDeleteAction,
    pub(crate) position: Point,
    pub(crate) expansion: FileContextMenuExpansion,
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
    NewEntry,
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarBookmarkContextMenuState {
    pub(crate) path: PathBuf,
    pub(crate) position: Point,
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarLocation {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) kind: SidebarLocationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarLocationKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Bookmark,
}

impl SidebarLocationKind {
    pub(crate) fn is_user_favorite(self) -> bool {
        matches!(
            self,
            Self::Desktop
                | Self::Documents
                | Self::Downloads
                | Self::Pictures
                | Self::Music
                | Self::Videos
                | Self::Bookmark
        )
    }
}

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
