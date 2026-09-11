mod application_logs;
mod application_shutdown;
pub(crate) mod archive_creation;
pub(crate) mod archive_extraction;
pub(crate) mod archive_password;
mod batch_rename;
pub(crate) mod checksum;
mod column_resize;
mod column_scroll;
mod config_persistence;
mod context_menu_settings;
pub(crate) mod convert;
mod desktop_activation;
mod directory_expansion_loading;
mod directory_metadata_demand;
mod entry_changes;
mod directory_recovery;
mod events;
mod file_drag_edge_scroll;
mod file_operation_notifications;
mod file_operations;
mod global_error;
mod icon_grid_expansion;
#[cfg(test)]
mod icon_grid_file_operation_tests;
#[cfg(test)]
mod icon_grid_invariant_tests;
mod list_directory_summaries;
#[cfg(test)]
mod list_size_sorting_tests;
mod list_view_settings;
mod navigation;
mod network_connections;
mod network_settings;
mod open_with;
mod pane_drag;
pub(crate) mod panes;
mod path_suggestions;
mod paths;
mod persistence_feedback;
mod pointer_interactions;
mod preview_settings;
pub(crate) mod preview_state;
mod preview_window_view;
mod properties;
mod remote_mounts;
pub(crate) mod right_preview_panel;
mod rendering_settings;
mod runtime;
pub(crate) mod scrollbar;
mod search;
mod search_paths;
mod selection;
mod session_persistence;
mod session_restore;
mod shortcuts;
mod sidebar_bookmarks;
mod sidebar_devices;
mod sidebar_resize;
pub(crate) mod smooth_scroll;
mod split_resize;
mod startup;
mod startup_settings;
mod tabs;
mod text_input_shortcuts;
mod thumbnailing;
mod trash;
#[cfg(test)]
mod trash_tests;
mod update;
mod view_modes;
mod wayland_dnd;
mod window_chrome;
mod window_control_settings;
mod windows;
mod x11_dnd;

pub(crate) use runtime::run;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationEvent, DesktopActivationRuntime, NetworkConnectionId, StorageDeviceId,
    TerminalEmulator,
};
use file_core::{DirectoryDiscovery, DirectoryMetadataRequirement, ScanOptions, TrashEntry};
use iced::event;
use iced::keyboard;
use iced::window;
use iced::{time, Element, Point, Subscription, Task, Theme};

use crate::animated_image_preview::animated_image_preview_subscription;
use crate::app::application_shutdown::ApplicationShutdownPhase;
use crate::app::archive_creation::ArchiveCreationState;
use crate::app::archive_extraction::ArchiveExtractionState;
use crate::app::checksum::ChecksumState;
use crate::app::column_resize::ColumnResizeDrag;
use crate::app::convert::ConvertState;
use crate::app::events::global_event_message;
use crate::app::preview_state::PendingOriginalImagePreview;
use crate::app::preview_state::SqlitePreviewState;
use crate::app::preview_state::SqliteTablesResizeDrag;
use crate::app::runtime::{
    desktop_activation_subscription, directory_watch_subscription, matugen_theme_subscription,
    sidebar_device_refresh_subscription, system_theme_command, wayland_file_dnd_subscription,
    x11_file_dnd_subscription,
};
use crate::app::scrollbar::{ScrollbarState, SCROLLBAR_ANIMATION_INTERVAL};
use crate::app::sidebar_bookmarks::SidebarBookmarkMotionState;
use crate::app::sidebar_resize::SidebarResizeDrag;
use crate::app::smooth_scroll::MosScrollState;
use crate::app::split_resize::SplitResizeDrag;
use crate::app::tabs::{TabAnimationState, TabBarReveal};
use crate::app::windows::{
    default_preview_size, main_window_settings, MAIN_WINDOW_INITIAL_HEIGHT,
    MAIN_WINDOW_INITIAL_WIDTH,
};
use crate::command_line::ApplicationLaunchRequest;
use crate::commands::{
    ensure_search_service_command, file_operation_subscription, startup_environment_command,
    wayland_dnd_window_handle_command, x11_dnd_window_handle_command,
};
use crate::config;
use crate::config::UiLanguage;
use crate::document_preview::PendingDocumentPreview;
use crate::localization;
use crate::matugen_theme::{fallback_theme, AppearanceMode, ApplicationTheme};
use crate::model::search::SearchWorkspaceState;
use crate::model::{
    empty_directory_entry_snapshot, AddressBarTransition, AddressEditingSession,
    ApplicationLogViewState, AudioPreviewPlayback, BatchRenameState, BreadcrumbDropTargetBounds,
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode,
    ColumnBrowserViewport, ColumnEntryBounds, ContextMenuState, DestructiveActionConfirmation,
    DirectoryCollectionPhase, DirectoryEntrySnapshot, DirectoryLoadingPlaceholder,
    DirectoryOrderPhase, ExpandedDirectory, FileDragState, FileDropPrompt, FileDropSessionState,
    FilePropertiesState, IconGridExpansionState, IconGridViewport, ImagePreviewViewport,
    ListColumnKind, Message, PaneDragPointerPress, PaneDragState, PendingOperation, PreviewSize,
    PreviewState, PreviewWindowChromeState, PreviewWindowProfile, ScrollbarRegion,
    SearchServiceState, SelectionMarquee, ContextMenuSettingsDragState, ContextMenuSettingsPage, SettingsCategory, SettingsSubpage,
    SidebarBookmarkDragState, SidebarBookmarkDropSlot, SidebarLocation,
    StartupDirectoryValidationRequest, TabDragState, TextPreviewDocument, TransferConflictState,
    TrashRefreshState, VideoPreviewPlayback,
};
use crate::network_connections::{NetworkConnectionEditorState, NetworkConnectionState};
use crate::open_with::OpenWithState;
use crate::operation_history::FileOperationHistory;
use crate::operation_queue::FileOperationQueue;
use crate::shortcuts::ShortcutCaptureState;
use crate::sidebar_devices::SidebarDeviceState;
use crate::startup_rendering::StartupRenderingEnvironment;
use crate::startup_trace;
use crate::thumbnail_cache::{ColumnViewport, ThumbnailCache};
use crate::video_preview::video_preview_subscription;
use crate::view::{
    auxiliary_window_content, floating_preview_window_content, view_browser,
    view_properties_window, view_settings_window, window_resize_frame,
};
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);
const POINTER_DRAG_ACTIVATION_DISTANCE: f32 = 3.0;
const PREVIEW_TREE_ANIMATION_INTERVAL: Duration = crate::ui_pacing::FRAME_INTERVAL_60HZ;
const OPERATION_PROGRESS_ANIMATION_INTERVAL: Duration = Duration::from_millis(80);
const AUDIO_PREVIEW_TICK_INTERVAL: Duration = Duration::from_millis(250);
const NETWORK_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const SEARCH_SERVICE_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const APPLICATION_LOG_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectoryMetadataDemandKey {
    context: crate::model::DirectoryMetadataLoadContext,
    requirement: DirectoryMetadataRequirement,
    index: usize,
}

pub(crate) struct FileBrowser {
    pub(crate) home_dir: PathBuf,
    pub(crate) current_dir: PathBuf,
    pub(crate) is_trash_view: bool,
    pub(crate) entries: DirectoryEntrySnapshot,
    /// 主列表条目索引：仅随 [`Self::set_entries`] 失效，hover 热路径 O(1) 查找用。
    entry_index: Option<(DirectoryEntrySnapshot, HashMap<PathBuf, usize>)>,
    pub(crate) directory_discovery: Option<DirectoryDiscovery>,
    pub(crate) directory_loading_placeholder: Option<DirectoryLoadingPlaceholder>,
    pub(crate) trash_entries: Vec<TrashEntry>,
    pub(crate) trash_refresh: TrashRefreshState,
    /// 回收站批量任务运行期间,watcher 触发的回收站重扫被挂起,只置此标记;
    /// 批量任务终结时统一补刷一次,避免逐条全量重扫的平方级风暴。
    trash_batch_rescan_pending: bool,
    pub(crate) selected: Option<PathBuf>,
    selected_paths: HashSet<PathBuf>,
    pub(crate) hovered_entry: Option<PathBuf>,
    pub(crate) hovered_sidebar: Option<PathBuf>,
    pub(crate) hovered_sidebar_device: Option<StorageDeviceId>,
    pub(crate) hovered_network_connection: Option<NetworkConnectionId>,
    cursor_paste_directory: Option<PathBuf>,
    pub(crate) preview: Option<PreviewState>,
    pending_document_preview: Option<PendingDocumentPreview>,
    document_preview_generation: u64,
    remote_preview_download_cancel: Option<tokio_util::sync::CancellationToken>,
    pub(crate) text_preview_document: Option<TextPreviewDocument>,
    animated_image_preview_generation: u64,
    original_image_preview_generation: u64,
    original_image_preview_cancel: Option<tokio_util::sync::CancellationToken>,
    pending_original_image_preview: Option<PendingOriginalImagePreview>,
    remote_preview_download_generation: u64,
    text_preview_generation: u64,
    directory_load_generation: u64,
    pub(crate) sqlite_preview: Option<SqlitePreviewState>,
    sqlite_preview_generation: u64,
    sqlite_tables_resize_drag: Option<SqliteTablesResizeDrag>,
    directory_load_cancel: Option<tokio_util::sync::CancellationToken>,
    next_directory_metadata_request_generation: u64,
    directory_metadata_in_flight: HashSet<DirectoryMetadataDemandKey>,
    selection_metadata_signature: u64,
    pub(crate) audio_preview: Option<AudioPreviewPlayback>,
    pub(crate) video_preview: Option<VideoPreviewPlayback>,
    pub(crate) preview_size: PreviewSize,
    pub(crate) text_preview_content_height: f32,
    pending_preview_resize: Option<PreviewSize>,
    preview_window_profile: PreviewWindowProfile,
    preview_window_pinned: bool,
    // 当前预览窗口展示的目标路径；Ready 态的目录/归档/原图变体不携带路径，
    // 空格 toggle 需要它判断“预览的仍是当前选中项”。
    preview_shown_path: Option<PathBuf>,
    preview_window_chrome: PreviewWindowChromeState,
    preview_window_bottom_controls: PreviewWindowChromeState,
    preview_window_drag_active: bool,
    preview_window_pointer_y: Option<f32>,
    preview_image_viewport: ImagePreviewViewport,
    preview_window_initial_chrome_generation: u64,
    main_window: window::Id,
    maximized_windows: HashSet<window::Id>,
    wayland_dnd: Option<wayland_dnd::WaylandDndRuntime>,
    x11_dnd: Option<x11_dnd::X11DndRuntime>,
    file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
    preview_window: Option<window::Id>,
    focused_window: window::Id,
    system_focused_window: Option<window::Id>,
    pub(crate) thumbnail_cache: ThumbnailCache,
    pub(crate) column_browser_viewport: ColumnBrowserViewport,
    pub(crate) column_viewports: HashMap<PathBuf, ColumnViewport>,
    icon_grid_viewports: HashMap<BrowserPaneId, PaneIconGridViewport>,
    pub(crate) icon_grid_expansion: Option<IconGridExpansionState>,
    list_expansion_follow: Option<view_modes::ListExpansionFollowPlan>,
    pub(crate) context_menu: Option<ContextMenuState>,
    pub(crate) open_with: Option<OpenWithState>,
    pub(crate) file_drop_prompt: Option<FileDropPrompt>,
    pub(crate) archive_creation: Option<ArchiveCreationState>,
    pub(crate) convert: Option<ConvertState>,
    pub(crate) checksum: Option<ChecksumState>,
    pub(crate) archive_extraction: Option<ArchiveExtractionState>,
    pub(crate) batch_rename: Option<BatchRenameState>,
    pub(crate) sidebar_locations: Vec<SidebarLocation>,
    pub(crate) sidebar_devices: SidebarDeviceState,
    pub(crate) network_connections: NetworkConnectionState,
    pub(crate) network_connection_editor: Option<NetworkConnectionEditorState>,
    pub(crate) sidebar_bookmark_drop_slot: Option<SidebarBookmarkDropSlot>,
    pub(crate) sidebar_bookmark_drag: Option<SidebarBookmarkDragState>,
    pub(crate) sidebar_bookmark_motion: HashMap<PathBuf, SidebarBookmarkMotionState>,
    pub(crate) sidebar_width: f32,
    sidebar_resize_drag: Option<SidebarResizeDrag>,
    pub(crate) right_preview_panel_open: bool,
    pub(crate) right_preview_panel_width: f32,
    right_preview_panel_resize_drag: Option<right_preview_panel::RightPreviewPanelResizeDrag>,
    pub(crate) right_preview_preview_ratio: f32,
    pub(crate) right_preview_panel_info: Option<crate::model::RightPreviewPanelInfoSnapshot>,
    right_preview_ratio_resize_drag: Option<right_preview_panel::RightPreviewPanelRatioDrag>,
    preview_load_surface: right_preview_panel::PreviewLoadSurface,
    pub(crate) renaming: Option<PathBuf>,
    pending_created_entry_rename: Option<PathBuf>,
    pub(crate) pending_operation: Option<PendingOperation>,
    pub(crate) transfer_conflict: Option<TransferConflictState>,
    pub(crate) destructive_action_confirmation: Option<DestructiveActionConfirmation>,
    settings_window: Option<window::Id>,
    properties_window: Option<window::Id>,
    pub(crate) properties: Option<FilePropertiesState>,
    properties_load_generation: u64,
    pub(crate) tabs: Vec<BrowserTab>,
    pub(crate) active_tab_id: usize,
    tab_bar_reveal: TabBarReveal,
    pub(crate) tab_animations: HashMap<usize, TabAnimationState>,
    pub(crate) panes: Vec<BrowserPane>,
    pub(crate) pane_layout: BrowserPaneLayout,
    tab_drag: Option<TabDragState>,
    pane_drag: Option<PaneDragState>,
    pane_drag_pointer_press: Option<PaneDragPointerPress>,
    split_resize_drag: Option<SplitResizeDrag>,
    pub(crate) selection_marquee: Option<SelectionMarquee>,
    pub(crate) file_drag: Option<FileDragState>,
    pub(crate) file_drop_session: Option<FileDropSessionState>,
    next_file_drag_gesture_id: u64,
    file_drop_layout_generation: u64,
    file_entry_bounds: Vec<ColumnEntryBounds>,
    breadcrumb_drop_target_bounds: Vec<BreadcrumbDropTargetBounds>,
    breadcrumb_drop_target_measurement_generation: u64,
    pub(crate) options: ScanOptions,
    application_launch_request: ApplicationLaunchRequest,
    user_config: config::UserConfig,
    pub(crate) preview_size_limit_mib_inputs: [String; 7],
    pub(crate) preview_size_limit_mib_errors: [Option<String>; 7],
    pub(crate) preview_extension_inputs: [String; 7],
    pub(crate) preview_extension_input_errors: [Option<String>; 7],
    pub(crate) preview_extension_expanded: [bool; 7],
    pub(crate) preview_extension_reset_confirmation: Option<usize>,
    pub(crate) preview_directory_expand_levels_input: String,
    pub(crate) preview_directory_expand_levels_error: Option<String>,
    pub(crate) startup_custom_directory_input: String,
    pub(crate) startup_custom_directory_error: Option<String>,
    pending_startup_directory_validation: Option<StartupDirectoryValidationRequest>,
    startup_directory_validation_generation: u64,
    pub(crate) rendering_gpu_preference: config::RenderingGpuPreference,
    pub(crate) renderer_restart_notice_visible: bool,
    startup_rendering_environment: StartupRenderingEnvironment,
    pub(super) pending_renderer_restart_environment:
        Option<crate::startup_rendering::StartupRenderingEnvironment>,
    pub(crate) address_editing: Option<AddressEditingSession>,
    pub(crate) address_bar_transition: Option<AddressBarTransition>,
    next_address_editing_session_id: u64,
    pub(crate) column_width_overrides: HashMap<usize, f32>,
    column_width_reference_content_widths: HashMap<usize, f32>,
    pub(crate) terminal_emulator: TerminalEmulator,
    pub(crate) terminal_panel: crate::terminal_panel::TerminalPanelState,
    pub(crate) selected_settings_category: SettingsCategory,
    pub(crate) context_menu_settings_page: ContextMenuSettingsPage,
    pub(crate) context_menu_reset_confirmation: Option<ContextMenuSettingsPage>,
    pub(crate) context_menu_settings_drag: Option<ContextMenuSettingsDragState>,
    pub(crate) settings_subpage: Option<SettingsSubpage>,
    pub(crate) expanded_color_scheme_family: Option<crate::matugen_theme::ColorSchemeFamily>,
    pub(crate) custom_color_scheme_import_error: Option<String>,
    pub(crate) search_service: SearchServiceState,
    pub(crate) search_workspace: Option<SearchWorkspaceState>,
    pub(crate) search_history_interaction: crate::model::SearchHistoryInteraction,
    next_search_workspace_session_id: u64,
    pub(crate) deepest_open_column_directory: Option<PathBuf>,
    focused_column_directory: Option<PathBuf>,
    last_pointer_clicked_column_directory: Option<PathBuf>,
    pub(crate) expanded_directories: HashMap<PathBuf, ExpandedDirectory>,
    pub(crate) view_mode: BrowserViewMode,
    pub(crate) rename_input: String,
    rename_input_history: file_operations::RenameInputHistory,
    pub(crate) directory_collection_phase: DirectoryCollectionPhase,
    pub(crate) directory_order_phase: DirectoryOrderPhase,
    global_error_notification: Option<global_error::GlobalErrorNotification>,
    next_global_error_notification_generation: u64,
    pub(crate) application_logs: ApplicationLogViewState,
    system_language: UiLanguage,
    pub(crate) cursor_position: Point,
    pub(crate) main_window_width: f32,
    pub(crate) main_window_height: f32,
    is_cursor_over_column_browser: bool,
    hovered_pane_id: Option<BrowserPaneId>,
    hovered_list_header_column: Option<(BrowserPaneId, ListColumnKind)>,
    keyboard_modifiers: keyboard::Modifiers,
    pub(crate) shortcut_capture: Option<ShortcutCaptureState>,
    selection_anchor: Option<PathBuf>,
    drag_selection_anchor: Option<PathBuf>,
    column_return_targets: HashMap<PathBuf, PathBuf>,
    pending_keyboard_column_focus: Option<PendingKeyboardColumnFocus>,
    pending_view_switch_reveal: Option<(BrowserPaneId, PathBuf)>,
    column_resize_drag: Option<ColumnResizeDrag>,
    list_column_resize_drag: Option<crate::app::list_view_settings::ListColumnResizeDrag>,
    list_column_reorder_drag: Option<crate::app::list_view_settings::ListColumnReorderDrag>,
    last_activation_click: Option<crate::model::LastActivationClick>,
    pub(crate) operation_queue: FileOperationQueue,
    pub(crate) operation_progress_animation_frame: u8,
    operation_history: FileOperationHistory,
    list_directory_summary_cache: crate::model::ListDirectorySummaryCache,
    user_preferences_save_in_flight: bool,
    pending_user_preferences_save: Option<config::UserPreferences>,
    pending_browser_session_save: bool,
    browser_session_saves_in_flight: usize,
    last_browser_session_save: Option<std::time::Instant>,
    scrollbar: ScrollbarState,
    smooth_scroll: MosScrollState,
    file_drag_edge_scroll: Option<file_drag_edge_scroll::FileDragEdgeScroll>,
    back_stack: Vec<PathBuf>,
    forward_stack: Vec<PathBuf>,
    next_tab_id: usize,
    next_pane_id: u64,
    next_icon_grid_expansion_session_id: u64,
    next_list_expansion_follow_session_id: u64,
    application_theme: ApplicationTheme,
    application_shutdown_phase: ApplicationShutdownPhase,
}

#[derive(Debug, Clone)]
struct PaneIconGridViewport {
    directory: PathBuf,
    viewport: IconGridViewport,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingKeyboardColumnFocus {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) directory: PathBuf,
    pub(crate) preferred_child: Option<PathBuf>,
}

impl FileBrowser {
    pub(crate) fn user_config(&self) -> &config::UserConfig {
        &self.user_config
    }

    pub(crate) fn active_language(&self) -> UiLanguage {
        self.user_config
            .language_setting
            .resolve(self.system_language)
    }

    pub(crate) fn active_appearance_mode(&self) -> crate::matugen_theme::AppearanceMode {
        self.application_theme
            .effective_mode(self.user_config.theme_mode)
    }

    pub(crate) fn theme_preview_colors(
        &self,
        color_scheme: crate::matugen_theme::ColorSchemePreset,
    ) -> [iced::Color; 3] {
        self.application_theme
            .preview_colors(self.user_config.theme_mode, color_scheme)
    }

    pub(crate) fn advance_window_animation_frame(&mut self) -> Task<Message> {
        self.preview_window_chrome.advance();
        self.preview_window_bottom_controls.advance();
        self.advance_terminal_panel_height_animation();
        self.advance_terminal_tab_shift_animations();
        Task::batch([
            self.advance_smooth_scroll_animation(),
            self.advance_scrollbar_animation(),
            self.advance_file_drag_edge_scroll(),
            self.advance_address_bar_transition(),
            self.advance_tab_bar_reveal_animation(),
            self.advance_tab_animations(),
            self.advance_list_directory_animations(),
            self.advance_icon_grid_expansion_animation(),
            self.advance_sidebar_bookmark_motion(),
        ])
    }

    fn refresh_current_language(&self) {
        localization::set_current_language(self.active_language());
    }

    pub(crate) fn file_operation_verification(&self) -> file_core::FileOperationVerification {
        self.user_config.file_operation_verification
    }

    pub(crate) fn remote_list_thumbnail_downloads_enabled(&self) -> bool {
        self.user_config.network_list_thumbnail_downloads_enabled
    }

    pub(crate) fn preview_file_size_limit_for(&self, path: &Path) -> u64 {
        // 调用点都位于预览门禁之后，分类必然成功；`None` 只可能来自
        // 门禁外的元数据查询，按文本上限处理是安全的兜底。
        let size_kind =
            crate::preview::classify_preview_path(path, &self.user_config.preview_extension_rules)
                .map(crate::preview::PreviewPathKind::file_size_kind)
                .unwrap_or(crate::config::PreviewFileSizeKind::Text);
        self.user_config.preview_size_limits.limit(size_kind)
    }

    pub(crate) fn preview_directory_expand_levels(&self) -> u8 {
        self.user_config.preview_directory_expand_levels
    }

    fn boot(
        application_launch_request: ApplicationLaunchRequest,
        file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
        initial_desktop_activation: Option<DesktopActivationEvent>,
        startup_rendering_environment: StartupRenderingEnvironment,
    ) -> (Self, Task<Message>) {
        let (main_window, open_main_window) = window::open(main_window_settings());
        let user_config = config::ui_thread_startup_config();
        let (browser, initial_tasks) = Self::new_with_main_window(
            user_config,
            main_window,
            application_launch_request,
            file_manager_activation,
            startup_rendering_environment,
        );

        let open_main_window = open_main_window.then(|window_id| {
            Task::batch([
                wayland_dnd_window_handle_command(window_id),
                x11_dnd_window_handle_command(window_id),
            ])
        });

        let initial_desktop_activation = initial_desktop_activation
            .map(Message::DesktopActivationReceived)
            .map(Task::done)
            .unwrap_or_else(Task::none);

        (
            browser,
            Task::batch([open_main_window, initial_tasks, initial_desktop_activation]),
        )
    }

    #[cfg(test)]
    pub(crate) fn new(user_config: config::UserConfig) -> (Self, Task<Message>) {
        Self::new_with_launch_request(user_config, ApplicationLaunchRequest::ConfiguredStartup)
    }

    #[cfg(test)]
    pub(crate) fn new_with_launch_request(
        user_config: config::UserConfig,
        application_launch_request: ApplicationLaunchRequest,
    ) -> (Self, Task<Message>) {
        Self::new_with_main_window(
            user_config,
            window::Id::unique(),
            application_launch_request,
            None,
            StartupRenderingEnvironment::fast_default(
                crate::startup_rendering::StartupRenderingBackend::Gl,
            ),
        )
    }

    fn new_with_main_window(
        user_config: config::UserConfig,
        main_window: window::Id,
        application_launch_request: ApplicationLaunchRequest,
        file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
        startup_rendering_environment: StartupRenderingEnvironment,
    ) -> (Self, Task<Message>) {
        startup_trace::mark_once("file_browser_new_started");
        let placeholder_dir = PathBuf::from("/");
        let options = ScanOptions::default();
        let initial_view_mode = user_config.browser_view_mode;
        let mut search_service = SearchServiceState::new();
        let initial_search_service_request = search_service.begin_initial_status_request();
        let mut initial_tab = BrowserTab::directory(0, placeholder_dir.clone());
        initial_tab.view_mode = initial_view_mode;
        let initial_pane = BrowserPane {
            id: BrowserPaneId::PRIMARY,
            current_dir: placeholder_dir.clone(),
            is_trash_view: false,
            entries: empty_directory_entry_snapshot(),
            directory_discovery: None,
            directory_loading_placeholder: None,
            trash_entries: Vec::new(),
            selected: None,
            selected_paths: HashSet::new(),
            selection_anchor: None,
            deepest_open_column_directory: None,
            expanded_directories: HashMap::new(),
            view_mode: initial_view_mode,
            column_browser_viewport: ColumnBrowserViewport::default(),
            column_viewports: HashMap::new(),
            tabs: vec![initial_tab.clone()],
            active_tab_id: initial_tab.id,
            directory_load_generation: 0,
            directory_load_cancel: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            directory_collection_phase: DirectoryCollectionPhase::Discovering,
            directory_order_phase: DirectoryOrderPhase::Ready {
                field: options.sort_field,
                direction: options.sort_direction,
            },
        };
        let mut browser = Self {
            home_dir: placeholder_dir.clone(),
            current_dir: placeholder_dir.clone(),
            is_trash_view: false,
            entries: empty_directory_entry_snapshot(),
            entry_index: None,
            directory_discovery: None,
            directory_loading_placeholder: None,
            trash_entries: Vec::new(),
            trash_refresh: TrashRefreshState::default(),
            trash_batch_rescan_pending: false,
            selected: None,
            selected_paths: HashSet::new(),
            hovered_entry: None,
            hovered_sidebar: None,
            hovered_sidebar_device: None,
            hovered_network_connection: None,
            cursor_paste_directory: None,
            preview: None,
            pending_document_preview: None,
            document_preview_generation: 0,
            remote_preview_download_cancel: None,
            text_preview_document: None,
            animated_image_preview_generation: 0,
            original_image_preview_generation: 0,
            original_image_preview_cancel: None,
            pending_original_image_preview: None,
            remote_preview_download_generation: 0,
            text_preview_generation: 0,
            directory_load_generation: 0,
            directory_load_cancel: None,
            next_directory_metadata_request_generation: 1,
            directory_metadata_in_flight: HashSet::new(),
            selection_metadata_signature: 0,
            audio_preview: None,
            sqlite_tables_resize_drag: None,
            sqlite_preview: None,
            sqlite_preview_generation: 0,
            video_preview: None,
            preview_size: default_preview_size(PreviewWindowProfile::Regular),
            text_preview_content_height: 0.0,
            pending_preview_resize: None,
            preview_window_profile: PreviewWindowProfile::Regular,
            preview_window_pinned: false,
            preview_shown_path: None,
            preview_window_chrome: PreviewWindowChromeState::default(),
            preview_window_bottom_controls: PreviewWindowChromeState::default(),
            preview_window_drag_active: false,
            preview_window_pointer_y: None,
            preview_image_viewport: ImagePreviewViewport::default(),
            preview_window_initial_chrome_generation: 0,
            main_window,
            maximized_windows: HashSet::new(),
            wayland_dnd: None,
            x11_dnd: None,
            file_manager_activation,
            preview_window: None,
            focused_window: main_window,
            system_focused_window: None,
            thumbnail_cache: ThumbnailCache::new(user_config.thumbnail_cache_dir.clone()),
            column_browser_viewport: ColumnBrowserViewport::default(),
            column_viewports: HashMap::new(),
            icon_grid_viewports: HashMap::new(),
            icon_grid_expansion: None,
            list_expansion_follow: None,
            context_menu: None,
            open_with: None,
            file_drop_prompt: None,
            archive_creation: None,
            convert: None,
            checksum: None,
            archive_extraction: None,
            batch_rename: None,
            sidebar_locations: Vec::new(),
            sidebar_devices: SidebarDeviceState::loading(),
            network_connections: NetworkConnectionState::default(),
            network_connection_editor: None,
            sidebar_bookmark_drop_slot: None,
            sidebar_bookmark_drag: None,
            sidebar_bookmark_motion: HashMap::new(),
            sidebar_width: user_config.sidebar_width,
            sidebar_resize_drag: None,
            right_preview_panel_open: user_config.right_preview_panel_open,
            right_preview_panel_width: config::normalize_right_preview_panel_width(
                user_config.right_preview_panel_width,
            ),
            right_preview_panel_resize_drag: None,
            right_preview_preview_ratio: config::normalize_right_preview_preview_ratio(
                user_config.right_preview_preview_ratio,
            ),
            right_preview_panel_info: None,
            right_preview_ratio_resize_drag: None,
            preview_load_surface: right_preview_panel::PreviewLoadSurface::StandaloneWindow,
            renaming: None,
            pending_created_entry_rename: None,
            pending_operation: None,
            transfer_conflict: None,
            destructive_action_confirmation: None,
            settings_window: None,
            properties_window: None,
            properties: None,
            properties_load_generation: 0,
            tabs: vec![initial_tab],
            active_tab_id: 0,
            tab_bar_reveal: TabBarReveal::default(),
            tab_animations: HashMap::new(),
            panes: vec![initial_pane],
            pane_layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
            tab_drag: None,
            pane_drag: None,
            pane_drag_pointer_press: None,
            split_resize_drag: None,
            selection_marquee: None,
            file_drag: None,
            file_drop_session: None,
            next_file_drag_gesture_id: 0,
            file_drop_layout_generation: 0,
            file_entry_bounds: Vec::new(),
            breadcrumb_drop_target_bounds: Vec::new(),
            breadcrumb_drop_target_measurement_generation: 0,
            options: options.clone(),
            application_launch_request,
            user_config: user_config.clone(),
            preview_size_limit_mib_inputs: config::preview_size_limit_mib_inputs(
                &user_config.preview_size_limits,
            ),
            preview_size_limit_mib_errors: [const { None }; 7],
            preview_extension_inputs: [const { String::new() }; 7],
            preview_extension_input_errors: [const { None }; 7],
            preview_extension_expanded: [false; 7],
            preview_extension_reset_confirmation: None,
            preview_directory_expand_levels_input: user_config
                .preview_directory_expand_levels
                .to_string(),
            preview_directory_expand_levels_error: None,
            startup_custom_directory_input: user_config
                .startup_custom_directory
                .to_string_lossy()
                .into_owned(),
            startup_custom_directory_error: None,
            pending_startup_directory_validation: None,
            startup_directory_validation_generation: 0,
            rendering_gpu_preference: user_config.rendering_gpu_preference,
            renderer_restart_notice_visible: false,
            startup_rendering_environment,
            pending_renderer_restart_environment: None,
            address_editing: None,
            address_bar_transition: None,
            next_address_editing_session_id: 1,
            column_width_overrides: HashMap::new(),
            column_width_reference_content_widths: HashMap::new(),
            terminal_emulator: user_config.terminal_emulator,
            terminal_panel: crate::terminal_panel::TerminalPanelState::new(),
            selected_settings_category: SettingsCategory::General,
            context_menu_settings_page: ContextMenuSettingsPage::FileEntry,
            context_menu_reset_confirmation: None,
            context_menu_settings_drag: None,
            settings_subpage: None,
            expanded_color_scheme_family: None,
            custom_color_scheme_import_error: None,
            search_service,
            search_workspace: None,
            search_history_interaction: crate::model::SearchHistoryInteraction::default(),
            next_search_workspace_session_id: 1,
            deepest_open_column_directory: None,
            focused_column_directory: None,
            last_pointer_clicked_column_directory: None,
            expanded_directories: HashMap::new(),
            view_mode: initial_view_mode,
            rename_input: String::new(),
            rename_input_history: file_operations::RenameInputHistory::default(),
            directory_collection_phase: DirectoryCollectionPhase::Discovering,
            directory_order_phase: DirectoryOrderPhase::Ready {
                field: options.sort_field,
                direction: options.sort_direction,
            },
            global_error_notification: None,
            next_global_error_notification_generation: 0,
            application_logs: ApplicationLogViewState::new(
                crate::runtime_logging::journald_initialization_warning(),
            ),
            system_language: UiLanguage::English,
            cursor_position: Point::new(0.0, 0.0),
            main_window_width: MAIN_WINDOW_INITIAL_WIDTH,
            main_window_height: MAIN_WINDOW_INITIAL_HEIGHT,
            is_cursor_over_column_browser: false,
            hovered_pane_id: None,
            hovered_list_header_column: None,
            keyboard_modifiers: keyboard::Modifiers::default(),
            shortcut_capture: None,
            selection_anchor: None,
            drag_selection_anchor: None,
            column_return_targets: HashMap::new(),
            pending_keyboard_column_focus: None,
            pending_view_switch_reveal: None,
            column_resize_drag: None,
            list_column_resize_drag: None,
            list_column_reorder_drag: None,
            last_activation_click: None,
            operation_queue: FileOperationQueue::new(),
            operation_progress_animation_frame: 0,
            operation_history: FileOperationHistory::new(),
            list_directory_summary_cache: crate::model::ListDirectorySummaryCache::default(),
            user_preferences_save_in_flight: false,
            pending_user_preferences_save: None,
            pending_browser_session_save: false,
            browser_session_saves_in_flight: 0,
            last_browser_session_save: None,
            scrollbar: ScrollbarState::default(),
            smooth_scroll: MosScrollState::default(),
            file_drag_edge_scroll: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            next_tab_id: 1,
            next_pane_id: 1,
            next_icon_grid_expansion_session_id: 1,
            next_list_expansion_follow_session_id: 1,
            application_theme: ApplicationTheme::new(fallback_theme(AppearanceMode::Light)),
            application_shutdown_phase: ApplicationShutdownPhase::Running,
        };
        browser
            .application_theme
            .replace_custom_color_scheme(user_config.custom_color_scheme);
        browser.refresh_current_language();
        browser.refresh_column_width_reference_content_widths();
        startup_trace::mark_once("file_browser_new_ready");
        let startup_rendering_environment = browser.startup_rendering_environment.clone();
        (
            browser,
            Task::batch([
                startup_environment_command(startup_rendering_environment),
                system_theme_command(),
                ensure_search_service_command(initial_search_service_request),
            ]),
        )
    }

    fn title(&self, window: window::Id) -> String {
        self.window_title(window)
    }

    fn subscription(&self) -> Subscription<Message> {
        if !self.application_shutdown_phase.is_running() {
            if self.application_shutdown_phase.is_draining() {
                let mut subscriptions = vec![event::listen_with(global_event_message)];
                if let Some(operation_subscription) = self
                    .operation_queue
                    .active_subscription()
                    .map(file_operation_subscription)
                {
                    subscriptions.push(operation_subscription);
                }
                return Subscription::batch(subscriptions);
            }
            return Subscription::none();
        }

        let mut subscriptions = vec![event::listen_with(global_event_message)];
        if let Some((generation, remaining)) = self.global_error_notification_countdown() {
            subscriptions.push(
                time::every(remaining)
                    .with(generation)
                    .map(|(generation, _)| Message::GlobalErrorNotificationElapsed(generation)),
            );
        }
        subscriptions.push(sidebar_device_refresh_subscription());
        if let Some(path) = config::matugen_theme_file_path() {
            subscriptions.push(matugen_theme_subscription(path));
        }
        if let Some(runtime) = &self.file_manager_activation {
            subscriptions.push(desktop_activation_subscription(Arc::clone(runtime)));
        }
        if let Some(runtime) = &self.wayland_dnd {
            subscriptions.push(wayland_file_dnd_subscription(
                runtime.window_handle,
                runtime.controller.clone(),
            ));
        }
        if let Some(runtime) = &self.x11_dnd {
            subscriptions.push(x11_file_dnd_subscription(
                runtime.window_handle,
                runtime.controller.clone(),
            ));
        }

        if !self.directory_collection_phase.is_discovering() && !self.is_trash_view {
            let watched_directories = std::iter::once(self.current_dir.clone())
                .chain(self.expanded_directories.keys().cloned())
                .chain(self.icon_grid_expansion_watch_directories())
                .collect::<HashSet<_>>();
            subscriptions.extend(
                watched_directories
                    .into_iter()
                    .map(directory_watch_subscription),
            );
        }

        if self.settings_window.is_some()
            && self.selected_settings_category == crate::model::SettingsCategory::Search
        {
            subscriptions.push(
                time::every(SEARCH_SERVICE_STATUS_REFRESH_INTERVAL)
                    .map(|_| Message::SearchServiceStatusRefreshRequested),
            );
        }

        if self.settings_window.is_some()
            && self.selected_settings_category == crate::model::SettingsCategory::Logs
        {
            subscriptions.push(
                time::every(APPLICATION_LOG_REFRESH_INTERVAL)
                    .map(|_| Message::ApplicationLogsRefreshRequested),
            );
        }

        if self.has_trash_tab() {
            subscriptions.extend(
                file_core::trash_bin::trash_watch_directories()
                    .into_iter()
                    .map(directory_watch_subscription),
            );
        }

        if !self.network_connections.entries.is_empty() {
            subscriptions.push(time::every(NETWORK_STATUS_REFRESH_INTERVAL).map(|_| {
                Message::NetworkConnection(
                    crate::network_connections::NetworkConnectionMessage::StatusRefreshRequested,
                )
            }));
        }

        if let Some(operation) = self.operation_queue.active_subscription() {
            subscriptions.push(file_operation_subscription(operation));
        }

        subscriptions.push(self.terminal_output_subscription());

        let remote_preview_progress_is_indeterminate = matches!(
            self.preview.as_ref(),
            Some(PreviewState::DownloadingRemoteFile(download)) if download.fraction().is_none()
        );
        if self.operation_queue.has_active_indeterminate_progress()
            || remote_preview_progress_is_indeterminate
        {
            subscriptions.push(
                time::every(OPERATION_PROGRESS_ANIMATION_INTERVAL)
                    .map(|_| Message::OperationProgressAnimationTick),
            );
        }

        if self.preview_tree_animation_is_active() {
            subscriptions.push(
                time::every(PREVIEW_TREE_ANIMATION_INTERVAL)
                    .map(|_| Message::PreviewTreeAnimationTick),
            );
        }

        if self.preview_window_chrome.is_animating()
            || self.preview_window_bottom_controls.is_animating()
            || self.scrollbar_animation_is_active()
            || self.smooth_scroll_animation_is_active()
            || self.file_drag_edge_scroll_is_active()
            || self.address_bar_transition_is_active()
            || self.tab_bar_reveal_animation_is_active()
            || self.tab_animation_is_active()
            || self.list_directory_animation_is_active()
            || self.icon_grid_expansion_animation_is_active()
            || self.sidebar_bookmark_motion_is_active()
            || self.terminal_panel.is_animating()
        {
            subscriptions.push(
                time::every(SCROLLBAR_ANIMATION_INTERVAL)
                    .map(|_| Message::WindowChromeAnimationTick),
            );
        }

        if self.audio_preview_is_active() {
            subscriptions
                .push(time::every(AUDIO_PREVIEW_TICK_INTERVAL).map(|_| Message::AudioPreviewTick));
        }

        if self.video_preview_is_active() {
            subscriptions
                .push(time::every(AUDIO_PREVIEW_TICK_INTERVAL).map(|_| Message::VideoPreviewTick));
        }

        if let Some((path, generation, position)) = self.active_animated_image_preview_stream() {
            subscriptions.push(animated_image_preview_subscription(
                path, generation, position,
            ));
        }

        if let Some((path, generation, position)) = self.active_video_preview_stream() {
            subscriptions.push(video_preview_subscription(path, generation, position));
        }

        Subscription::batch(subscriptions)
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.application_theme
            .active(self.user_config.theme_mode, self.user_config.color_scheme)
    }

    fn view_with_window_chrome<'a>(
        &'a self,
        window: window::Id,
        integrated_title: &'static str,
        content: Element<'a, Message>,
        preview_pin: Option<bool>,
    ) -> Element<'a, Message> {
        let frame_state = self.window_frame_state(window);
        let content = auxiliary_window_content(
            integrated_title,
            self.window_title(window),
            content,
            &self.user_config.window_controls,
            window,
            frame_state,
            preview_pin,
        );
        window_resize_frame(content, window, frame_state)
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if self.settings_window == Some(window) {
            view_settings_window(self, window)
        } else if self.properties_window == Some(window) {
            let content = view_properties_window(
                self.properties.as_ref(),
                self.scrollbar_visibility_for(&ScrollbarRegion::Properties),
                self.scrollbar_viewport_for(&ScrollbarRegion::Properties),
            );
            self.view_with_window_chrome(window, "Properties", content, None)
        } else if self.preview_window == Some(window) {
            let content =
                self.preview_window_content(self.preview_window_bottom_controls.opacity());
            if self.preview_window_uses_window_chrome() {
                self.view_with_window_chrome(
                    window,
                    "Preview",
                    content,
                    Some(self.preview_window_pinned),
                )
            } else {
                let frame_state = self.window_frame_state(window);
                let content = floating_preview_window_content(
                    content,
                    &self.user_config.window_controls,
                    window,
                    frame_state,
                    self.preview_window_chrome.opacity(),
                    self.preview_window_pinned,
                );
                window_resize_frame(content, window, frame_state)
            }
        } else if window == self.main_window {
            startup_trace::mark_once("first_main_window_view");
            if !self.directory_collection_phase.is_discovering()
                && self.directory_order_phase.is_ready()
            {
                startup_trace::mark_once("first_browser_view_after_initial_load");
            }
            view_browser(self)
        } else {
            iced::widget::container(crate::typography::readable_text("Closing window..."))
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .into()
        }
    }
}
