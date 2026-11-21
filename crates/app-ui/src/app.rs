pub(crate) mod actions;
mod advanced_new_folder;
mod application_logs;
mod application_shutdown;
pub(crate) mod archive_creation;
pub(crate) mod archive_extraction;
pub(crate) mod archive_member_password;
#[cfg(test)]
mod archive_member_password_tests;
pub(crate) mod archive_password;
mod auxiliary_window;
mod batch_rename;
pub(crate) mod checksum;
mod column_resize;
mod column_scroll;
mod config_persistence;
mod context_menu_keys;
mod context_menu_settings;
pub(crate) mod convert;
mod desktop_activation;
mod directory_expansion_loading;
mod directory_metadata_demand;
mod directory_metadata_in_flight;
mod directory_recovery;
mod displayed_directory_totals;
mod duplicate_selection;
mod entry_changes;
mod entry_hover_recalc;
mod events;
mod file_drag_edge_scroll;
mod file_grouping_rail_scroll;
mod file_operation_notifications;
mod file_operations;
pub(crate) mod font_settings;
mod global_error;
mod icon_grid_expansion;
#[cfg(test)]
mod icon_grid_file_operation_tests;
#[cfg(test)]
mod icon_grid_invariant_tests;
mod list_directory_summaries;
#[cfg(test)]
mod list_row_cache_tests;
#[cfg(test)]
mod list_size_sorting_tests;
mod list_view_settings;
mod navigation;
mod network_connections;
mod network_settings;
mod open_with;
mod pane_drag;
#[cfg(test)]
mod pane_lifecycle_tests;
pub(crate) mod panes;
mod path_suggestions;
mod paths;
mod persistence_feedback;
mod pointer_interactions;
mod preferences_sync;
mod preview_settings;
pub(crate) mod preview_state;
mod preview_window_view;
mod properties;
mod remote_mounts;
mod rendering_settings;
mod revealed_path;
#[cfg(test)]
mod revealed_path_tests;
pub(crate) mod right_preview_panel;
mod runtime;
mod search;
mod search_location;
mod search_paths;
mod selection;
mod session_persistence;
mod session_restore;
mod shortcut_commands;
mod shortcut_help;
mod shortcuts;
mod sidebar_bookmarks;
mod sidebar_devices;
mod sidebar_resize;
mod smart_extract;
mod smart_folders;
pub(crate) mod smooth_scroll;
mod split_commands;
mod split_resize;
mod startup;
mod startup_settings;
mod super_box;
mod super_box_complete;
#[cfg(test)]
mod super_box_extras_tests;
mod super_box_find;
mod super_box_pipe;
mod super_box_shell;
mod tabs;
#[cfg(test)]
mod task_driving_tests;
mod task_end;
#[cfg(test)]
mod terminal_follow_tests;
#[cfg(test)]
mod terminal_keys_tests;
mod text_input_shortcuts;
mod thumbnailing;
pub(crate) mod transfer;
pub(crate) mod transfer_service;
mod trash;
#[cfg(test)]
mod trash_tests;
mod update;
mod video_fullscreen;
mod view_modes;
mod vim_keys;
mod wayland_dnd;
mod window_chrome;
mod window_control_settings;
mod windows;
mod x11_dnd;

use auxiliary_window::AuxiliaryWindow;
pub(crate) use runtime::run;
pub(crate) use super_box::{super_box_input_id, SuperBoxMessage};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationEvent, DesktopActivationRuntime, NetworkConnectionId, StorageDeviceId,
    TerminalEmulator,
};
use file_core::ScanOptions;
use iced::event;
use iced::keyboard;
use iced::window;
use iced::{time, Element, Point, Subscription, Task, Theme};

// 媒体预览流式订阅（自本 crate 订阅桥下沉 bennu-preview 后直连）：
// 输出 PreviewMessage，经 Message::Preview 包装回流引擎。
use bennu_preview::animated_image_preview::animated_image_preview_subscription;
use bennu_preview::engine::video_fullscreen::VideoFullscreen;
use bennu_preview::engine::{audio_preview_tick_subscription, video_preview_tick_subscription};
use bennu_preview::preview::PreviewSize;
use bennu_preview::video_preview::video_preview_subscription;
use bennu_preview::window_shell::HEADER_BAR_HEIGHT;

use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
use bennu_preview::preview_message::PreviewMessage;
use bennu_theme::{AppearanceMode, ApplicationTheme};

use crate::app::application_shutdown::ApplicationShutdownPhase;
use crate::app::archive_creation::ArchiveCreationState;
use crate::app::archive_extraction::ArchiveExtractionState;
use crate::app::checksum::ChecksumState;
use crate::app::column_resize::ColumnResizeDrag;
use crate::app::events::global_event_message;
use crate::app::runtime::{
    desktop_activation_subscription, directory_watch_subscription, foreign_task_wait_subscription,
    matugen_colors_subscription, properties_watch_subscription, state_database_subscription,
    storage_change_subscription, system_mode_subscription, wayland_file_dnd_subscription,
    x11_file_dnd_subscription,
};
use crate::app::sidebar_bookmarks::SidebarBookmarkMotionState;
use crate::app::sidebar_resize::SidebarResizeDrag;
use crate::app::split_resize::SplitResizeDrag;
use crate::app::tabs::{TabAnimationState, TabBarReveal};
use crate::app::windows::{
    default_preview_size, main_window_settings, MAIN_WINDOW_INITIAL_HEIGHT,
    MAIN_WINDOW_INITIAL_WIDTH,
};
use crate::command_line::ApplicationLaunchRequest;
use crate::commands::{
    ensure_search_service_command, startup_environment_command, wayland_dnd_window_handle_command,
    x11_dnd_window_handle_command,
};
use crate::config;
use crate::config::UiLanguage;
use crate::model::ConvertState;
use crate::model::{
    AdvancedNewFolderState, ApplicationLogViewState, BatchRenameState, BreadcrumbDropTargetBounds,
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, ColumnEntryBounds,
    ContextMenuSettingsDragState, ContextMenuSettingsPage, ContextMenuState, CreatedEntryAfter,
    DestructiveActionConfirmation, DirectoryOrderPhase, FileAreaMenuItem, FileDragSpringHover,
    FileDragState, FileDropPrompt, FileDropSessionState, FilePropertiesState,
    IconGridExpansionState, IconGridViewport, ListColumnKind, Message, PaneAddressBarTransition,
    PaneAddressEditingSession, PaneDragPointerPress, PaneDragState, PendingOperation, PreviewState,
    PreviewWindowProfile, SearchServiceState, SelectionMarquee, SettingsCategory, SettingsSubpage,
    SidebarBookmarkDragState, SidebarBookmarkDropSlot, SidebarLocation,
    StartupDirectoryValidationRequest, TabDragState, TransferConflictState, TrashRefreshState,
};
use crate::network_connections::{NetworkConnectionEditorState, NetworkConnectionState};
use crate::open_with::OpenWithState;
use crate::operation_history::FileOperationHistory;
use crate::shortcuts::ShortcutCaptureState;
use crate::sidebar_devices::SidebarDeviceState;
use crate::startup_rendering::StartupRenderingEnvironment;
use crate::startup_trace;
use crate::thumbnail_cache::ThumbnailCache;
use crate::view::{view_browser, view_properties_window, view_settings_window, window_shell};
use ::file_operations::FileOperationQueue;
use bennu_preview::preview_window_chrome::preview_pin_button;
use bennu_preview::window_shell::{single_window, HeaderBar, WindowShell};
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);
const POINTER_DRAG_ACTIVATION_DISTANCE: f32 = 3.0;
const PREVIEW_TREE_ANIMATION_INTERVAL: Duration = bennu_theme::animation::FRAME_INTERVAL_60HZ;
const OPERATION_PROGRESS_ANIMATION_INTERVAL: Duration = Duration::from_millis(80);
const SEARCH_SERVICE_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const APPLICATION_LOG_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct FileBrowser {
    pub(crate) home_dir: PathBuf,
    pub(crate) trash_refresh: TrashRefreshState,
    /// 回收站批量任务运行期间,watcher 触发的回收站重扫被挂起,只置此标记;
    /// 批量任务终结时统一补刷一次,避免逐条全量重扫的平方级风暴。
    trash_batch_rescan_pending: bool,
    pub(crate) hovered_entry: Option<PathBuf>,
    pub(crate) hovered_sidebar: Option<PathBuf>,
    /// 侧栏的回收站行不是路径，悬停单独记。
    pub(crate) sidebar_trash_hovered: bool,
    pub(crate) hovered_sidebar_device: Option<StorageDeviceId>,
    pub(crate) hovered_network_connection: Option<NetworkConnectionId>,
    cursor_paste_directory: Option<PathBuf>,
    /// 预览状态机聚合体（bennu-preview）：预览会话、窗口与媒体播放状态都在
    /// 引擎里，宿主一律经本字段显式访问。仍留宿主的预览相关字段是
    /// `right_preview_panel_*` 四个（见结构体后段，类型在 right_preview_panel
    /// 视图模块）。
    pub(crate) preview_engine: bennu_preview::engine::PreviewEngine,
    next_directory_metadata_request_generation: u64,
    directory_metadata_in_flight: directory_metadata_in_flight::DirectoryMetadataInFlight,
    /// 补文件信息的结果每到一次加一。信息是后台写进条目里的一次性格子，条目快照不变，
    /// 按大小、日期分组的列表行流靠它知道要重排。
    pub(crate) entry_metadata_revision: u64,
    main_window: window::Id,
    maximized_windows: HashSet<window::Id>,
    wayland_dnd: Option<wayland_dnd::WaylandDndRuntime>,
    x11_dnd: Option<x11_dnd::X11DndRuntime>,
    file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
    focused_window: window::Id,
    system_focused_window: Option<window::Id>,
    pub(crate) thumbnail_cache: ThumbnailCache,
    icon_grid_viewports: HashMap<BrowserPaneId, PaneIconGridViewport>,
    pub(crate) icon_grid_expansion: Option<IconGridExpansionState>,
    list_expansion_follow: Option<view_modes::ListExpansionFollowPlan>,
    pub(crate) context_menu: Option<ContextMenuState>,
    pub(crate) open_with: Option<OpenWithState>,
    /// 超级输入框（Alt+Space）。可关闭的阻断浮层，同 `open_with`。
    pub(crate) super_box: Option<crate::super_box::SuperBox>,
    /// 框发出的命令和它们最后一次的结果；不跟着框关掉。
    pub(crate) super_box_commands: super_box_shell::SuperBoxCommands,
    pub(crate) file_drop_prompt: Option<FileDropPrompt>,
    pub(crate) archive_creation: Option<ArchiveCreationState>,
    pub(crate) convert: Option<ConvertState>,
    pub(crate) checksum: Option<ChecksumState>,
    pub(crate) archive_extraction: Option<ArchiveExtractionState>,
    pub(crate) archive_member_password: Option<archive_member_password::ArchiveMemberPasswordState>,
    pub(crate) batch_rename: Option<BatchRenameState>,
    pub(crate) advanced_new_folder: Option<AdvancedNewFolderState>,
    /// 确认时选定的收尾动作(创建后进入/新标签进入),完成边界消费一次。
    pending_created_entry_after: Option<CreatedEntryAfter>,
    pub(crate) sidebar_locations: Vec<SidebarLocation>,
    pub(crate) sidebar_devices: SidebarDeviceState,
    pub(crate) network_connections: NetworkConnectionState,
    pub(crate) network_connection_editor: Option<NetworkConnectionEditorState>,
    pub(crate) sidebar_bookmark_drop_slot: Option<SidebarBookmarkDropSlot>,
    pub(crate) sidebar_bookmark_drag: Option<SidebarBookmarkDragState>,
    pub(crate) sidebar_bookmark_motion: HashMap<PathBuf, SidebarBookmarkMotionState>,
    /// 侧栏显示时的宽度（用户拖出来的，存盘的也是它）。布局和命中区域一律用
    /// `sidebar_layout_width()`：侧栏隐藏时那里是 0。
    pub(crate) sidebar_width_when_shown: f32,
    sidebar_resize_drag: Option<SidebarResizeDrag>,
    pub(crate) right_preview_panel_open: bool,
    pub(crate) right_preview_panel_width: f32,
    right_preview_panel_resize_drag: Option<right_preview_panel::RightPreviewPanelResizeDrag>,
    pub(crate) right_preview_preview_ratio: f32,
    pub(crate) right_preview_panel_info: Option<crate::model::RightPreviewPanelInfoSnapshot>,
    right_preview_ratio_resize_drag: Option<right_preview_panel::RightPreviewPanelRatioDrag>,
    pub(crate) renaming: Option<PathBuf>,
    pending_created_entry_rename: Option<PathBuf>,
    pub(crate) pending_operation: Option<PendingOperation>,
    pub(crate) transfer_conflict: Option<TransferConflictState>,
    pub(crate) destructive_action_confirmation: Option<DestructiveActionConfirmation>,
    settings_window: Option<window::Id>,
    properties_window: Option<window::Id>,
    pub(crate) properties: Option<FilePropertiesState>,
    properties_load_generation: u64,
    /// 发送到手机辅助窗口（bennu-transfer）。
    pub(crate) transfer_window: Option<window::Id>,
    /// LocalSend 收发任务（进任务面板）；设备行显示每台设备最新的那次发送。
    pub(crate) transfer_tasks: transfer::TransferTasks,
    /// 两个传输窗口各自勾选的设备；关哪个窗口清哪份。
    pub(crate) checked_devices: transfer::CheckedDevices,
    /// LocalSend 窗口（bennu-localsend）与它的待发列表、"按 IP 添加"表单；关窗即清空。
    pub(crate) local_send_window: Option<window::Id>,
    pub(crate) local_send_staged: Vec<transfer::StagedItem>,
    pub(crate) favorite_address_form: Option<transfer::FavoriteAddressForm>,
    /// LocalSend 窗口停在"设备"还是"记录"标签（关窗不重置：下次打开还在原来那页）。
    pub(crate) local_send_page: transfer::LocalSendPage,
    /// LocalSend 传输记录（新的在前，启动时从状态库读入）。
    pub(crate) transfer_history: transfer::TransferHistory,
    /// 通过链接接收（本机卡片里的二维码）。
    pub(crate) link_receiving: transfer::LinkReceiving,
    /// 待发卡片停在"文件"还是"文字"标签，与要发的文字。
    pub(crate) local_send_tab: transfer::StagedTab,
    pub(crate) local_send_text: iced::widget::text_editor::Content,
    /// 设置页收藏设备改名的输入（证书指纹 → 正在输入的名字），回车提交后清掉。
    pub(crate) favorite_alias_inputs: HashMap<String, String>,
    /// 常驻 LocalSend 服务运行时；None = 未启动或启动失败。
    pub(crate) transfer_runtime: Option<transfer::LocalSendRuntime>,
    /// 这个进程负不负责 LocalSend 接收（同一用户只有一个 Bennu 进程收）。
    pub(crate) transfer_receiver_role: transfer::ReceiverRole,
    /// 二维码下载会话；与 transfer_window 同生命周期。
    pub(crate) transfer_session: Option<transfer::TransferSessionState>,
    /// 收到的文字，排队一次弹一条（队首在显示）。
    pub(crate) incoming_messages: std::collections::VecDeque<transfer::IncomingMessage>,
    /// 接收确认 Modal 状态（不是收藏的设备发来文件）。
    pub(crate) incoming_transfer: Option<transfer::IncomingTransferConfirmation>,
    /// 设置页设备名输入缓冲。
    pub(crate) transfer_device_alias_input: String,
    /// 设置页端口输入缓冲；启动时与已保存端口同步。
    pub(crate) transfer_port_input: String,
    /// 上一次提交的端口不合法（配置未改）。
    pub(crate) transfer_port_invalid: bool,
    /// 设置页接收目录输入缓冲；空 = 用默认的 ~/Downloads。
    pub(crate) transfer_download_dir_input: String,
    /// 上一次提交的接收目录不是绝对路径（配置未改）。
    pub(crate) transfer_download_dir_invalid: bool,
    /// 设置页 PIN 输入；None = "接收需要 PIN"关着（此时配置里也没有 PIN），启动时与已存 PIN 同步。
    pub(crate) transfer_pin_input: Option<String>,
    /// 上一次保存的 PIN 是空的（配置未改）。
    pub(crate) transfer_pin_invalid: bool,
    tab_bar_reveal: TabBarReveal,
    pub(crate) tab_animations: HashMap<usize, TabAnimationState>,
    /// 全部窗格；浏览状态只存在这里（窗格→标签），活动窗格由 `pane_layout`
    /// 指向，没有第二份工作副本。
    pub(crate) panes: Vec<BrowserPane>,
    pub(crate) pane_layout: BrowserPaneLayout,
    tab_drag: Option<TabDragState>,
    pane_drag: Option<PaneDragState>,
    pane_drag_pointer_press: Option<PaneDragPointerPress>,
    split_resize_drag: Option<SplitResizeDrag>,
    pub(crate) selection_marquee: Option<SelectionMarquee>,
    pub(crate) file_drag: Option<FileDragState>,
    /// 拖拽源所在的列表滚动可视区,最近一次测量快照。
    pub(crate) file_drag_viewport: Option<iced::Rectangle>,
    pub(crate) file_drop_session: Option<FileDropSessionState>,
    /// 拖拽悬停目录自动打开(spring)候选:spring_open 模块收敛。
    pub(crate) file_drag_spring_hover: Option<FileDragSpringHover>,
    /// 拖拽会话存续期间条目内容变化(spring 导航加载完成/watcher 增量/
    /// 元数据落地)后落点布局过期;由 spring tick 消化重测。
    file_drop_layout_stale: bool,
    next_file_drag_gesture_id: u64,
    file_drop_layout_generation: u64,
    file_entry_bounds: Vec<ColumnEntryBounds>,
    breadcrumb_drop_target_bounds: Vec<BreadcrumbDropTargetBounds>,
    breadcrumb_drop_target_measurement_generation: u64,
    pub(crate) options: ScanOptions,
    application_launch_request: ApplicationLaunchRequest,
    user_config: config::UserConfig,
    pub(crate) preview_size_limit_inputs: [String; config::PreviewSizeLimitKind::ALL.len()],
    pub(crate) preview_size_limit_errors: [Option<String>; config::PreviewSizeLimitKind::ALL.len()],
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
    /// 改了要重启才生效的设置（渲染显卡、字体）之后的提示。
    pub(crate) restart_notice_visible: bool,
    pub(crate) installed_fonts: font_settings::InstalledFontList,
    startup_rendering_environment: StartupRenderingEnvironment,
    pub(super) pending_renderer_restart_environment:
        Option<crate::startup_rendering::StartupRenderingEnvironment>,
    pub(crate) address_editing: Option<PaneAddressEditingSession>,
    pub(crate) address_bar_transition: Option<PaneAddressBarTransition>,
    next_address_editing_session_id: u64,
    pub(crate) column_width_overrides: HashMap<usize, f32>,
    /// 所有栏宽 override 共享的参考内容宽度:override 值语义上是"参考宽度下的宽度",
    /// 读取时按 参考宽→当前内容宽 等比例换算。持久化/恢复/拖拽/模式切换都走同一换算。
    column_width_reference_content_width: f32,
    pub(crate) terminal_emulator: TerminalEmulator,
    pub(crate) terminal_panel: crate::terminal_panel::TerminalPanelState,
    pub(crate) selected_settings_category: SettingsCategory,
    pub(crate) context_menu_settings_page: ContextMenuSettingsPage,
    pub(crate) context_menu_reset_confirmation: Option<ContextMenuSettingsPage>,
    pub(crate) context_menu_settings_drag: Option<ContextMenuSettingsDragState>,
    /// 设置页文件条目预览中悬停展开的组锚点;None = 无展开。
    pub(crate) context_menu_preview_expansion: Option<FileAreaMenuItem>,
    pub(crate) settings_subpage: Option<SettingsSubpage>,
    pub(crate) search_service: SearchServiceState,
    pub(crate) search_history_interaction: crate::model::SearchHistoryInteraction,
    /// 搜索筛选栏上开着的下拉（一次只开一个）。
    pub(crate) search_filter_menu: Option<crate::model::SearchFilterMenu>,
    next_search_workspace_session_id: u64,
    focused_column_directory: Option<PathBuf>,
    last_pointer_clicked_column_directory: Option<PathBuf>,
    pub(crate) rename_input: String,
    rename_input_history: file_operations::RenameInputHistory,
    /// 执行端拒绝本次重命名的原因。只在 `begin_rename` 开始会话、输入变化、
    /// 被拒重开这三处改写，读取经 `rename_hint` 且要求会话仍在进行，
    /// 所以不会把上一次会话的原因带到新会话。
    rename_rejection: Option<::file_operations::RenameRejection>,
    global_error_notification: Option<global_error::GlobalErrorNotification>,
    next_global_error_notification_generation: u64,
    pub(crate) application_logs: ApplicationLogViewState,
    system_language: UiLanguage,
    /// 最近一次指针事件给出的窗口坐标（主窗口、预览窗口根部的指针记录器静默写入）；
    /// `None` = 还没有收到过任何指针事件。读取方需要"光标确实在某处"的事实时
    /// （hover 重算）必须判 `None`；只在指针手势里取起点的读取方经 [`Self::cursor_or_origin`]。
    pub(crate) cursor_position: bennu_theme::pointer_tracker::PointerPosition,
    /// 按住左键的那个窗口：按住期间该窗口的指针移动才作为消息进来（拖动、改大小、框选都要按住）。
    pointer_held_window: Option<window::Id>,
    pub(crate) main_window_width: f32,
    pub(crate) main_window_height: f32,
    /// 主窗口现在是不是在全屏放右侧面板的视频（宿主已切过去的状态，跟着引擎收敛）。
    main_window_video_fullscreen: bool,
    /// 光标当前压着的浏览内容区(窗格内容不含 tab 条)。悬停不激活窗格,
    /// 该记录只用于 Ctrl+Shift 分栏拖拽在被捕获控件上仍可判定起点。
    cursor_over_browser_content_pane: Option<BrowserPaneId>,
    hovered_pane_id: Option<BrowserPaneId>,
    hovered_list_header_column: Option<(BrowserPaneId, ListColumnKind)>,
    /// 光标最后进入的分组索引栏:栏是事件屏障(除滚轮),滚动补偿重算
    /// 必须知道"光标正压在栏上",否则会把栏下层的行误判成 hover。
    /// 栏不可见时由重算路径自愈清除,不逐个卸载点排查。
    hovered_grouping_rail_pane: Option<BrowserPaneId>,
    pub(crate) keyboard_modifiers: keyboard::Modifiers,
    pub(crate) shortcut_capture: Option<ShortcutCaptureState>,
    drag_selection_anchor: Option<PathBuf>,
    column_return_targets: HashMap<PathBuf, PathBuf>,
    pending_keyboard_column_focus: Option<PendingKeyboardColumnFocus>,
    pending_selection_reveal: Option<(BrowserPaneId, PathBuf)>,
    column_resize_drag: Option<ColumnResizeDrag>,
    list_column_resize_drag: Option<crate::app::list_view_settings::ListColumnResizeDrag>,
    list_column_reorder_drag: Option<crate::app::list_view_settings::ListColumnReorderDrag>,
    last_activation_click: Option<crate::model::LastActivationClick>,
    pub(crate) operation_queue: FileOperationQueue,
    pub(crate) operation_progress_animation_frame: u8,
    operation_history: FileOperationHistory,
    /// 动作文件夹最近一次读出来的内容；第一次读完之前是空的。
    pub(crate) action_library: crate::actions::ActionLibrary,
    pub(crate) action_runs: actions::ActionRuns,
    pub(crate) rules: actions::Rules,
    pub(crate) action_hint: Option<actions::ActionHint>,
    list_directory_summary_cache: crate::model::ListDirectorySummaryCache,
    user_preferences_save_in_flight: bool,
    pending_user_preferences_save: Option<config::UserPreferences>,
    /// 这个进程上次从库里读到或写下的设置：和它不一样的才是这个进程改的（见
    /// `preferences_sync`）。
    user_preferences_seen: config::UserPreferences,
    /// 同上，对 config.toml。
    app_config_seen: config::AppConfig,
    /// 启动时设置读不了或原数据没能备份时为 Blocked：写回会覆盖用户原来的设置。
    user_preferences_persistence: config::SettingsPersistence,
    /// 同上，对 config.toml。
    app_config_persistence: config::SettingsPersistence,
    pending_browser_session_save: bool,
    browser_session_saves_in_flight: usize,
    last_browser_session_save: Option<std::time::Instant>,
    file_drag_edge_scroll: Option<file_drag_edge_scroll::FileDragEdgeScroll>,
    next_tab_id: usize,
    /// 打字跳转的输入缓冲（同一时刻只有活动窗格在接收打字）。
    pub(crate) type_ahead: file_core::type_ahead::TypeAheadBuffer,
    /// vim 模式下字母层的状态（待定的第一个键、范围选择、`/` 跳转）。
    pub(crate) vim: crate::shortcuts::VimState,
    /// F1 快捷键一览开着。
    pub(crate) shortcut_help_open: bool,
    /// 本窗口最近关掉的标签页（快照，后关的在最后），Ctrl+Shift+T 依次恢复。只在内存里。
    closed_tabs: Vec<crate::model::BrowserTabSession>,
    next_pane_id: u64,
    next_icon_grid_expansion_session_id: u64,
    next_list_expansion_follow_session_id: u64,
    application_theme: ApplicationTheme,
    application_shutdown_phase: ApplicationShutdownPhase,
}

#[derive(Debug, Clone)]
struct PaneIconGridViewport {
    location: crate::model::LocationKey,
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

    /// 指针手势（拖拽起点、弹出菜单锚点）只会由指针事件触发，此时坐标必已
    /// 记录；键盘等没有指针的入口触发时退回窗口原点，这是整个应用唯一把
    /// "未知"折成坐标的地方。
    pub(crate) fn cursor_or_origin(&self) -> Point {
        self.cursor_position.get().unwrap_or(Point::ORIGIN)
    }

    pub(crate) fn active_language(&self) -> UiLanguage {
        self.user_config
            .language_setting
            .resolve(self.system_language)
    }

    /// 设置页缩略图用的颜色：和换上这套配色后真正的颜色同源。
    pub(crate) fn color_scheme_roles(
        &self,
        color_scheme: bennu_theme::ColorSchemePreset,
    ) -> bennu_theme::UiColorRoles {
        self.application_theme
            .scheme_roles(self.user_config.theme_mode, color_scheme)
    }

    pub(crate) fn matugen_colors_available(&self) -> bool {
        self.application_theme.matugen_available()
    }

    pub(crate) fn advance_window_animation_frame(&mut self) -> Task<Message> {
        self.preview_engine.preview_window_bottom_controls.advance();
        self.advance_terminal_panel_height_animation();
        self.advance_terminal_tab_shift_animations();
        self.advance_device_refresh_spin();
        // 动画帧改变布局（终端面板高度、标签栏出现、列表折叠、图标展开），
        // 逐帧把视口收进新内容。
        Task::batch([
            self.advance_file_drag_edge_scroll(),
            self.advance_address_bar_transition(),
            self.advance_tab_bar_reveal_animation(),
            self.advance_tab_animations(),
            self.advance_list_directory_animations(),
            self.advance_icon_grid_expansion_animation(),
            self.advance_sidebar_bookmark_motion(),
            self.clamp_visible_pane_viewports(),
        ])
    }

    fn refresh_current_language(&self) {
        bennu_localization::set_current_language(self.active_language());
    }

    pub(crate) fn file_operation_verification(&self) -> file_core::FileOperationVerification {
        self.user_config.file_operation_verification
    }

    pub(crate) fn remote_list_thumbnail_downloads_enabled(&self) -> bool {
        self.user_config.network_list_thumbnail_downloads_enabled
    }

    /// 这个路径的预览大小上限，0 = 不限制（命令层的既有约定）。文本按需读取，没有上限；
    /// 调用点都位于预览门禁之后，分类不会失败。
    pub(crate) fn preview_file_size_limit_for(&self, path: &Path) -> u64 {
        classify_preview_path(path, &self.user_config.preview_extension_rules)
            .and_then(PreviewPathKind::size_limit_kind)
            .map_or(0, |kind| self.user_config.preview_size_limits.limit(kind))
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

        // 启动时带来的激活（`--box`、打开属性）等主窗口建好再处理：弹出的框要给输入框焦点，窗口
        // 还没有时找不到它。
        let open_main_window = open_main_window.then(move |window_id| {
            Task::batch([
                wayland_dnd_window_handle_command(window_id),
                x11_dnd_window_handle_command(window_id),
                initial_desktop_activation
                    .clone()
                    .map(Message::DesktopActivationReceived)
                    .map(Task::done)
                    .unwrap_or_else(Task::none),
            ])
        });

        (browser, Task::batch([open_main_window, initial_tasks]))
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
        initial_tab.choose_view_mode(initial_view_mode);
        initial_tab.directory_order_phase = DirectoryOrderPhase::Ready {
            field: options.sort_field,
            direction: options.sort_direction,
        };
        // 引擎配置快照自 UserConfig 填充（此后每次偏好持久化同步刷新，
        // 见 config_persistence.rs），预览域行为参数单一来源仍是持久化层。
        let preview_engine_config = bennu_preview::engine::PreviewEngineConfig {
            directory_expand_levels: user_config.preview_directory_expand_levels,
            extension_rules: user_config.preview_extension_rules.clone(),
            size_limits: user_config.preview_size_limits,
        };
        let initial_pane = BrowserPane::new(BrowserPaneId::PRIMARY, initial_tab);
        let mut browser = Self {
            home_dir: placeholder_dir,
            trash_refresh: TrashRefreshState::default(),
            trash_batch_rescan_pending: false,
            hovered_entry: None,
            hovered_sidebar: None,
            sidebar_trash_hovered: false,
            hovered_sidebar_device: None,
            hovered_network_connection: None,
            cursor_paste_directory: None,
            preview_engine: bennu_preview::engine::PreviewEngine::new(
                preview_engine_config,
                default_preview_size(PreviewWindowProfile::Regular),
                windows::preview_window_identity(),
            ),
            next_directory_metadata_request_generation: 1,
            directory_metadata_in_flight: Default::default(),
            entry_metadata_revision: 0,
            main_window,
            maximized_windows: HashSet::new(),
            wayland_dnd: None,
            x11_dnd: None,
            file_manager_activation,
            focused_window: main_window,
            system_focused_window: None,
            thumbnail_cache: ThumbnailCache::new(thumbnails::thumbnail_cache_dir()),
            icon_grid_viewports: HashMap::new(),
            icon_grid_expansion: None,
            list_expansion_follow: None,
            context_menu: None,
            open_with: None,
            super_box: None,
            super_box_commands: Default::default(),
            file_drop_prompt: None,
            archive_creation: None,
            convert: None,
            checksum: None,
            archive_extraction: None,
            archive_member_password: None,
            batch_rename: None,
            advanced_new_folder: None,
            pending_created_entry_after: None,
            sidebar_locations: Vec::new(),
            sidebar_devices: SidebarDeviceState::loading(),
            network_connections: NetworkConnectionState::default(),
            network_connection_editor: None,
            sidebar_bookmark_drop_slot: None,
            sidebar_bookmark_drag: None,
            sidebar_bookmark_motion: HashMap::new(),
            sidebar_width_when_shown: user_config.sidebar_width,
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
            renaming: None,
            pending_created_entry_rename: None,
            pending_operation: None,
            transfer_conflict: None,
            destructive_action_confirmation: None,
            settings_window: None,
            properties_window: None,
            properties: None,
            properties_load_generation: 0,
            transfer_window: None,
            transfer_tasks: transfer::TransferTasks::default(),
            checked_devices: transfer::CheckedDevices::default(),
            local_send_window: None,
            local_send_staged: Vec::new(),
            favorite_address_form: None,
            local_send_page: transfer::LocalSendPage::Devices,
            transfer_history: transfer::TransferHistory::default(),
            link_receiving: transfer::LinkReceiving::default(),
            local_send_tab: transfer::StagedTab::Files,
            local_send_text: iced::widget::text_editor::Content::new(),
            favorite_alias_inputs: HashMap::new(),
            transfer_runtime: None,
            transfer_receiver_role: transfer::ReceiverRole::Unclaimed,
            transfer_session: None,
            incoming_transfer: None,
            incoming_messages: std::collections::VecDeque::new(),
            transfer_device_alias_input: String::new(),
            transfer_port_input: String::new(),
            transfer_pin_input: None,
            transfer_pin_invalid: false,
            transfer_port_invalid: false,
            transfer_download_dir_input: String::new(),
            transfer_download_dir_invalid: false,
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
            file_drag_viewport: None,
            file_drop_session: None,
            file_drag_spring_hover: None,
            file_drop_layout_stale: false,
            next_file_drag_gesture_id: 0,
            file_drop_layout_generation: 0,
            file_entry_bounds: Vec::new(),
            breadcrumb_drop_target_bounds: Vec::new(),
            breadcrumb_drop_target_measurement_generation: 0,
            options: options.clone(),
            application_launch_request,
            user_config: user_config.clone(),
            preview_size_limit_inputs: config::preview_size_limit_inputs(
                &user_config.preview_size_limits,
            ),
            preview_size_limit_errors: [const { None }; config::PreviewSizeLimitKind::ALL.len()],
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
            restart_notice_visible: false,
            installed_fonts: font_settings::InstalledFontList::default(),
            startup_rendering_environment,
            pending_renderer_restart_environment: None,
            address_editing: None,
            address_bar_transition: None,
            next_address_editing_session_id: 1,
            column_width_overrides: HashMap::new(),
            column_width_reference_content_width: MAIN_WINDOW_INITIAL_WIDTH,
            terminal_emulator: user_config.terminal_emulator,
            terminal_panel: crate::terminal_panel::TerminalPanelState::new(),
            selected_settings_category: SettingsCategory::General,
            context_menu_settings_page: ContextMenuSettingsPage::FileEntry,
            context_menu_reset_confirmation: None,
            context_menu_settings_drag: None,
            context_menu_preview_expansion: None,
            settings_subpage: None,
            search_service,
            search_history_interaction: crate::model::SearchHistoryInteraction::default(),
            search_filter_menu: None,
            next_search_workspace_session_id: 1,
            focused_column_directory: None,
            last_pointer_clicked_column_directory: None,
            rename_input: String::new(),
            rename_input_history: file_operations::RenameInputHistory::default(),
            rename_rejection: None,
            global_error_notification: None,
            next_global_error_notification_generation: 0,
            application_logs: ApplicationLogViewState::new(
                crate::runtime_logging::journald_initialization_warning(),
            ),
            system_language: UiLanguage::English,
            cursor_position: bennu_theme::pointer_tracker::PointerPosition::default(),
            pointer_held_window: None,
            main_window_video_fullscreen: false,
            main_window_width: MAIN_WINDOW_INITIAL_WIDTH,
            main_window_height: MAIN_WINDOW_INITIAL_HEIGHT,
            cursor_over_browser_content_pane: None,
            hovered_pane_id: None,
            hovered_list_header_column: None,
            hovered_grouping_rail_pane: None,
            keyboard_modifiers: keyboard::Modifiers::default(),
            shortcut_capture: None,
            drag_selection_anchor: None,
            column_return_targets: HashMap::new(),
            pending_keyboard_column_focus: None,
            pending_selection_reveal: None,
            column_resize_drag: None,
            list_column_resize_drag: None,
            list_column_reorder_drag: None,
            last_activation_click: None,
            operation_queue: FileOperationQueue::new(),
            operation_progress_animation_frame: 0,
            operation_history: FileOperationHistory::new(),
            action_library: crate::actions::ActionLibrary::empty(),
            action_runs: actions::ActionRuns::default(),
            rules: actions::Rules::default(),
            action_hint: None,
            list_directory_summary_cache: crate::model::ListDirectorySummaryCache::default(),
            user_preferences_save_in_flight: false,
            pending_user_preferences_save: None,
            user_preferences_seen: user_config.user_preferences(),
            app_config_seen: config::AppConfig::from_user_config(&user_config),
            user_preferences_persistence: config::SettingsPersistence::Writable,
            app_config_persistence: config::SettingsPersistence::Writable,
            pending_browser_session_save: false,
            browser_session_saves_in_flight: 0,
            last_browser_session_save: None,
            file_drag_edge_scroll: None,
            next_tab_id: 1,
            type_ahead: file_core::type_ahead::TypeAheadBuffer::default(),
            vim: crate::shortcuts::VimState::default(),
            shortcut_help_open: false,
            closed_tabs: Vec::new(),
            next_pane_id: 1,
            next_icon_grid_expansion_session_id: 1,
            next_list_expansion_follow_session_id: 1,
            // 读到系统设置之前按浅色。
            application_theme: ApplicationTheme::new(AppearanceMode::Light),
            application_shutdown_phase: ApplicationShutdownPhase::Running,
        };
        // 全屏视频的控件亮着时，指针移动只由窗口根部的记录器记下时刻，引擎到点时读。
        let pointer_motion = browser.cursor_position.motion();
        browser.preview_engine.share_pointer_motion(pointer_motion);
        browser.refresh_current_language();
        browser.apply_media_preferences_to_preview();
        browser.refresh_column_width_reference_content_width();
        startup_trace::mark_once("file_browser_new_ready");
        let startup_rendering_environment = browser.startup_rendering_environment.clone();
        let thumbnail_cache_trimming = Task::future(thumbnails::keep_thumbnail_cache_trimmed(
            browser.thumbnail_cache.cache_dir().to_path_buf(),
        ))
        .discard();
        (
            browser,
            Task::batch([
                startup_environment_command(startup_rendering_environment),
                ensure_search_service_command(initial_search_service_request),
                thumbnail_cache_trimming,
            ]),
        )
    }

    fn title(&self, window: window::Id) -> String {
        self.window_title(window)
    }

    fn subscription(&self) -> Subscription<Message> {
        if !self.application_shutdown_phase.is_running() {
            if self.application_shutdown_phase.is_draining() {
                // 驱动者是 Task::stream 命令,已脱离订阅生命周期,排空期间
                // 无需为它们保留订阅;终态消息照常送达 update 的排空分支。
                let subscriptions = vec![event::listen_with(global_event_message)];
                return Subscription::batch(subscriptions);
            }
            return Subscription::none();
        }

        let mut subscriptions = vec![event::listen_with(global_event_message)];
        // 常驻 LocalSend 服务事件：单流常驻，服务重启换代时重挂。
        if let Some(runtime) = &self.transfer_runtime {
            subscriptions.push(transfer_service::local_send_event_subscription(
                runtime.generation,
                runtime.events.resubscribe(),
            ));
            if self.waits_for_receiver_lock() {
                subscriptions.push(transfer::receiver_lock_subscription());
            }
            if self.shows_device_list() {
                subscriptions.push(transfer_service::device_list_watch_subscription(
                    runtime.generation,
                    runtime.handle.device_list_watch(),
                ));
            }
        }
        if let Some((generation, remaining)) = self.global_error_notification_countdown() {
            subscriptions.push(
                time::every(remaining)
                    .with(generation)
                    .map(|(generation, _)| Message::GlobalErrorNotificationElapsed(generation)),
            );
        }
        subscriptions.push(storage_change_subscription());
        subscriptions.push(matugen_colors_subscription(
            config::matugen_theme_file_path(),
        ));
        subscriptions.push(system_mode_subscription());
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

        let include_hidden = self.options.include_hidden;
        let revealing = self.directories_listing_revealed_entries();
        subscriptions.extend(self.watched_directories().into_iter().map(|directory| {
            let passes_hidden = include_hidden || revealing.contains(directory.as_path());
            directory_watch_subscription(directory, passes_hidden)
        }));

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

        if let Some(properties) = &self.properties {
            subscriptions.extend(
                properties
                    .watched_directories()
                    .into_iter()
                    .map(properties_watch_subscription),
            );
        }

        if self.has_trash_tab() {
            subscriptions.extend(
                // 回收站列表来自快照，任何变化都要重拉快照。
                file_core::trash_bin::trash_watch_directories()
                    .into_iter()
                    .map(|directory| directory_watch_subscription(directory, true)),
            );
        }

        // 驱动者监督 tick:只在有可恢复任务在跑时检测信号超时的驱动者并换代重启。
        if self.operation_queue.needs_driver_supervision() {
            subscriptions.push(
                time::every(::file_operations::SUPERVISION_INTERVAL)
                    .map(|_| Message::OperationSupervisionTick),
            );
        }
        // 认领其他进程留下的任务、拿其他进程改的设置:状态库有写入时重新看,拿不到锁的
        // 任务阻塞等它被放手(见 file_operations 的 supervision)。
        if let Some(database) = self.operation_queue.state_database() {
            subscriptions.push(state_database_subscription(database.path().to_path_buf()));
        }
        subscriptions.extend(
            self.operation_queue
                .foreign_task_waits()
                .into_iter()
                .map(foreign_task_wait_subscription),
        );

        subscriptions.push(self.terminal_output_subscription());
        subscriptions.push(actions::action_library_subscription(self.active_language()));
        subscriptions.extend(self.rule_watch_subscriptions());

        let remote_preview_progress_is_indeterminate = matches!(
            self.preview_engine.preview.as_ref(),
            Some(PreviewState::DownloadingRemoteFile(download)) if download.fraction().is_none()
        );
        if self.operation_queue.has_active_indeterminate_progress()
            || self.transfer_tasks.has_indeterminate()
            || remote_preview_progress_is_indeterminate
        {
            subscriptions.push(
                time::every(OPERATION_PROGRESS_ANIMATION_INTERVAL)
                    .map(|_| Message::OperationProgressAnimationTick),
            );
        }

        // spring 候选存在期间:tick 驱动进度环重绘,到点触发自动打开。
        if self.file_drag_spring_hover.is_some() {
            subscriptions.push(
                time::every(
                    crate::app::selection::spring_open::FILE_DRAG_SPRING_OPEN_TICK_INTERVAL,
                )
                .map(|_| Message::FileDragSpringOpenTick),
            );
        }

        if self.preview_engine.preview_tree_animation_is_active() {
            subscriptions.push(
                time::every(PREVIEW_TREE_ANIMATION_INTERVAL)
                    .map(|_| Message::Preview(PreviewMessage::PreviewTreeAnimationTick)),
            );
        }

        if self
            .preview_engine
            .preview_window_bottom_controls
            .is_animating()
            || self.file_drag_edge_scroll_is_active()
            || self.address_bar_transition_is_active()
            || self.tab_bar_reveal_animation_is_active()
            || self.tab_animation_is_active()
            || self.list_directory_animation_is_active()
            || self.icon_grid_expansion_animation_is_active()
            || self.sidebar_bookmark_motion_is_active()
            || self.terminal_panel.is_animating()
            || self.device_refresh_is_spinning()
        {
            subscriptions.push(
                time::every(bennu_theme::animation::FRAME_INTERVAL_60HZ)
                    .map(|_| Message::WindowChromeAnimationTick),
            );
        }

        if self.preview_engine.audio_preview_is_active() {
            subscriptions.push(audio_preview_tick_subscription().map(Message::Preview));
        }

        // 预览中的文本文件被追加/改写时跟着变；没有文本预览时是空订阅。
        subscriptions.push(
            self.preview_engine
                .text_file_watch_subscription()
                .map(Message::Preview),
        );
        // 文件夹预览：监视根和展开的文件夹、测量在跑时的读数节拍；不是文件夹时是空订阅。
        subscriptions.push(
            self.preview_engine
                .folder_preview_subscription()
                .map(Message::Preview),
        );

        if self.preview_engine.video_preview_is_active() {
            subscriptions.push(video_preview_tick_subscription().map(Message::Preview));
        }

        if let Some((path, generation, position)) =
            self.preview_engine.active_animated_image_preview_stream()
        {
            subscriptions.push(
                animated_image_preview_subscription(path, generation, position)
                    .map(Message::Preview),
            );
        }

        if let Some(segment) = self.preview_engine.active_video_preview_stream() {
            subscriptions.push(video_preview_subscription(segment).map(Message::Preview));
        }

        Subscription::batch(subscriptions)
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.application_theme
            .active(self.user_config.theme_mode, self.user_config.color_scheme)
    }

    fn window_frame<'a>(
        &'a self,
        window: window::Id,
        build: impl FnOnce(WindowShell<'a, Message>) -> Element<'a, Message>,
    ) -> Element<'a, Message> {
        build(window_shell(self, window))
    }

    fn single_window_view<'a>(
        &'a self,
        window: window::Id,
        header: HeaderBar<'a, Message>,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        self.window_frame(window, |shell| single_window(header, content, &shell))
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        let mut content = self.window_content(window);
        if window == self.main_window {
            content = crate::window_layout_size::report_window_layout_size(
                content,
                Message::MainWindowLaidOut,
            );
        }
        let shares_position =
            window == self.main_window || self.preview_engine.preview_window == Some(window);
        bennu_theme::pointer_tracker::track_pointer(
            content,
            shares_position.then(|| self.cursor_position.clone()),
            self.pointer_follow(window),
        )
    }

    fn window_content(&self, window: window::Id) -> Element<'_, Message> {
        match self.auxiliary_window(window) {
            Some(AuxiliaryWindow::Settings) => view_settings_window(self, window),
            Some(AuxiliaryWindow::Properties) => self.window_frame(window, |shell| {
                view_properties_window(self.properties.as_ref(), shell)
            }),
            Some(AuxiliaryWindow::Transfer) => self.single_window_view(
                window,
                HeaderBar::new().title(tr!("Send to Phone")),
                crate::view::view_transfer_window(self),
            ),
            Some(AuxiliaryWindow::LocalSend) => self.single_window_view(
                window,
                crate::view::local_send_header(self),
                crate::view::view_local_send_window(self),
            ),
            Some(AuxiliaryWindow::Preview) => self.view_preview_window(window),
            None => self.view_main_or_closing_window(window),
        }
    }

    /// 预览窗口：顶栏常驻（标题是文件名，左边“固定”）；视频/音频底部控制条仍自动淡出。
    fn view_preview_window(&self, window: window::Id) -> Element<'_, Message> {
        let engine = &self.preview_engine;
        let title = engine
            .preview_shown_path
            .as_deref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| tr!("Preview"));
        let opacity = engine.preview_window_bottom_controls.opacity();
        // 全屏：没有顶栏和窗口边框，内容就是整个屏幕。
        if engine.video_fullscreen == Some(VideoFullscreen::PreviewWindow) {
            return self.preview_window_content(
                PreviewSize {
                    width: engine.preview_size.width,
                    height: engine.preview_size.height + HEADER_BAR_HEIGHT,
                },
                opacity,
            );
        }
        let header = HeaderBar::new()
            .title(title)
            .start(preview_pin_button(engine.preview_window_pinned));
        let content = self.preview_window_content(engine.preview_size, opacity);
        self.single_window_view(window, header, content)
    }

    fn view_main_or_closing_window(&self, window: window::Id) -> Element<'_, Message> {
        if window != self.main_window {
            // 已登记关闭、但合成器还没销毁的窗口：渲染占位避免空白。
            return iced::widget::container(bennu_preview::panel_text::readable_text(tr!(
                "Closing window..."
            )))
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .into();
        }
        if self.main_window_video_fullscreen {
            return self.main_window_video_fullscreen_view();
        }
        startup_trace::mark_once("first_main_window_view");
        if !self.tab().directory_collection_phase.is_discovering()
            && self.tab().directory_order_phase.is_ready()
        {
            startup_trace::mark_once("first_browser_view_after_initial_load");
        }
        view_browser(self)
    }
}
