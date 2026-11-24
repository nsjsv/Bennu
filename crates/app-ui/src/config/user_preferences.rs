use std::path::PathBuf;

use bennu_preview::preview_config::{StoredPreviewExtensionRules, StoredPreviewPreferences};
use bennu_sidebar::{StoredNetworkConnection, StoredSidebarFavorite};
use bennu_state::{PreferencesChange, StoreResult, StoredPath, UserSettingsStore};
use desktop_linux::TerminalEmulator;
use file_core::FileOperationVerification;

use super::app_config::AppConfig;
use super::convert_defaults::StoredConvertDefaults;
use super::startup::StartupLocationPolicy;
use super::unrecognized_settings::UnrecognizedSettings;
use super::{
    browser_view_mode_config_value, browser_view_mode_from_config_value, default_user_config,
    file_operation_verification_config_value, file_operation_verification_from_config_value,
    list_directory_size_display_mode_config_value,
    list_directory_size_display_mode_from_config_value, normalize_sidebar_width,
    normalize_visible_column_count,
    sort_direction_config_value, sort_direction_from_config_value, sort_field_config_value,
    sort_field_from_config_value, transfer_port_from, transfer_receive_pin_from,
    ColumnWidthAdjustMode, FavoriteTransferDevice, LaunchWindowPolicy, PreviewExtensionRules,
    PreviewFileSizeLimits, SidebarFavoriteConfig, StoredLastSearchScope, StoredListViewColumn,
    StoredUserPreferences, StoredWindowControlPlacement, TransferAcceptMode, UiLanguageSetting,
    UserConfig, ViewDensityLevel,
};
use crate::model::{
    list_column_kind_config_value, list_column_kind_from_config_value, BrowserViewMode,
    ContextMenuPreferences, FileGroupingMode, LastSearchScope, ListColumnConfig,
    ListDirectorySizeDisplayMode, ListSortPreference, ListViewPreferences, WindowControlKind,
    WindowControlPlacement, WindowControlSide, WindowControlVisibility, WindowControlsConfig,
};
use crate::network_connections::SavedNetworkConnection;
use crate::info_panel::InfoPanelPreferences;
use crate::right_dock::RightDockLayout;
use crate::shortcuts::ShortcutConfig;
use bennu_preview::preview_config::{
    media_preview_volume_from_stored, media_preview_volume_percent,
    preview_directory_expand_levels_from_stored, preview_extension_rules_from_stored,
    preview_size_limits_from_stored, MediaEndAction,
};
use bennu_preview::text_view::TextViewSettings;
use bennu_theme::window_controls::SidebarStyle;
use bennu_theme::{ColorSchemePreset, ThemeMode};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UserPreferences {
    pub(crate) network_list_thumbnail_downloads_enabled: bool,
    pub(crate) preview_size_limits: PreviewFileSizeLimits,
    pub(crate) preview_directory_expand_levels: u8,
    pub(crate) preview_extension_rules: PreviewExtensionRules,
    pub(crate) text_preview_view: TextViewSettings,
    pub(crate) media_preview_volume: f32,
    pub(crate) media_end_action: MediaEndAction,
    pub(crate) show_hidden_files: bool,
    pub(crate) language_setting: UiLanguageSetting,
    pub(crate) sidebar_width: f32,
    pub(crate) right_dock: RightDockLayout,
    pub(crate) info_panel: InfoPanelPreferences,
    pub(crate) sidebar_favorites: Option<Vec<SidebarFavoriteConfig>>,
    pub(crate) network_connections: Vec<SavedNetworkConnection>,
    pub(crate) terminal_emulator: TerminalEmulator,
    pub(crate) terminal_shell: String,
    pub(crate) interface_font: String,
    pub(crate) monospace_font: String,
    pub(crate) interface_text_size: u16,
    pub(crate) file_operation_verification: FileOperationVerification,
    pub(crate) browser_view_mode: BrowserViewMode,
    pub(crate) visible_column_count: usize,
    pub(crate) window_controls: WindowControlsConfig,
    pub(crate) sidebar_style: SidebarStyle,
    pub(crate) sidebar_visible: bool,
    pub(crate) columns_view_density: ViewDensityLevel,
    pub(crate) list_view_density: ViewDensityLevel,
    pub(crate) icons_view_density: ViewDensityLevel,
    pub(crate) list_view_preferences: ListViewPreferences,
    pub(crate) list_directory_size_display_mode: ListDirectorySizeDisplayMode,
    pub(crate) file_grouping: FileGroupingMode,
    pub(crate) startup_location_policy: StartupLocationPolicy,
    pub(crate) launch_window_policy: LaunchWindowPolicy,
    pub(crate) column_width_adjust_mode: ColumnWidthAdjustMode,
    pub(crate) startup_custom_directory: PathBuf,
    pub(crate) save_view_state: bool,
    pub(crate) shortcuts: ShortcutConfig,
    pub(crate) letter_mode: crate::shortcuts::LetterMode,
    pub(crate) search_history: crate::model::SearchHistory,
    pub(crate) super_box_history: crate::super_box::SuperBoxHistory,
    pub(crate) super_box_mode: crate::super_box::BoxMode,
    pub(crate) super_box_found_in_columns: crate::super_box::FoundInColumns,
    pub(crate) smart_folders: Vec<String>,
    pub(crate) last_search_scope: Option<LastSearchScope>,
    pub(crate) theme_mode: ThemeMode,
    pub(crate) color_scheme: ColorSchemePreset,
    pub(crate) context_menus: ContextMenuPreferences,
    pub(crate) transfer_download_dir: Option<PathBuf>,
    pub(crate) transfer_device_alias: Option<String>,
    pub(crate) transfer_favorite_devices: Vec<FavoriteTransferDevice>,
    pub(crate) transfer_receive_enabled: bool,
    pub(crate) transfer_port: u16,
    pub(crate) transfer_accept_mode: TransferAcceptMode,
    pub(crate) transfer_receive_pin: Option<String>,
    pub(crate) convert_defaults: crate::model::ConvertDefaults,
}

impl UserPreferences {
    pub(crate) fn from_user_config(config: &UserConfig) -> Self {
        Self {
            network_list_thumbnail_downloads_enabled: config
                .network_list_thumbnail_downloads_enabled,
            preview_size_limits: config.preview_size_limits,
            preview_directory_expand_levels: config.preview_directory_expand_levels,
            preview_extension_rules: config.preview_extension_rules.clone(),
            text_preview_view: config.text_preview_view,
            media_preview_volume: config.media_preview_volume,
            media_end_action: config.media_end_action,
            show_hidden_files: config.show_hidden_files,
            language_setting: config.language_setting,
            sidebar_width: normalize_sidebar_width(config.sidebar_width),
            right_dock: config.right_dock.clone(),
            info_panel: config.info_panel.clone(),
            sidebar_favorites: config.sidebar_favorites.clone(),
            network_connections: config.network_connections.clone(),
            terminal_emulator: config.terminal_emulator,
            terminal_shell: config.terminal_shell.clone(),
            interface_font: config.interface_font.clone(),
            monospace_font: config.monospace_font.clone(),
            interface_text_size: config.interface_text_size,
            file_operation_verification: config.file_operation_verification,
            browser_view_mode: config.browser_view_mode,
            visible_column_count: normalize_visible_column_count(config.visible_column_count),
            window_controls: config.window_controls.clone(),
            sidebar_style: config.sidebar_style,
            sidebar_visible: config.sidebar_visible,
            columns_view_density: config.columns_view_density,
            list_view_density: config.list_view_density,
            icons_view_density: config.icons_view_density,
            list_view_preferences: config.list_view_preferences.clone(),
            list_directory_size_display_mode: config.list_directory_size_display_mode,
            file_grouping: config.file_grouping,
            startup_location_policy: config.startup_location_policy,
            launch_window_policy: config.launch_window_policy,
            column_width_adjust_mode: config.column_width_adjust_mode,
            startup_custom_directory: config.startup_custom_directory.clone(),
            save_view_state: config.startup_location_policy.saves_view_state(),
            shortcuts: config.shortcuts.clone(),
            letter_mode: config.letter_mode,
            search_history: config.search_history.clone(),
            super_box_history: config.super_box_history.clone(),
            super_box_mode: config.super_box_mode,
            super_box_found_in_columns: config.super_box_found_in_columns,
            smart_folders: config.smart_folders.clone(),
            last_search_scope: config.last_search_scope.clone(),
            theme_mode: config.theme_mode,
            color_scheme: config.color_scheme,
            context_menus: config.context_menus.clone(),
            transfer_download_dir: config.transfer_download_dir.clone(),
            transfer_device_alias: config.transfer_device_alias.clone(),
            transfer_favorite_devices: config.transfer_favorite_devices.clone(),
            transfer_receive_enabled: config.transfer_receive_enabled,
            transfer_port: config.transfer_port,
            transfer_accept_mode: config.transfer_accept_mode,
            transfer_receive_pin: config.transfer_receive_pin.clone(),
            convert_defaults: config.convert_defaults.clone(),
        }
    }

    pub(crate) fn apply_to_user_config(&self, config: &mut UserConfig) {
        config.network_list_thumbnail_downloads_enabled =
            self.network_list_thumbnail_downloads_enabled;
        config.preview_size_limits = self.preview_size_limits;
        config.preview_directory_expand_levels = self.preview_directory_expand_levels;
        config.preview_extension_rules = self.preview_extension_rules.clone();
        config.text_preview_view = self.text_preview_view;
        config.media_preview_volume = self.media_preview_volume;
        config.media_end_action = self.media_end_action;
        config.show_hidden_files = self.show_hidden_files;
        config.language_setting = self.language_setting;
        config.sidebar_width = self.sidebar_width;
        config.right_dock = self.right_dock.clone();
        config.info_panel = self.info_panel.clone();
        config.sidebar_favorites = self.sidebar_favorites.clone();
        config.network_connections = self.network_connections.clone();
        config.terminal_emulator = self.terminal_emulator;
        config.terminal_shell = self.terminal_shell.clone();
        config.interface_font = self.interface_font.clone();
        config.monospace_font = self.monospace_font.clone();
        config.interface_text_size = self.interface_text_size;
        config.file_operation_verification = self.file_operation_verification;
        config.browser_view_mode = self.browser_view_mode;
        config.visible_column_count = normalize_visible_column_count(self.visible_column_count);
        config.window_controls = self.window_controls.clone();
        config.sidebar_style = self.sidebar_style;
        config.sidebar_visible = self.sidebar_visible;
        config.columns_view_density = self.columns_view_density;
        config.list_view_density = self.list_view_density;
        config.icons_view_density = self.icons_view_density;
        config.list_view_preferences = self.list_view_preferences.clone();
        config.list_directory_size_display_mode = self.list_directory_size_display_mode;
        config.file_grouping = self.file_grouping;
        config.startup_location_policy = self.startup_location_policy;
        config.launch_window_policy = self.launch_window_policy;
        config.column_width_adjust_mode = self.column_width_adjust_mode;
        config.startup_custom_directory = self.startup_custom_directory.clone();
        config.save_view_state = self.startup_location_policy.saves_view_state();
        config.shortcuts = self.shortcuts.clone();
        config.letter_mode = self.letter_mode;
        config.search_history = self.search_history.clone();
        config.super_box_history = self.super_box_history.clone();
        config.super_box_mode = self.super_box_mode;
        config.super_box_found_in_columns = self.super_box_found_in_columns;
        config.smart_folders = self.smart_folders.clone();
        config.last_search_scope = self.last_search_scope.clone();
        config.theme_mode = self.theme_mode;
        config.color_scheme = self.color_scheme;
        config.context_menus = self.context_menus.clone();
        config.transfer_download_dir = self.transfer_download_dir.clone();
        config.transfer_device_alias = self.transfer_device_alias.clone();
        config.transfer_favorite_devices = self.transfer_favorite_devices.clone();
        config.transfer_receive_enabled = self.transfer_receive_enabled;
        config.transfer_port = self.transfer_port;
        config.transfer_accept_mode = self.transfer_accept_mode;
        config.transfer_receive_pin = self.transfer_receive_pin.clone();
        config.convert_defaults = self.convert_defaults.clone();
    }

    pub(crate) fn to_stored(&self) -> StoredUserPreferences {
        let mut stored = StoredUserPreferences::default();
        stored.network_list_thumbnail_downloads_enabled =
            self.network_list_thumbnail_downloads_enabled;
        // 新代码写分类型键，legacy 全局键写 None（只读迁移，见
        // bennu-preview::preview_config 的迁移语义）。
        stored.preview = StoredPreviewPreferences {
            max_preview_file_bytes: None,
            preview_text_size_bytes: None,
            preview_image_size_bytes: Some(self.preview_size_limits.image_bytes),
            preview_video_size_bytes: None,
            preview_audio_size_bytes: None,
            preview_archive_size_bytes: None,
            preview_document_size_bytes: Some(self.preview_size_limits.document_bytes),
            preview_sqlite_size_bytes: Some(self.preview_size_limits.sqlite_bytes),
            preview_extension_rules: Some(StoredPreviewExtensionRules {
                text: None,
                image: None,
                video: None,
                audio: None,
                sqlite: Some(self.preview_extension_rules.sqlite.clone()),
                archive: Some(self.preview_extension_rules.archive.clone()),
                document: Some(self.preview_extension_rules.document.clone()),
            }),
            preview_directory_expand_levels: Some(self.preview_directory_expand_levels),
            preview_extra_text_extensions: Some(self.preview_extension_rules.text.clone()),
            text_preview_zoom_percent: Some(self.text_preview_view.zoom_percent),
            text_preview_wrap: Some(self.text_preview_view.wrap),
            preview_extra_audio_extensions: Some(self.preview_extension_rules.audio.clone()),
            media_preview_volume_percent: Some(media_preview_volume_percent(
                self.media_preview_volume,
            )),
            audio_end_action: Some(self.media_end_action.stored_name().to_owned()),
            preview_extra_video_extensions: Some(self.preview_extension_rules.video.clone()),
            preview_extra_image_extensions: Some(self.preview_extension_rules.image.clone()),
        };
        stored.show_hidden_files = self.show_hidden_files;
        stored.language_setting = self.language_setting.config_value().to_owned();
        stored.sidebar_width = f64::from(normalize_sidebar_width(self.sidebar_width));
        stored.right_dock = Some(right_dock_to_stored(&self.right_dock));
        stored.info_panel = Some(info_panel_to_stored(&self.info_panel));
        stored.sidebar_favorites = self
            .sidebar_favorites
            .as_ref()
            .map(|favorites| stored_sidebar_favorites(favorites));
        stored.network_connections = stored_network_connections(&self.network_connections);
        stored.terminal_emulator = self.terminal_emulator.config_value().to_owned();
        stored.terminal_shell = Some(self.terminal_shell.clone());
        stored.interface_font = Some(self.interface_font.clone());
        stored.monospace_font = Some(self.monospace_font.clone());
        stored.interface_text_size = Some(self.interface_text_size);
        stored.file_operation_verification =
            file_operation_verification_config_value(self.file_operation_verification).to_owned();
        stored.browser_view_mode =
            browser_view_mode_config_value(self.browser_view_mode).to_owned();
        stored.visible_column_count =
            normalize_visible_column_count(self.visible_column_count) as u32;
        stored.window_controls = stored_window_controls(&self.window_controls);
        stored.sidebar_style = self.sidebar_style.config_value().to_owned();
        stored.sidebar_visible = self.sidebar_visible;
        stored.columns_view_density = Some(self.columns_view_density.index());
        stored.list_view_density = Some(self.list_view_density.index());
        stored.icons_view_density = Some(self.icons_view_density.index());
        stored.list_view_columns = stored_list_view_columns(&self.list_view_preferences);
        stored.list_sort_field =
            sort_field_config_value(self.list_view_preferences.sort().field).to_owned();
        stored.list_sort_direction =
            sort_direction_config_value(self.list_view_preferences.sort().direction).to_owned();
        stored.list_directory_size_display_mode =
            list_directory_size_display_mode_config_value(self.list_directory_size_display_mode)
                .to_owned();
        stored.file_grouping = Some(self.file_grouping.config_value().to_owned());
        stored.startup_location = self.startup_location_policy.config_value().to_owned();
        stored.launch_window_policy = self.launch_window_policy.config_value().to_owned();
        stored.column_width_adjust_mode = self.column_width_adjust_mode.config_value().to_owned();
        stored.startup_custom_directory = StoredPath::from_path(&self.startup_custom_directory);
        stored.save_view_state = self.startup_location_policy.saves_view_state();
        stored.shortcut_overrides = Some(stored_shortcut_overrides(&self.shortcuts));
        stored.letter_keys = self.letter_mode.config_value().to_owned();
        stored.search_history = self.search_history.entries().to_vec();
        stored.super_box_history = self.super_box_history.entries().to_vec();
        stored.super_box_mode = Some(self.super_box_mode.config_value().to_owned());
        stored.super_box_found_in_columns =
            Some(self.super_box_found_in_columns.config_value().to_owned());
        stored.smart_folders = self.smart_folders.clone();
        stored.last_search_scope = self.last_search_scope.as_ref().map(|scope| match scope {
            LastSearchScope::Global => StoredLastSearchScope::Global,
            LastSearchScope::Directory(path) => StoredLastSearchScope::Directory {
                path: StoredPath::from_path(path),
            },
        });
        stored.theme_mode = self.theme_mode.config_value().to_owned();
        stored.color_scheme = self.color_scheme.config_value().to_owned();
        stored.context_menu_layouts = Some(stored_context_menu_layouts(&self.context_menus));
        stored.transfer_download_dir = self
            .transfer_download_dir
            .as_ref()
            .map(|dir| StoredPath::from_path(dir));
        stored.transfer_device_alias = self.transfer_device_alias.clone();
        stored.transfer_trusted_devices =
            stored_favorite_transfer_devices(&self.transfer_favorite_devices);
        stored.transfer_receive_enabled = self.transfer_receive_enabled;
        stored.transfer_port = self.transfer_port;
        stored.transfer_accept_mode = self.transfer_accept_mode.config_value().to_owned();
        stored.transfer_receive_pin = self.transfer_receive_pin.clone();
        stored.convert_defaults =
            Some(StoredConvertDefaults::from_defaults(&self.convert_defaults));
        stored
    }

    /// 把存储值转成界面设置；值不认识的项换成默认值，并在结果里列出这些项。
    pub(crate) fn from_stored(
        stored: StoredUserPreferences,
        default: &UserConfig,
    ) -> InterpretedPreferences {
        let default_preferences = Self::from_user_config(default);
        let mut unrecognized = UnrecognizedSettings::default();
        let stored_theme_mode = ThemeMode::from_config_value(&stored.theme_mode);
        let stored_color_scheme = ColorSchemePreset::from_stored_value(&stored.color_scheme);
        // 主题模式与配色要么一起用、要么一起回落：只认一半会拼出用户没选过的组合。
        if stored_theme_mode.is_none() {
            unrecognized.note("theme_mode");
        }
        if stored_color_scheme.is_none() {
            unrecognized.note("color_scheme");
        }
        let (theme_mode, color_scheme) = match (stored_theme_mode, stored_color_scheme) {
            (Some(theme_mode), Some(color_scheme)) => (theme_mode, color_scheme),
            _ => (
                default_preferences.theme_mode,
                default_preferences.color_scheme,
            ),
        };
        let startup_location_policy = unrecognized.value_or_default(
            "startup_location",
            StartupLocationPolicy::from_config_value(&stored.startup_location),
            default_preferences.startup_location_policy,
        );
        let list_view_preferences =
            list_view_preferences_from_stored(&stored, &default_preferences, &mut unrecognized);
        let window_controls =
            window_controls_from_stored(&stored, &default_preferences, &mut unrecognized);
        let sidebar_style = unrecognized.value_or_default(
            "sidebar_style",
            SidebarStyle::from_config_value(&stored.sidebar_style),
            default_preferences.sidebar_style,
        );
        let context_menus =
            context_menu_preferences_from_stored(&stored, &default_preferences, &mut unrecognized);
        let columns_view_density = stored
            .columns_view_density
            .map(ViewDensityLevel::from_index)
            .unwrap_or(default_preferences.columns_view_density);
        let list_view_density = stored
            .list_view_density
            .map(ViewDensityLevel::from_index)
            .unwrap_or(default_preferences.list_view_density);
        let icons_view_density = stored
            .icons_view_density
            .map(ViewDensityLevel::from_index)
            .or_else(|| {
                stored
                    .icon_grid_size
                    .map(ViewDensityLevel::from_icon_grid_size)
            })
            .unwrap_or(default_preferences.icons_view_density);
        // None = 旧版本数据未持久化过分组设置，不算读不懂；有值但不认识才算。
        let file_grouping = match stored.file_grouping.as_deref() {
            Some(value) => unrecognized.value_or_default(
                "file_grouping",
                FileGroupingMode::from_config_value(value),
                default_preferences.file_grouping,
            ),
            None => default_preferences.file_grouping,
        };
        let shortcuts = shortcut_config_from_stored(&stored, &mut unrecognized);
        let preferences = Self {
            network_list_thumbnail_downloads_enabled: stored
                .network_list_thumbnail_downloads_enabled,
            preview_size_limits: preview_size_limits_from_stored(
                &stored.preview,
                default_preferences.preview_size_limits,
            ),
            preview_directory_expand_levels: preview_directory_expand_levels_from_stored(
                &stored.preview,
                default_preferences.preview_directory_expand_levels,
            ),
            text_preview_view: TextViewSettings::from_stored(
                stored.preview.text_preview_zoom_percent,
                stored.preview.text_preview_wrap,
            ),
            media_preview_volume: media_preview_volume_from_stored(
                stored.preview.media_preview_volume_percent,
            ),
            media_end_action: MediaEndAction::from_stored(
                stored.preview.audio_end_action.as_deref(),
            ),
            preview_extension_rules: preview_extension_rules_from_stored(
                &stored.preview,
                &default_preferences.preview_extension_rules,
            ),
            show_hidden_files: stored.show_hidden_files,
            language_setting: unrecognized.value_or_default(
                "language_setting",
                UiLanguageSetting::from_config_value(&stored.language_setting),
                default_preferences.language_setting,
            ),
            sidebar_width: normalize_sidebar_width(stored.sidebar_width as f32),
            right_dock: right_dock_from_stored(&stored, &mut unrecognized),
            info_panel: info_panel_from_stored(&stored, &default_preferences, &mut unrecognized),
            sidebar_favorites: stored
                .sidebar_favorites
                .map(|favorites| bennu_sidebar::sidebar_favorites_from_stored(&favorites)),
            network_connections: network_connections_from_stored(
                &stored.network_connections,
                &mut unrecognized,
            ),
            terminal_emulator: unrecognized.value_or_default(
                "terminal_emulator",
                TerminalEmulator::from_config_value(&stored.terminal_emulator),
                default_preferences.terminal_emulator,
            ),
            terminal_shell: stored
                .terminal_shell
                .clone()
                .unwrap_or(default_preferences.terminal_shell.clone()),
            interface_font: stored
                .interface_font
                .clone()
                .unwrap_or(default_preferences.interface_font.clone()),
            monospace_font: stored
                .monospace_font
                .clone()
                .unwrap_or(default_preferences.monospace_font.clone()),
            interface_text_size: stored.interface_text_size.map_or(
                default_preferences.interface_text_size,
                bennu_theme::typography::nearest_text_size,
            ),
            file_operation_verification: unrecognized.value_or_default(
                "file_operation_verification",
                file_operation_verification_from_config_value(&stored.file_operation_verification),
                default_preferences.file_operation_verification,
            ),
            browser_view_mode: unrecognized.value_or_default(
                "browser_view_mode",
                browser_view_mode_from_config_value(&stored.browser_view_mode),
                default_preferences.browser_view_mode,
            ),
            visible_column_count: normalize_visible_column_count(
                stored.visible_column_count as usize,
            ),
            window_controls,
            sidebar_style,
            sidebar_visible: stored.sidebar_visible,
            columns_view_density,
            list_view_density,
            icons_view_density,
            list_view_preferences,
            list_directory_size_display_mode: unrecognized.value_or_default(
                "list_directory_size_display_mode",
                list_directory_size_display_mode_from_config_value(
                    &stored.list_directory_size_display_mode,
                ),
                default_preferences.list_directory_size_display_mode,
            ),
            file_grouping,
            startup_location_policy,
            launch_window_policy: unrecognized.value_or_default(
                "launch_window_policy",
                LaunchWindowPolicy::from_config_value(&stored.launch_window_policy),
                default_preferences.launch_window_policy,
            ),
            column_width_adjust_mode: unrecognized.value_or_default(
                "column_width_adjust_mode",
                ColumnWidthAdjustMode::from_config_value(&stored.column_width_adjust_mode),
                default_preferences.column_width_adjust_mode,
            ),
            startup_custom_directory: stored.startup_custom_directory.to_path_buf(),
            save_view_state: startup_location_policy.saves_view_state(),
            shortcuts,
            letter_mode: unrecognized.value_or_default(
                "letter_keys",
                crate::shortcuts::LetterMode::from_config_value(&stored.letter_keys),
                default_preferences.letter_mode,
            ),
            search_history: crate::model::SearchHistory::from_persisted(
                stored.search_history.clone(),
            ),
            super_box_history: crate::super_box::SuperBoxHistory::from_persisted(
                stored.super_box_history.clone(),
            ),
            smart_folders: stored.smart_folders.clone(),
            // None = 旧版本数据，用默认值。
            super_box_found_in_columns: match stored.super_box_found_in_columns.as_deref() {
                None => default_preferences.super_box_found_in_columns,
                Some(value) => unrecognized.value_or_default(
                    "super_box_found_in_columns",
                    crate::super_box::FoundInColumns::from_config_value(value),
                    default_preferences.super_box_found_in_columns,
                ),
            },
            // None = 旧版本数据，用默认的搜索。
            super_box_mode: match stored.super_box_mode.as_deref() {
                None => default_preferences.super_box_mode,
                Some(value) => unrecognized.value_or_default(
                    "super_box_mode",
                    crate::super_box::BoxMode::from_config_value(value),
                    default_preferences.super_box_mode,
                ),
            },
            last_search_scope: stored.last_search_scope.as_ref().map(|scope| match scope {
                StoredLastSearchScope::Global => LastSearchScope::Global,
                StoredLastSearchScope::Directory { path } => {
                    LastSearchScope::Directory(path.to_path_buf())
                }
            }),
            theme_mode,
            color_scheme,
            context_menus,
            transfer_download_dir: stored
                .transfer_download_dir
                .as_ref()
                .map(|dir| dir.to_path_buf()),
            transfer_device_alias: stored.transfer_device_alias.clone(),
            transfer_favorite_devices: favorite_transfer_devices_from_stored(
                &stored.transfer_trusted_devices,
                &mut unrecognized,
            ),
            transfer_receive_enabled: stored.transfer_receive_enabled,
            transfer_port: unrecognized.value_or_default(
                "transfer_port",
                transfer_port_from(stored.transfer_port),
                default_preferences.transfer_port,
            ),
            transfer_accept_mode: unrecognized.value_or_default(
                "transfer_accept_mode",
                TransferAcceptMode::from_config_value(&stored.transfer_accept_mode),
                default_preferences.transfer_accept_mode,
            ),
            transfer_receive_pin: stored.transfer_receive_pin.as_deref().and_then(|pin| {
                let recognized = transfer_receive_pin_from(pin);
                if recognized.is_none() {
                    unrecognized.note("transfer_receive_pin");
                }
                recognized
            }),
            convert_defaults: stored.convert_defaults.as_ref().map_or_else(
                || default_preferences.convert_defaults.clone(),
                |stored| stored.to_defaults(&mut unrecognized),
            ),
        };
        InterpretedPreferences {
            preferences,
            unrecognized,
        }
    }
}

/// `from_stored` 的结果：界面设置，以及值不认识、已换成默认值的项。
#[derive(Debug)]
pub(crate) struct InterpretedPreferences {
    pub(crate) preferences: UserPreferences,
    pub(crate) unrecognized: UnrecognizedSettings,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self::from_user_config(&default_user_config())
    }
}

impl UserConfig {
    pub(crate) fn from_parts(app_config: AppConfig, preferences: UserPreferences) -> Self {
        let mut config = default_user_config();
        app_config.apply_to_user_config(&mut config);
        preferences.apply_to_user_config(&mut config);
        config
    }

    pub(crate) fn user_preferences(&self) -> UserPreferences {
        UserPreferences::from_user_config(self)
    }
}

pub(crate) fn save_user_preferences(
    store: &UserSettingsStore,
    change: &PreferencesChange<UserPreferences>,
) -> StoreResult<()> {
    store.save_user_preferences(&stored_change(change))
}

pub(crate) fn stored_change(
    change: &PreferencesChange<UserPreferences>,
) -> PreferencesChange<StoredUserPreferences> {
    PreferencesChange {
        seen: change.seen.to_stored(),
        current: change.current.to_stored(),
    }
}

fn stored_sidebar_favorites(favorites: &[SidebarFavoriteConfig]) -> Vec<StoredSidebarFavorite> {
    favorites
        .iter()
        .map(|favorite| StoredSidebarFavorite {
            label: favorite.label.clone(),
            path: StoredPath::from_path(&favorite.path),
        })
        .collect()
}

fn stored_network_connections(
    connections: &[SavedNetworkConnection],
) -> Vec<StoredNetworkConnection> {
    connections
        .iter()
        .map(|saved| StoredNetworkConnection {
            id: saved.connection.id.as_str().to_owned(),
            label: saved.connection.label.clone(),
            protocol: saved.connection.protocol.config_value().to_owned(),
            uri: saved.connection.uri.clone(),
            auto_connect: saved.auto_connect,
        })
        .collect()
}

/// 解释与文件选择器共用（`bennu_sidebar`）；这里另记被丢弃的条目，供设置损坏提示。
fn network_connections_from_stored(
    connections: &[StoredNetworkConnection],
    unrecognized: &mut UnrecognizedSettings,
) -> Vec<SavedNetworkConnection> {
    let restored = bennu_sidebar::network_connections_from_stored(connections);
    unrecognized.note_dropped("network_connections", connections.len(), restored.len());
    restored
}

fn stored_list_view_columns(preferences: &ListViewPreferences) -> Vec<StoredListViewColumn> {
    preferences
        .columns()
        .iter()
        .map(|column| StoredListViewColumn {
            kind: list_column_kind_config_value(column.kind).to_owned(),
            width: f64::from(column.width),
            visible: column.visible,
        })
        .collect()
}

fn list_view_preferences_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> ListViewPreferences {
    let columns = stored
        .list_view_columns
        .iter()
        .filter_map(|column| {
            let kind = list_column_kind_from_config_value(&column.kind)?;
            Some(ListColumnConfig::new(
                kind,
                column.width as f32,
                column.visible,
            ))
        })
        .collect::<Vec<_>>();
    unrecognized.note_dropped(
        "list_view_columns",
        stored.list_view_columns.len(),
        columns.len(),
    );
    let sort = ListSortPreference {
        field: unrecognized.value_or_default(
            "list_sort_field",
            sort_field_from_config_value(&stored.list_sort_field),
            default.list_view_preferences.sort().field,
        ),
        direction: unrecognized.value_or_default(
            "list_sort_direction",
            sort_direction_from_config_value(&stored.list_sort_direction),
            default.list_view_preferences.sort().direction,
        ),
    };
    ListViewPreferences::new(columns, sort)
}

fn window_controls_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> WindowControlsConfig {
    let placements = stored
        .window_controls
        .iter()
        .filter_map(|placement| {
            let kind = WindowControlKind::from_config_value(&placement.kind)?;
            let side = unrecognized.value_or_default(
                "window_controls",
                WindowControlSide::from_config_value(&placement.side),
                default.window_controls.placement(kind).side(),
            );
            Some(WindowControlPlacement::new(
                kind,
                side,
                WindowControlVisibility::from(placement.visible),
            ))
        })
        .collect::<Vec<_>>();
    unrecognized.note_dropped(
        "window_controls",
        stored.window_controls.len(),
        placements.len(),
    );
    WindowControlsConfig::from_partial_placements(placements)
}

fn stored_window_controls(
    window_controls: &WindowControlsConfig,
) -> Vec<StoredWindowControlPlacement> {
    window_controls
        .placements()
        .iter()
        .map(|placement| StoredWindowControlPlacement {
            kind: placement.kind().config_value().to_owned(),
            side: placement.side().config_value().to_owned(),
            visible: placement.visibility().is_visible(),
        })
        .collect()
}

mod context_menu_storage;
use context_menu_storage::{context_menu_preferences_from_stored, stored_context_menu_layouts};
mod shortcut_storage;
use shortcut_storage::{shortcut_config_from_stored, stored_shortcut_overrides};
mod right_dock_storage;
use right_dock_storage::{
    info_panel_from_stored, info_panel_to_stored, right_dock_from_stored, right_dock_to_stored,
};
mod transfer_favorites;
use transfer_favorites::{favorite_transfer_devices_from_stored, stored_favorite_transfer_devices};

#[cfg(test)]
mod tests;
