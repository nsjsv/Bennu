use std::path::PathBuf;

use desktop_linux::{DisplayRendererGpu, TerminalEmulator};
use file_core::{FileOperationVerification, SortDirection, SortField};

use crate::matugen_theme::{
    default_custom_color_scheme, ColorSchemePreset, CustomColorScheme, ThemeMode,
};
use crate::model::{
    BrowserViewMode, ContextMenuPreferences, FileGroupingMode, LastSearchScope,
    ListDirectorySizeDisplayMode, SearchHistory,
};
use crate::network_connections::SavedNetworkConnection;
use crate::shortcuts::ShortcutConfig;

mod app_config;
#[cfg(test)]
pub(crate) use app_config::default_app_config;
pub(crate) use app_config::{
    load_app_config, save_app_config, save_app_config_preserving_probe_cache, AppConfig,
    RendererProbeCacheRecord,
};
pub(crate) mod column_width_adjust_mode;
pub(crate) use column_width_adjust_mode::{
    ColumnWidthAdjustMode, DEFAULT_COLUMN_WIDTH_ADJUST_MODE,
};
pub(crate) mod launch_window;
mod legacy_toml;
pub(crate) use launch_window::{
    stored_launch_window_policy, LaunchWindowPolicy, DEFAULT_LAUNCH_WINDOW_POLICY,
};
pub(crate) mod startup;
pub(crate) use startup::StartupLocationPolicy;
mod user_preferences;
pub(crate) use user_preferences::{
    load_user_config_for_app_config, save_user_preferences, UserPreferences,
};

const APP_DIR_NAME: &str = "bennu";
pub(super) const CONFIG_FILE_NAME: &str = "config.toml";
const MATUGEN_THEME_FILE_NAME: &str = "matugen.toml";
const STATE_DATABASE_FILE_NAME: &str = "state.sqlite";

pub(crate) const DEFAULT_TERMINAL_EMULATOR: TerminalEmulator = TerminalEmulator::Automatic;
pub(crate) const DEFAULT_RENDERING_GPU_PREFERENCE: RenderingGpuPreference =
    RenderingGpuPreference::DisplayGpu;
pub(crate) const DEFAULT_FILE_OPERATION_VERIFICATION: FileOperationVerification =
    FileOperationVerification::BasicMetadata;
pub(crate) const DEFAULT_SIDEBAR_WIDTH: f32 = 180.0;
pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 140.0;
pub(crate) const MAX_SIDEBAR_WIDTH: f32 = 360.0;
pub(crate) const DEFAULT_RIGHT_PREVIEW_PANEL_WIDTH: f32 = 320.0;
pub(crate) const MIN_RIGHT_PREVIEW_PANEL_WIDTH: f32 = 200.0;
pub(crate) const MAX_RIGHT_PREVIEW_PANEL_WIDTH: f32 = 640.0;
pub(crate) const DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO: f32 = 0.7;
pub(crate) const MIN_RIGHT_PREVIEW_PREVIEW_RATIO: f32 = 0.25;
pub(crate) const MAX_RIGHT_PREVIEW_PREVIEW_RATIO: f32 = 1.0;
pub(crate) const MIN_COLUMN_WIDTH: f32 = 96.0;
pub(crate) const MAX_COLUMN_WIDTH: f32 = 960.0;
pub(crate) const MIN_VISIBLE_COLUMN_COUNT: usize = 3;
pub(crate) const DEFAULT_VISIBLE_COLUMN_COUNT: usize = 3;
pub(crate) const MAX_VISIBLE_COLUMN_COUNT: usize = 5;
pub(crate) const DEFAULT_SEARCH_MAX_EXTRACT_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const DEFAULT_ICON_GRID_SIZE: u32 = 96;
pub(crate) const MIN_ICON_GRID_SIZE: u32 = 64;
pub(crate) const MAX_ICON_GRID_SIZE: u32 = 192;
pub(crate) const ICON_GRID_SIZE_STEP: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewDensityLevel(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewDensityStep {
    Increase,
    Decrease,
}

impl ViewDensityLevel {
    const MAX_INDEX: u8 = ((MAX_ICON_GRID_SIZE - MIN_ICON_GRID_SIZE) / ICON_GRID_SIZE_STEP) as u8;
    pub(crate) const DEFAULT: Self =
        Self(((DEFAULT_ICON_GRID_SIZE - MIN_ICON_GRID_SIZE) / ICON_GRID_SIZE_STEP) as u8);

    pub(crate) const fn from_index(index: u8) -> Self {
        Self(if index > Self::MAX_INDEX {
            Self::MAX_INDEX
        } else {
            index
        })
    }

    pub(crate) const fn index(self) -> u8 {
        self.0
    }

    pub(crate) const fn icon_grid_size(self) -> u32 {
        MIN_ICON_GRID_SIZE + self.0 as u32 * ICON_GRID_SIZE_STEP
    }

    pub(crate) fn scale(self) -> f32 {
        self.icon_grid_size() as f32 / DEFAULT_ICON_GRID_SIZE as f32
    }

    pub(crate) fn from_icon_grid_size(size: u32) -> Self {
        let size = normalize_icon_grid_size(size);
        let index = (size - MIN_ICON_GRID_SIZE + ICON_GRID_SIZE_STEP / 2) / ICON_GRID_SIZE_STEP;
        Self::from_index(index as u8)
    }

    pub(crate) const fn step(self, step: ViewDensityStep) -> Self {
        match step {
            ViewDensityStep::Increase => Self::from_index(self.0.saturating_add(1)),
            ViewDensityStep::Decrease => Self(self.0.saturating_sub(1)),
        }
    }
}

impl UserConfig {
    /// Icons 视图活动几何的唯一读取入口；`icon_grid_size` 字段只保留兼容镜像用途。
    pub(crate) fn icons_icon_edge(&self) -> u32 {
        self.icons_view_density.icon_grid_size()
    }

    pub(crate) fn set_icons_view_density(&mut self, level: ViewDensityLevel) {
        self.icons_view_density = level;
        self.icon_grid_size = level.icon_grid_size();
    }
}

// 纯搬移：UiLanguage 枚举本体已下沉 bennu-localization（翻译表按它
// 分派，语言设置解析在此仍需引用），re-export 维持
// crate::config::UiLanguage 既有路径；TOML 存取（UiLanguageSetting）
// 属 app-ui 存储域，留在本文件。
pub(crate) use bennu_localization::UiLanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiLanguageSetting {
    System,
    English,
    Chinese,
}

impl UiLanguageSetting {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "english" => Some(Self::English),
            "chinese" => Some(Self::Chinese),
            _ => None,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "english",
            Self::Chinese => "chinese",
        }
    }

    pub(crate) fn resolve(self, system_language: UiLanguage) -> UiLanguage {
        match self {
            Self::System => system_language,
            Self::English => UiLanguage::English,
            Self::Chinese => UiLanguage::Chinese,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderingGpuPreference {
    DisplayGpu,
    HighPerformanceGpu,
}

impl RenderingGpuPreference {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "display" => Some(Self::DisplayGpu),
            "gpu" => Some(Self::HighPerformanceGpu),
            _ => None,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::DisplayGpu => "display",
            Self::HighPerformanceGpu => "gpu",
        }
    }

    pub(crate) fn iced_backend_candidates(self) -> &'static str {
        match self {
            Self::DisplayGpu | Self::HighPerformanceGpu => "wgpu,tiny-skia",
        }
    }

    pub(crate) fn wgpu_power_preference(
        self,
        display_renderer_gpu: Option<&DisplayRendererGpu>,
    ) -> Option<&'static str> {
        match self {
            Self::DisplayGpu => Some(
                display_renderer_gpu
                    .map(|gpu| gpu.class().wgpu_power_preference())
                    .unwrap_or("none"),
            ),
            Self::HighPerformanceGpu => Some("high"),
        }
    }

    pub(crate) fn mesa_vulkan_device_select(
        self,
        display_renderer_gpu: Option<&DisplayRendererGpu>,
    ) -> Option<String> {
        match self {
            Self::DisplayGpu => {
                display_renderer_gpu.map(DisplayRendererGpu::mesa_vulkan_device_select)
            }
            Self::HighPerformanceGpu => None,
        }
    }
}

pub(crate) fn file_operation_verification_from_config_value(
    value: &str,
) -> Option<FileOperationVerification> {
    match value {
        "basic_metadata" => Some(FileOperationVerification::BasicMetadata),
        "strong" => Some(FileOperationVerification::Strong),
        _ => None,
    }
}

pub(crate) fn file_operation_verification_config_value(
    verification: FileOperationVerification,
) -> &'static str {
    match verification {
        FileOperationVerification::BasicMetadata => "basic_metadata",
        FileOperationVerification::Strong => "strong",
    }
}

pub(crate) fn browser_view_mode_from_config_value(value: &str) -> Option<BrowserViewMode> {
    match value {
        "columns" => Some(BrowserViewMode::Columns),
        "list" => Some(BrowserViewMode::List),
        "icons" => Some(BrowserViewMode::Icons),
        _ => None,
    }
}

pub(crate) fn browser_view_mode_config_value(view_mode: BrowserViewMode) -> &'static str {
    match view_mode {
        BrowserViewMode::Columns => "columns",
        BrowserViewMode::List => "list",
        BrowserViewMode::Icons => "icons",
    }
}

pub(crate) fn sort_field_from_config_value(value: &str) -> Option<SortField> {
    match value {
        "name" => Some(SortField::Name),
        "modified" => Some(SortField::Modified),
        "size" => Some(SortField::Size),
        "kind" => Some(SortField::Kind),
        _ => None,
    }
}

pub(crate) fn sort_field_config_value(field: SortField) -> &'static str {
    match field {
        SortField::Name => "name",
        SortField::Modified => "modified",
        SortField::Size => "size",
        SortField::Kind => "kind",
    }
}

pub(crate) fn sort_direction_from_config_value(value: &str) -> Option<SortDirection> {
    match value {
        "ascending" => Some(SortDirection::Ascending),
        "descending" => Some(SortDirection::Descending),
        _ => None,
    }
}

pub(crate) fn sort_direction_config_value(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "ascending",
        SortDirection::Descending => "descending",
    }
}

pub(crate) fn list_directory_size_display_mode_from_config_value(
    value: &str,
) -> Option<ListDirectorySizeDisplayMode> {
    ListDirectorySizeDisplayMode::from_config_value(value)
}

pub(crate) fn list_directory_size_display_mode_config_value(
    mode: ListDirectorySizeDisplayMode,
) -> &'static str {
    mode.config_value()
}

// 纯搬移：预览配置域（分类型大小上限/后缀规则/目录展开层级及其默认值
// 与归一化函数）已下沉 bennu-preview（preview_config 模块）；re-export
// 维持 crate::config::* 既有调用路径，TOML 键解析/写出仍在本 crate
// 存储域（user_preferences/legacy_toml）。
pub(crate) use bennu_preview::preview_config::{
    normalize_preview_directory_expand_levels, normalize_preview_extension,
    preview_size_limit_bytes_from_mib, preview_size_limit_mib, PreviewExtensionRules,
    PreviewFileSizeKind, PreviewFileSizeLimits, DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS,
    MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS,
};

#[derive(Debug, Clone)]
pub(crate) struct UserConfig {
    pub(crate) thumbnail_cache_dir: PathBuf,
    pub(crate) network_list_thumbnail_downloads_enabled: bool,
    pub(crate) preview_size_limits: PreviewFileSizeLimits,
    pub(crate) preview_directory_expand_levels: u8,
    pub(crate) preview_extension_rules: PreviewExtensionRules,
    pub(crate) show_hidden_files: bool,
    pub(crate) language_setting: UiLanguageSetting,
    pub(crate) sidebar_width: f32,
    pub(crate) right_preview_panel_open: bool,
    pub(crate) right_preview_panel_width: f32,
    pub(crate) right_preview_preview_ratio: f32,
    pub(crate) sidebar_favorites: Option<Vec<SidebarFavoriteConfig>>,
    pub(crate) network_connections: Vec<SavedNetworkConnection>,
    pub(crate) terminal_emulator: TerminalEmulator,
    /// 内嵌终端 shell;空串 = 跟随系统登录 shell。
    pub(crate) terminal_shell: String,
    pub(crate) rendering_gpu_preference: RenderingGpuPreference,
    pub(crate) search_content_indexing_enabled: bool,
    pub(crate) search_max_extract_bytes: u64,
    pub(crate) search_history: SearchHistory,
    /// 最近一次搜索的范围记忆；打开下一个搜索工作区时恢复为默认档。
    pub(crate) last_search_scope: Option<LastSearchScope>,
    pub(crate) theme_mode: ThemeMode,
    pub(crate) color_scheme: ColorSchemePreset,
    pub(crate) custom_color_scheme: CustomColorScheme,
    pub(crate) file_operation_verification: FileOperationVerification,
    pub(crate) browser_view_mode: BrowserViewMode,
    pub(crate) visible_column_count: usize,
    pub(crate) window_controls: crate::model::WindowControlsConfig,
    pub(crate) icon_grid_size: u32,
    pub(crate) columns_view_density: ViewDensityLevel,
    pub(crate) list_view_density: ViewDensityLevel,
    pub(crate) icons_view_density: ViewDensityLevel,
    pub(crate) list_view_preferences: crate::model::ListViewPreferences,
    pub(crate) list_directory_size_display_mode: ListDirectorySizeDisplayMode,
    /// 文件分组维度，全局一份（列表/大图共用，多栏不读）；默认无分组。
    pub(crate) file_grouping: FileGroupingMode,
    pub(crate) startup_location_policy: StartupLocationPolicy,
    pub(crate) startup_custom_directory: PathBuf,
    pub(crate) save_view_state: bool,
    pub(crate) shortcuts: ShortcutConfig,
    pub(crate) launch_window_policy: LaunchWindowPolicy,
    pub(crate) column_width_adjust_mode: ColumnWidthAdjustMode,
    pub(crate) context_menus: ContextMenuPreferences,
    /// 传输接收保存目录;None = 使用默认 ~/Downloads。
    pub(crate) transfer_download_dir: Option<PathBuf>,
    /// 本机对外设备显示名;None = 使用 hostname。
    pub(crate) transfer_device_alias: Option<String>,
    /// 信任的 LocalSend 设备;这些设备的发送请求自动接收。
    pub(crate) transfer_trusted_devices: Vec<TrustedTransferDevice>,
}

/// 信任设备列表条目:fingerprint 是 LocalSend 协议设备身份,alias/added_at
/// 仅用于设置页展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedTransferDevice {
    pub(crate) fingerprint: String,
    pub(crate) alias: String,
    pub(crate) added_at: String,
}

/// 本机默认对外设备名:hostname。gethostname 是 POSIX 标准接口,
/// 失败回退固定名,不影响发现流程。
pub(crate) fn default_transfer_device_alias() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: buffer 长度充足,且 gethostname 保证以 NUL 结尾或截断。
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result == 0 {
        if let Ok(name) = std::ffi::CStr::from_bytes_until_nul(&buffer) {
            let name = name.to_string_lossy().trim().to_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "Bennu".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarFavoriteConfig {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
}

pub(crate) fn default_state_database_path() -> PathBuf {
    let fallback_base = dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    dirs::data_dir()
        .unwrap_or(fallback_base)
        .join(APP_DIR_NAME)
        .join(STATE_DATABASE_FILE_NAME)
}

pub(crate) fn default_user_config() -> UserConfig {
    let fallback_base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cache_base = dirs::cache_dir().unwrap_or_else(|| fallback_base.clone());
    UserConfig {
        thumbnail_cache_dir: cache_base.join("thumbnails"),
        network_list_thumbnail_downloads_enabled: false,
        preview_size_limits: PreviewFileSizeLimits::with_default_limits(),
        preview_directory_expand_levels: DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS,
        preview_extension_rules: PreviewExtensionRules::default_rules(),
        show_hidden_files: false,
        language_setting: UiLanguageSetting::System,
        sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        right_preview_panel_open: false,
        right_preview_panel_width: DEFAULT_RIGHT_PREVIEW_PANEL_WIDTH,
        right_preview_preview_ratio: DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO,
        sidebar_favorites: None,
        network_connections: Vec::new(),
        terminal_emulator: DEFAULT_TERMINAL_EMULATOR,
        terminal_shell: String::new(),
        rendering_gpu_preference: DEFAULT_RENDERING_GPU_PREFERENCE,
        search_content_indexing_enabled: true,
        search_max_extract_bytes: DEFAULT_SEARCH_MAX_EXTRACT_BYTES,
        search_history: SearchHistory::default(),
        last_search_scope: None,
        theme_mode: ThemeMode::Automatic,
        color_scheme: ColorSchemePreset::Default,
        custom_color_scheme: default_custom_color_scheme(),
        file_operation_verification: DEFAULT_FILE_OPERATION_VERIFICATION,
        browser_view_mode: BrowserViewMode::Columns,
        visible_column_count: DEFAULT_VISIBLE_COLUMN_COUNT,
        window_controls: crate::model::WindowControlsConfig::default(),
        icon_grid_size: DEFAULT_ICON_GRID_SIZE,
        columns_view_density: ViewDensityLevel::DEFAULT,
        list_view_density: ViewDensityLevel::DEFAULT,
        icons_view_density: ViewDensityLevel::DEFAULT,
        list_view_preferences: crate::model::ListViewPreferences::default(),
        list_directory_size_display_mode: ListDirectorySizeDisplayMode::ItemCount,
        file_grouping: FileGroupingMode::None,
        startup_location_policy: StartupLocationPolicy::Home,
        startup_custom_directory: fallback_base.clone(),
        save_view_state: false,
        launch_window_policy: DEFAULT_LAUNCH_WINDOW_POLICY,
        column_width_adjust_mode: DEFAULT_COLUMN_WIDTH_ADJUST_MODE,
        shortcuts: ShortcutConfig::defaults(),
        context_menus: crate::model::ContextMenuPreferences::defaults(),
        transfer_download_dir: None,
        transfer_device_alias: None,
        transfer_trusted_devices: Vec::new(),
    }
}

pub(crate) fn ui_thread_startup_config() -> UserConfig {
    UserConfig {
        thumbnail_cache_dir: PathBuf::new(),
        network_list_thumbnail_downloads_enabled: false,
        preview_size_limits: PreviewFileSizeLimits::with_default_limits(),
        preview_directory_expand_levels: DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS,
        preview_extension_rules: PreviewExtensionRules::default_rules(),
        show_hidden_files: false,
        language_setting: UiLanguageSetting::System,
        sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        right_preview_panel_open: false,
        right_preview_panel_width: DEFAULT_RIGHT_PREVIEW_PANEL_WIDTH,
        right_preview_preview_ratio: DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO,
        sidebar_favorites: None,
        network_connections: Vec::new(),
        terminal_emulator: DEFAULT_TERMINAL_EMULATOR,
        terminal_shell: String::new(),
        rendering_gpu_preference: DEFAULT_RENDERING_GPU_PREFERENCE,
        search_content_indexing_enabled: true,
        search_max_extract_bytes: DEFAULT_SEARCH_MAX_EXTRACT_BYTES,
        search_history: SearchHistory::default(),
        last_search_scope: None,
        theme_mode: ThemeMode::Automatic,
        color_scheme: ColorSchemePreset::Default,
        custom_color_scheme: default_custom_color_scheme(),
        file_operation_verification: DEFAULT_FILE_OPERATION_VERIFICATION,
        browser_view_mode: BrowserViewMode::Columns,
        visible_column_count: DEFAULT_VISIBLE_COLUMN_COUNT,
        window_controls: crate::model::WindowControlsConfig::default(),
        icon_grid_size: DEFAULT_ICON_GRID_SIZE,
        columns_view_density: ViewDensityLevel::DEFAULT,
        list_view_density: ViewDensityLevel::DEFAULT,
        icons_view_density: ViewDensityLevel::DEFAULT,
        list_view_preferences: crate::model::ListViewPreferences::default(),
        list_directory_size_display_mode: ListDirectorySizeDisplayMode::ItemCount,
        file_grouping: FileGroupingMode::None,
        startup_location_policy: StartupLocationPolicy::Home,
        startup_custom_directory: PathBuf::new(),
        save_view_state: false,
        shortcuts: ShortcutConfig::defaults(),
        context_menus: ContextMenuPreferences::defaults(),
        launch_window_policy: DEFAULT_LAUNCH_WINDOW_POLICY,
        column_width_adjust_mode: DEFAULT_COLUMN_WIDTH_ADJUST_MODE,
        transfer_download_dir: None,
        transfer_device_alias: None,
        transfer_trusted_devices: Vec::new(),
    }
}

pub(crate) fn normalize_column_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
    } else {
        MIN_COLUMN_WIDTH
    }
}

pub(crate) fn normalize_sidebar_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
    } else {
        DEFAULT_SIDEBAR_WIDTH
    }
}

pub(crate) fn normalize_right_preview_panel_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_RIGHT_PREVIEW_PANEL_WIDTH, MAX_RIGHT_PREVIEW_PANEL_WIDTH)
    } else {
        DEFAULT_RIGHT_PREVIEW_PANEL_WIDTH
    }
}

/// 存储边的比例合法性归一;信息区 120px 保底依赖运行期窗高,由
/// app 层读取侧按当前窗高再夹取,这里只挡非法值(非有限/越界)。
pub(crate) fn normalize_right_preview_preview_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(
            MIN_RIGHT_PREVIEW_PREVIEW_RATIO,
            MAX_RIGHT_PREVIEW_PREVIEW_RATIO,
        )
    } else {
        DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO
    }
}

pub(crate) fn normalize_icon_grid_size(size: u32) -> u32 {
    size.clamp(MIN_ICON_GRID_SIZE, MAX_ICON_GRID_SIZE)
}

pub(crate) fn normalize_visible_column_count(count: usize) -> usize {
    count.clamp(MIN_VISIBLE_COLUMN_COUNT, MAX_VISIBLE_COLUMN_COUNT)
}

pub(super) fn app_config_dir_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join(APP_DIR_NAME))
}

pub(crate) fn preview_size_limit_mib_inputs(limits: &PreviewFileSizeLimits) -> [String; 7] {
    PreviewFileSizeKind::ALL.map(|kind| preview_size_limit_mib(limits.limit(kind)).to_string())
}

pub(crate) fn matugen_theme_file_path() -> Option<PathBuf> {
    app_config_dir_path().map(|path| path.join(MATUGEN_THEME_FILE_NAME))
}

pub(crate) fn toml_string<'a>(document: &'a toml::Table, key: &str) -> Option<&'a str> {
    document
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod view_density_tests {
    use super::*;

    #[test]
    fn levels_map_all_sizes_and_clamp_inputs() {
        for (index, size) in [64, 80, 96, 112, 128, 144, 160, 176, 192]
            .into_iter()
            .enumerate()
        {
            let level = ViewDensityLevel::from_index(index as u8);
            assert_eq!((level.index(), level.icon_grid_size()), (index as u8, size));
        }

        assert_eq!(ViewDensityLevel::DEFAULT.index(), 2);
        assert_eq!(ViewDensityLevel::DEFAULT.scale(), 1.0);
        assert_eq!(ViewDensityLevel::from_index(u8::MAX).index(), 8);
        assert_eq!(ViewDensityLevel::from_icon_grid_size(0).index(), 0);
        assert_eq!(ViewDensityLevel::from_icon_grid_size(71).index(), 0);
        assert_eq!(ViewDensityLevel::from_icon_grid_size(72).index(), 1);
        assert_eq!(ViewDensityLevel::from_icon_grid_size(u32::MAX).index(), 8);
    }

    #[test]
    fn stepping_uses_direction_and_stops_at_boundaries() {
        assert_eq!(
            ViewDensityLevel::from_index(0)
                .step(ViewDensityStep::Decrease)
                .index(),
            0
        );
        assert_eq!(
            ViewDensityLevel::from_index(0)
                .step(ViewDensityStep::Increase)
                .index(),
            1
        );
        assert_eq!(
            ViewDensityLevel::from_index(8)
                .step(ViewDensityStep::Increase)
                .index(),
            8
        );
        assert_eq!(
            ViewDensityLevel::from_index(8)
                .step(ViewDensityStep::Decrease)
                .index(),
            7
        );
    }
}

#[cfg(test)]
mod tests;
