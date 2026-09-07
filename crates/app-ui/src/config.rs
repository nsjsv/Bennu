use std::path::{Path, PathBuf};

use desktop_linux::{DisplayRendererGpu, TerminalEmulator};
use file_core::{FileOperationVerification, SortDirection, SortField};

use crate::matugen_theme::{
    default_custom_color_scheme, ColorSchemePreset, CustomColorScheme, ThemeMode,
};
use crate::model::{
    BrowserViewMode, ContextMenuPreferences, ListDirectorySizeDisplayMode, SearchHistory,
};
use crate::network_connections::SavedNetworkConnection;
use crate::shortcuts::ShortcutConfig;

mod app_config;
pub(crate) use app_config::{
    load_app_config, save_app_config, save_app_config_preserving_probe_cache, AppConfig,
    RendererProbeCacheRecord,
};
#[cfg(test)]
pub(crate) use app_config::default_app_config;
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

const APP_DIR_NAME: &str = "file-manager";
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
pub(crate) const PREVIEW_FILE_SIZE_UNIT_BYTES: u64 = 1024 * 1024;
pub(crate) const DEFAULT_PREVIEW_TEXT_SIZE_BYTES: u64 = 25 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_IMAGE_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_VIDEO_SIZE_BYTES: u64 = 1024 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_AUDIO_SIZE_BYTES: u64 = 200 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_ARCHIVE_SIZE_BYTES: u64 = 25 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_SQLITE_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const DEFAULT_PREVIEW_DOCUMENT_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub(crate) const MIN_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 0;
pub(crate) const MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 3;
pub(crate) const DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 1;
/// 空格预览各类型的默认后缀表，镜像各预览类型的内置判定；
/// 后缀以小写、无前导点的规范形态存储。替换式语义：用户可增删，
/// 删除即该后缀不再按此类型预览。
pub(crate) const DEFAULT_PREVIEW_TEXT_EXTENSIONS: [&str; 22] = [
    "txt", "md", "log", "conf", "ini", "yaml", "yml", "json", "xml", "toml", "sh", "py", "js",
    "ts", "c", "cpp", "h", "rs", "java", "css", "html", "csv",
];
pub(crate) const DEFAULT_PREVIEW_IMAGE_EXTENSIONS: [&str; 11] = [
    "avif", "bmp", "gif", "ico", "jpg", "jpeg", "png", "svg", "tif", "tiff", "webp",
];
pub(crate) const DEFAULT_PREVIEW_VIDEO_EXTENSIONS: [&str; 6] =
    ["mp4", "m4v", "mkv", "mov", "webm", "avi"];
pub(crate) const DEFAULT_PREVIEW_AUDIO_EXTENSIONS: [&str; 7] =
    ["mp3", "wav", "flac", "ogg", "oga", "m4a", "aac"];
pub(crate) const DEFAULT_PREVIEW_SQLITE_EXTENSIONS: [&str; 4] = ["db", "sqlite", "sqlite3", "db3"];
pub(crate) const DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS: [&str; 6] =
    ["zip", "tar", "tar.gz", "tgz", "7z", "rar"];
pub(crate) const DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS: [&str; 10] = [
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
];
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiLanguage {
    English,
    Chinese,
}

impl UiLanguage {
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Self::English => 0,
            Self::Chinese => 1,
        }
    }

    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Chinese,
            _ => Self::English,
        }
    }
}

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
            Self::DisplayGpu | Self::HighPerformanceGpu => "wgpu",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewFileSizeKind {
    Text,
    Image,
    Video,
    Audio,
    Archive,
    Document,
    Sqlite,
}

impl PreviewFileSizeKind {
    pub(crate) const ALL: [PreviewFileSizeKind; 7] = [
        Self::Text,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Document,
        Self::Sqlite,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreviewFileSizeLimits {
    pub(crate) text_bytes: u64,
    pub(crate) image_bytes: u64,
    pub(crate) video_bytes: u64,
    pub(crate) audio_bytes: u64,
    pub(crate) archive_bytes: u64,
    pub(crate) document_bytes: u64,
    pub(crate) sqlite_bytes: u64,
}

impl PreviewFileSizeLimits {
    pub(crate) fn with_default_limits() -> Self {
        Self {
            text_bytes: DEFAULT_PREVIEW_TEXT_SIZE_BYTES,
            image_bytes: DEFAULT_PREVIEW_IMAGE_SIZE_BYTES,
            video_bytes: DEFAULT_PREVIEW_VIDEO_SIZE_BYTES,
            audio_bytes: DEFAULT_PREVIEW_AUDIO_SIZE_BYTES,
            archive_bytes: DEFAULT_PREVIEW_ARCHIVE_SIZE_BYTES,
            document_bytes: DEFAULT_PREVIEW_DOCUMENT_SIZE_BYTES,
            sqlite_bytes: DEFAULT_PREVIEW_SQLITE_SIZE_BYTES,
        }
    }

    pub(crate) fn limit(self, kind: PreviewFileSizeKind) -> u64 {
        match kind {
            PreviewFileSizeKind::Text => self.text_bytes,
            PreviewFileSizeKind::Image => self.image_bytes,
            PreviewFileSizeKind::Video => self.video_bytes,
            PreviewFileSizeKind::Audio => self.audio_bytes,
            PreviewFileSizeKind::Archive => self.archive_bytes,
            PreviewFileSizeKind::Document => self.document_bytes,
            PreviewFileSizeKind::Sqlite => self.sqlite_bytes,
        }
    }

    pub(crate) fn set_limit(&mut self, kind: PreviewFileSizeKind, bytes: u64) {
        match kind {
            PreviewFileSizeKind::Text => self.text_bytes = bytes,
            PreviewFileSizeKind::Image => self.image_bytes = bytes,
            PreviewFileSizeKind::Video => self.video_bytes = bytes,
            PreviewFileSizeKind::Audio => self.audio_bytes = bytes,
            PreviewFileSizeKind::Archive => self.archive_bytes = bytes,
            PreviewFileSizeKind::Document => self.document_bytes = bytes,
            PreviewFileSizeKind::Sqlite => self.sqlite_bytes = bytes,
        }
    }

    /// 迁移时用旧的全局单值上限同时填充全部六个类型。
    pub(crate) fn from_legacy_global_bytes(bytes: u64) -> Self {
        Self {
            text_bytes: bytes,
            image_bytes: bytes,
            video_bytes: bytes,
            audio_bytes: bytes,
            archive_bytes: bytes,
            document_bytes: bytes,
            sqlite_bytes: bytes,
        }
    }
}

pub(crate) fn normalize_preview_directory_expand_levels(levels: u8) -> u8 {
    levels.clamp(
        MIN_PREVIEW_DIRECTORY_EXPAND_LEVELS,
        MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS,
    )
}

/// 把用户输入规范成可匹配的后缀：去首尾空白、去前导点、转小写。
/// 含内部空白（如 "my ext"）永远无法命中真实文件名，直接拒绝。
/// 复合后缀（如 tar.gz）保留内部点。
pub(crate) fn normalize_preview_extension(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('.');
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return None;
    }
    Some(trimmed.to_lowercase())
}

/// 空格预览的分类型后缀规则：每个类型的列表完全决定该类型识别哪些
/// 后缀（替换式）。匹配按文件名 `ends_with` 进行，天然覆盖 tar.gz
/// 这类复合后缀；大小写不敏感，与各渲染器行为保持一致。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PreviewExtensionRules {
    pub(crate) text: Vec<String>,
    pub(crate) image: Vec<String>,
    pub(crate) video: Vec<String>,
    pub(crate) audio: Vec<String>,
    pub(crate) sqlite: Vec<String>,
    pub(crate) archive: Vec<String>,
    pub(crate) document: Vec<String>,
}

impl PreviewExtensionRules {
    pub(crate) fn default_rules() -> Self {
        Self {
            text: default_extensions(&DEFAULT_PREVIEW_TEXT_EXTENSIONS),
            image: default_extensions(&DEFAULT_PREVIEW_IMAGE_EXTENSIONS),
            video: default_extensions(&DEFAULT_PREVIEW_VIDEO_EXTENSIONS),
            audio: default_extensions(&DEFAULT_PREVIEW_AUDIO_EXTENSIONS),
            sqlite: default_extensions(&DEFAULT_PREVIEW_SQLITE_EXTENSIONS),
            archive: default_extensions(&DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS),
            document: default_extensions(&DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS),
        }
    }

    pub(crate) fn matches(&self, kind: PreviewFileSizeKind, path: &Path) -> bool {
        extensions_match(self.list(kind), path)
    }

    pub(crate) fn list(&self, kind: PreviewFileSizeKind) -> &Vec<String> {
        match kind {
            PreviewFileSizeKind::Text => &self.text,
            PreviewFileSizeKind::Image => &self.image,
            PreviewFileSizeKind::Video => &self.video,
            PreviewFileSizeKind::Audio => &self.audio,
            PreviewFileSizeKind::Archive => &self.archive,
            PreviewFileSizeKind::Document => &self.document,
            PreviewFileSizeKind::Sqlite => &self.sqlite,
        }
    }

    pub(crate) fn list_mut(&mut self, kind: PreviewFileSizeKind) -> &mut Vec<String> {
        match kind {
            PreviewFileSizeKind::Text => &mut self.text,
            PreviewFileSizeKind::Image => &mut self.image,
            PreviewFileSizeKind::Video => &mut self.video,
            PreviewFileSizeKind::Audio => &mut self.audio,
            PreviewFileSizeKind::Archive => &mut self.archive,
            PreviewFileSizeKind::Document => &mut self.document,
            PreviewFileSizeKind::Sqlite => &mut self.sqlite,
        }
    }

    pub(crate) fn set_list(&mut self, kind: PreviewFileSizeKind, extensions: Vec<String>) {
        *self.list_mut(kind) = extensions;
    }

    pub(crate) fn default_list(kind: PreviewFileSizeKind) -> Vec<String> {
        let builtin: &[&str] = match kind {
            PreviewFileSizeKind::Text => &DEFAULT_PREVIEW_TEXT_EXTENSIONS,
            PreviewFileSizeKind::Image => &DEFAULT_PREVIEW_IMAGE_EXTENSIONS,
            PreviewFileSizeKind::Video => &DEFAULT_PREVIEW_VIDEO_EXTENSIONS,
            PreviewFileSizeKind::Audio => &DEFAULT_PREVIEW_AUDIO_EXTENSIONS,
            PreviewFileSizeKind::Archive => &DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS,
            PreviewFileSizeKind::Document => &DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS,
            PreviewFileSizeKind::Sqlite => &DEFAULT_PREVIEW_SQLITE_EXTENSIONS,
        };
        default_extensions(builtin)
    }
}

fn default_extensions(builtin: &[&str]) -> Vec<String> {
    builtin
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect()
}

fn extensions_match(extensions: &[String], path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_lowercase();
    extensions
        .iter()
        .any(|candidate| file_name.ends_with(&format!(".{candidate}")))
}

pub(crate) fn preview_size_limit_mib(bytes: u64) -> u64 {
    bytes.div_ceil(PREVIEW_FILE_SIZE_UNIT_BYTES)
}

pub(crate) fn preview_size_limit_bytes_from_mib(mib: u64) -> Option<u64> {
    mib.checked_mul(PREVIEW_FILE_SIZE_UNIT_BYTES)
}

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
    pub(crate) startup_location_policy: StartupLocationPolicy,
    pub(crate) startup_custom_directory: PathBuf,
    pub(crate) save_view_state: bool,
    pub(crate) shortcuts: ShortcutConfig,
    pub(crate) launch_window_policy: LaunchWindowPolicy,
    pub(crate) context_menus: ContextMenuPreferences,
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
        startup_location_policy: StartupLocationPolicy::Home,
        startup_custom_directory: fallback_base.clone(),
        save_view_state: false,
        launch_window_policy: DEFAULT_LAUNCH_WINDOW_POLICY,
        shortcuts: ShortcutConfig::defaults(),
        context_menus: crate::model::ContextMenuPreferences::defaults(),
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
        startup_location_policy: StartupLocationPolicy::Home,
        startup_custom_directory: PathBuf::new(),
        save_view_state: false,
        shortcuts: ShortcutConfig::defaults(),
        context_menus: ContextMenuPreferences::defaults(),
        launch_window_policy: DEFAULT_LAUNCH_WINDOW_POLICY,
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
