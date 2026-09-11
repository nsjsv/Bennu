use std::path::{Path, PathBuf};

use desktop_linux::{NetworkConnection, NetworkConnectionId, NetworkProtocol, TerminalEmulator};
use file_core::FileOperationVerification;
use file_operation_store::{
    StoreResult, StoredContextMenuItemEntry, StoredContextMenuLayout, StoredContextMenuLayouts,
    StoredListViewColumn, StoredNetworkConnection, StoredPath, StoredPreviewExtensionRules,
    StoredShortcutBinding, StoredSidebarFavorite, StoredUserPreferences,
    StoredWindowControlPlacement, TaskQueueStore,
};

use super::app_config::AppConfig;
use super::legacy_toml;
use super::startup::StartupLocationPolicy;
use super::{
    app_config_dir_path, browser_view_mode_config_value, browser_view_mode_from_config_value,
    default_state_database_path, default_user_config, file_operation_verification_config_value,
    file_operation_verification_from_config_value, list_directory_size_display_mode_config_value,
    list_directory_size_display_mode_from_config_value, normalize_preview_directory_expand_levels,
    normalize_right_preview_panel_width, normalize_right_preview_preview_ratio,
    normalize_sidebar_width, normalize_visible_column_count,
    sort_direction_config_value, sort_direction_from_config_value, sort_field_config_value,
    sort_field_from_config_value, ColumnWidthAdjustMode, LaunchWindowPolicy,
    PreviewExtensionRules, PreviewFileSizeLimits, SidebarFavoriteConfig, UiLanguageSetting,
    UserConfig, ViewDensityLevel,
};
use crate::matugen_theme::{ColorSchemePreset, CustomColorScheme, ThemeMode};
use crate::model::{
    list_column_kind_config_value, list_column_kind_from_config_value, BrowserViewMode,
    ContextMenuLayoutConfigValues, ContextMenuPreferences, ListColumnConfig,
    ListDirectorySizeDisplayMode, ListSortPreference, ListViewPreferences, WindowChromeLayout,
    WindowControlKind, WindowControlPlacement, WindowControlSide, WindowControlVisibility,
    WindowControlsConfig,
};
use crate::network_connections::SavedNetworkConnection;
use crate::shortcuts::ShortcutConfig;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UserPreferences {
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
    pub(crate) terminal_shell: String,
    pub(crate) file_operation_verification: FileOperationVerification,
    pub(crate) browser_view_mode: BrowserViewMode,
    pub(crate) visible_column_count: usize,
    pub(crate) window_controls: WindowControlsConfig,
    pub(crate) icon_grid_size: u32,
    pub(crate) columns_view_density: ViewDensityLevel,
    pub(crate) list_view_density: ViewDensityLevel,
    pub(crate) icons_view_density: ViewDensityLevel,
    pub(crate) list_view_preferences: ListViewPreferences,
    pub(crate) list_directory_size_display_mode: ListDirectorySizeDisplayMode,
    pub(crate) startup_location_policy: StartupLocationPolicy,
    pub(crate) launch_window_policy: LaunchWindowPolicy,
    pub(crate) column_width_adjust_mode: ColumnWidthAdjustMode,
    pub(crate) startup_custom_directory: PathBuf,
    pub(crate) save_view_state: bool,
    pub(crate) shortcuts: ShortcutConfig,
    pub(crate) search_history: crate::model::SearchHistory,
    pub(crate) theme_mode: ThemeMode,
    pub(crate) color_scheme: ColorSchemePreset,
    pub(crate) custom_color_scheme: CustomColorScheme,
    pub(crate) context_menus: ContextMenuPreferences,
}

impl UserPreferences {
    fn icons_view_density_from_user_config(config: &UserConfig) -> ViewDensityLevel {
        // 旧调用点可能只更新 icon_grid_size，默认档位时仍需由兼容字段迁移。
        let legacy_level = ViewDensityLevel::from_icon_grid_size(config.icon_grid_size);
        if config.icons_view_density == ViewDensityLevel::DEFAULT
            && legacy_level != ViewDensityLevel::DEFAULT
        {
            legacy_level
        } else {
            config.icons_view_density
        }
    }

    pub(crate) fn from_user_config(config: &UserConfig) -> Self {
        let icons_view_density = Self::icons_view_density_from_user_config(config);
        Self {
            network_list_thumbnail_downloads_enabled: config
                .network_list_thumbnail_downloads_enabled,
            preview_size_limits: config.preview_size_limits,
            preview_directory_expand_levels: config.preview_directory_expand_levels,
            preview_extension_rules: config.preview_extension_rules.clone(),
            show_hidden_files: config.show_hidden_files,
            language_setting: config.language_setting,
            sidebar_width: normalize_sidebar_width(config.sidebar_width),
            right_preview_panel_open: config.right_preview_panel_open,
            right_preview_panel_width: normalize_right_preview_panel_width(
                config.right_preview_panel_width,
            ),
            right_preview_preview_ratio: normalize_right_preview_preview_ratio(
                config.right_preview_preview_ratio,
            ),
            sidebar_favorites: config.sidebar_favorites.clone(),
            network_connections: config.network_connections.clone(),
            terminal_emulator: config.terminal_emulator,
            terminal_shell: config.terminal_shell.clone(),
            file_operation_verification: config.file_operation_verification,
            browser_view_mode: config.browser_view_mode,
            visible_column_count: normalize_visible_column_count(config.visible_column_count),
            window_controls: config.window_controls.clone(),
            icon_grid_size: icons_view_density.icon_grid_size(),
            columns_view_density: config.columns_view_density,
            list_view_density: config.list_view_density,
            icons_view_density,
            list_view_preferences: config.list_view_preferences.clone(),
            list_directory_size_display_mode: config.list_directory_size_display_mode,
            startup_location_policy: config.startup_location_policy,
            launch_window_policy: config.launch_window_policy,
            column_width_adjust_mode: config.column_width_adjust_mode,
            startup_custom_directory: config.startup_custom_directory.clone(),
            save_view_state: config.startup_location_policy.saves_view_state(),
            shortcuts: config.shortcuts.clone(),
            search_history: config.search_history.clone(),
            theme_mode: config.theme_mode,
            color_scheme: config.color_scheme,
            custom_color_scheme: config.custom_color_scheme.clone(),
            context_menus: config.context_menus.clone(),
        }
    }

    pub(crate) fn apply_to_user_config(&self, config: &mut UserConfig) {
        config.network_list_thumbnail_downloads_enabled =
            self.network_list_thumbnail_downloads_enabled;
        config.preview_size_limits = self.preview_size_limits;
        config.preview_directory_expand_levels = self.preview_directory_expand_levels;
        config.preview_extension_rules = self.preview_extension_rules.clone();
        config.show_hidden_files = self.show_hidden_files;
        config.language_setting = self.language_setting;
        config.sidebar_width = self.sidebar_width;
        config.right_preview_panel_open = self.right_preview_panel_open;
        config.right_preview_panel_width = self.right_preview_panel_width;
        config.right_preview_preview_ratio = self.right_preview_preview_ratio;
        config.sidebar_favorites = self.sidebar_favorites.clone();
        config.network_connections = self.network_connections.clone();
        config.terminal_emulator = self.terminal_emulator;
        config.terminal_shell = self.terminal_shell.clone();
        config.file_operation_verification = self.file_operation_verification;
        config.browser_view_mode = self.browser_view_mode;
        config.visible_column_count = normalize_visible_column_count(self.visible_column_count);
        config.window_controls = self.window_controls.clone();
        config.columns_view_density = self.columns_view_density;
        config.list_view_density = self.list_view_density;
        config.icons_view_density = self.icons_view_density;
        config.icon_grid_size = self.icons_view_density.icon_grid_size();
        config.list_view_preferences = self.list_view_preferences.clone();
        config.list_directory_size_display_mode = self.list_directory_size_display_mode;
        config.startup_location_policy = self.startup_location_policy;
        config.launch_window_policy = self.launch_window_policy;
        config.column_width_adjust_mode = self.column_width_adjust_mode;
        config.startup_custom_directory = self.startup_custom_directory.clone();
        config.save_view_state = self.startup_location_policy.saves_view_state();
        config.shortcuts = self.shortcuts.clone();
        config.search_history = self.search_history.clone();
        config.theme_mode = self.theme_mode;
        config.color_scheme = self.color_scheme;
        config.custom_color_scheme = self.custom_color_scheme.clone();
        config.context_menus = self.context_menus.clone();
    }

    pub(crate) fn to_stored(&self) -> StoredUserPreferences {
        let mut stored = StoredUserPreferences::default();
        stored.network_list_thumbnail_downloads_enabled =
            self.network_list_thumbnail_downloads_enabled;
        stored.max_preview_file_bytes = None;
        stored.preview_text_size_bytes = Some(self.preview_size_limits.text_bytes);
        stored.preview_image_size_bytes = Some(self.preview_size_limits.image_bytes);
        stored.preview_video_size_bytes = Some(self.preview_size_limits.video_bytes);
        stored.preview_audio_size_bytes = Some(self.preview_size_limits.audio_bytes);
        stored.preview_archive_size_bytes = Some(self.preview_size_limits.archive_bytes);
        stored.preview_document_size_bytes = Some(self.preview_size_limits.document_bytes);
        stored.preview_sqlite_size_bytes = Some(self.preview_size_limits.sqlite_bytes);
        stored.preview_extension_rules = Some(StoredPreviewExtensionRules {
            text: Some(self.preview_extension_rules.text.clone()),
            image: Some(self.preview_extension_rules.image.clone()),
            video: Some(self.preview_extension_rules.video.clone()),
            audio: Some(self.preview_extension_rules.audio.clone()),
            sqlite: Some(self.preview_extension_rules.sqlite.clone()),
            archive: Some(self.preview_extension_rules.archive.clone()),
            document: Some(self.preview_extension_rules.document.clone()),
        });
        stored.preview_directory_expand_levels = Some(self.preview_directory_expand_levels);
        stored.show_hidden_files = self.show_hidden_files;
        stored.language_setting = self.language_setting.config_value().to_owned();
        stored.sidebar_width = f64::from(normalize_sidebar_width(self.sidebar_width));
        stored.right_preview_panel_open = self.right_preview_panel_open;
        stored.right_preview_panel_width = Some(f64::from(normalize_right_preview_panel_width(
            self.right_preview_panel_width,
        )));
        stored.right_preview_preview_ratio = Some(f64::from(
            normalize_right_preview_preview_ratio(self.right_preview_preview_ratio),
        ));
        stored.sidebar_favorites = self
            .sidebar_favorites
            .as_ref()
            .map(|favorites| stored_sidebar_favorites(favorites));
        stored.network_connections = stored_network_connections(&self.network_connections);
        stored.terminal_emulator = self.terminal_emulator.config_value().to_owned();
        stored.terminal_shell = Some(self.terminal_shell.clone());
        stored.file_operation_verification =
            file_operation_verification_config_value(self.file_operation_verification).to_owned();
        stored.browser_view_mode =
            browser_view_mode_config_value(self.browser_view_mode).to_owned();
        stored.visible_column_count =
            normalize_visible_column_count(self.visible_column_count) as u32;
        stored.window_chrome_layout = self.window_controls.layout().config_value().to_owned();
        stored.window_controls = stored_window_controls(&self.window_controls);
        stored.columns_view_density = Some(self.columns_view_density.index());
        stored.list_view_density = Some(self.list_view_density.index());
        stored.icons_view_density = Some(self.icons_view_density.index());
        stored.icon_grid_size = self.icons_view_density.icon_grid_size();
        stored.list_view_columns = stored_list_view_columns(&self.list_view_preferences);
        stored.list_sort_field =
            sort_field_config_value(self.list_view_preferences.sort().field).to_owned();
        stored.list_sort_direction =
            sort_direction_config_value(self.list_view_preferences.sort().direction).to_owned();
        stored.list_directory_size_display_mode =
            list_directory_size_display_mode_config_value(self.list_directory_size_display_mode)
                .to_owned();
        stored.startup_location = self.startup_location_policy.config_value().to_owned();
        stored.launch_window_policy = self.launch_window_policy.config_value().to_owned();
        stored.column_width_adjust_mode = self.column_width_adjust_mode.config_value().to_owned();
        stored.startup_custom_directory = StoredPath::from_path(&self.startup_custom_directory);
        stored.save_view_state = self.startup_location_policy.saves_view_state();
        stored.shortcuts = stored_shortcuts(&self.shortcuts);
        stored.search_history = self.search_history.entries().to_vec();
        stored.theme_mode = self.theme_mode.config_value().to_owned();
        stored.color_scheme = self.color_scheme.config_value().to_owned();
        stored.custom_color_scheme = Some(self.custom_color_scheme.to_stored());
        stored.context_menu_layouts = Some(stored_context_menu_layouts(&self.context_menus));
        stored
    }

    pub(crate) fn from_stored(stored: StoredUserPreferences, default: &UserConfig) -> Self {
        let default_preferences = Self::from_user_config(default);
        let (theme_mode, color_scheme) = match (
            ThemeMode::from_config_value(&stored.theme_mode),
            ColorSchemePreset::from_config_value(&stored.color_scheme),
        ) {
            (Some(theme_mode), Some(color_scheme)) => (theme_mode, color_scheme),
            _ => (
                default_preferences.theme_mode,
                default_preferences.color_scheme,
            ),
        };
        let startup_location_policy =
            StartupLocationPolicy::from_config_value(&stored.startup_location)
                .unwrap_or(default_preferences.startup_location_policy);
        let list_view_preferences =
            list_view_preferences_from_stored(&stored, &default_preferences);
        let custom_color_scheme = CustomColorScheme::from_stored(
            stored.custom_color_scheme.as_ref(),
            &default_preferences.custom_color_scheme,
        );
        let window_controls = window_controls_from_stored(&stored, &default_preferences);
        let context_menus = context_menu_preferences_from_stored(&stored, &default_preferences);
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
            .unwrap_or_else(|| ViewDensityLevel::from_icon_grid_size(stored.icon_grid_size));
        Self {
            network_list_thumbnail_downloads_enabled: stored
                .network_list_thumbnail_downloads_enabled,
            preview_size_limits: preview_size_limits_from_stored(&stored, &default_preferences),
            preview_directory_expand_levels: stored.preview_directory_expand_levels.map_or(
                default_preferences.preview_directory_expand_levels,
                normalize_preview_directory_expand_levels,
            ),
            // 每类型独立回退：None 是旧版本数据或缺失类型；空列表是
            // 用户显式清空，必须原样保留。
            preview_extension_rules: preview_extension_rules_from_stored(
                &stored,
                &default_preferences,
            ),
            show_hidden_files: stored.show_hidden_files,
            language_setting: UiLanguageSetting::from_config_value(&stored.language_setting)
                .unwrap_or(default_preferences.language_setting),
            sidebar_width: normalize_sidebar_width(stored.sidebar_width as f32),
            right_preview_panel_open: stored.right_preview_panel_open,
            // 宽度缺省/非法值都回退默认;非法存储值按“从未设置过”处理。
            right_preview_panel_width: stored
                .right_preview_panel_width
                .map(|width| normalize_right_preview_panel_width(width as f32))
                .unwrap_or(default_preferences.right_preview_panel_width),
            right_preview_preview_ratio: stored
                .right_preview_preview_ratio
                .map(|ratio| normalize_right_preview_preview_ratio(ratio as f32))
                .unwrap_or(default_preferences.right_preview_preview_ratio),
            sidebar_favorites: stored
                .sidebar_favorites
                .map(|favorites| sidebar_favorites_from_stored(&favorites)),
            network_connections: network_connections_from_stored(&stored.network_connections),
            terminal_emulator: TerminalEmulator::from_config_value(&stored.terminal_emulator)
                .unwrap_or(default_preferences.terminal_emulator),
            terminal_shell: stored
                .terminal_shell
                .clone()
                .unwrap_or(default_preferences.terminal_shell.clone()),
            file_operation_verification: file_operation_verification_from_config_value(
                &stored.file_operation_verification,
            )
            .unwrap_or(default_preferences.file_operation_verification),
            browser_view_mode: browser_view_mode_from_config_value(&stored.browser_view_mode)
                .unwrap_or(default_preferences.browser_view_mode),
            visible_column_count: normalize_visible_column_count(
                stored.visible_column_count as usize,
            ),
            window_controls,
            icon_grid_size: icons_view_density.icon_grid_size(),
            columns_view_density,
            list_view_density,
            icons_view_density,
            list_view_preferences,
            list_directory_size_display_mode: list_directory_size_display_mode_from_config_value(
                &stored.list_directory_size_display_mode,
            )
            .unwrap_or(default_preferences.list_directory_size_display_mode),
            startup_location_policy,
            launch_window_policy: LaunchWindowPolicy::from_config_value(
                &stored.launch_window_policy,
            )
            .unwrap_or(default_preferences.launch_window_policy),
            column_width_adjust_mode: ColumnWidthAdjustMode::from_config_value(
                &stored.column_width_adjust_mode,
            )
            .unwrap_or(default_preferences.column_width_adjust_mode),
            startup_custom_directory: stored.startup_custom_directory.to_path_buf(),
            save_view_state: startup_location_policy.saves_view_state(),
            shortcuts: shortcut_config_from_stored(&stored.shortcuts),
            search_history: crate::model::SearchHistory::from_persisted(
                stored.search_history.clone(),
            ),
            theme_mode,
            color_scheme,
            custom_color_scheme,
            context_menus,
        }
    }
}

fn preview_extension_rules_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
) -> PreviewExtensionRules {
    let fallback = &default.preview_extension_rules;
    let Some(stored_rules) = &stored.preview_extension_rules else {
        return fallback.clone();
    };
    let per_type = |stored: &Option<Vec<String>>, default_extensions: &Vec<String>| {
        stored.clone().unwrap_or_else(|| default_extensions.clone())
    };
    PreviewExtensionRules {
        text: per_type(&stored_rules.text, &fallback.text),
        image: per_type(&stored_rules.image, &fallback.image),
        video: per_type(&stored_rules.video, &fallback.video),
        audio: per_type(&stored_rules.audio, &fallback.audio),
        sqlite: per_type(&stored_rules.sqlite, &fallback.sqlite),
        archive: per_type(&stored_rules.archive, &fallback.archive),
        document: per_type(&stored_rules.document, &fallback.document),
    }
}

fn preview_size_limits_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
) -> PreviewFileSizeLimits {
    let legacy_global_limit = stored.max_preview_file_bytes;
    let limit = |stored_bytes: Option<u64>, default_bytes: u64| {
        stored_bytes
            .or(legacy_global_limit)
            .unwrap_or(default_bytes)
    };
    PreviewFileSizeLimits {
        text_bytes: limit(
            stored.preview_text_size_bytes,
            default.preview_size_limits.text_bytes,
        ),
        image_bytes: limit(
            stored.preview_image_size_bytes,
            default.preview_size_limits.image_bytes,
        ),
        video_bytes: limit(
            stored.preview_video_size_bytes,
            default.preview_size_limits.video_bytes,
        ),
        audio_bytes: limit(
            stored.preview_audio_size_bytes,
            default.preview_size_limits.audio_bytes,
        ),
        archive_bytes: limit(
            stored.preview_archive_size_bytes,
            default.preview_size_limits.archive_bytes,
        ),
        document_bytes: limit(
            stored.preview_document_size_bytes,
            default.preview_size_limits.document_bytes,
        ),
        sqlite_bytes: limit(
            stored.preview_sqlite_size_bytes,
            default.preview_size_limits.sqlite_bytes,
        ),
    }
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

pub(crate) fn load_user_config_for_app_config(app_config: AppConfig) -> UserConfig {
    let state_database_path = default_state_database_path();
    let config_dir = app_config_dir_path();
    load_user_config_from_sources(app_config, &state_database_path, config_dir.as_deref())
}

pub(super) fn load_user_config_from_sources(
    app_config: AppConfig,
    state_database_path: &Path,
    config_dir: Option<&Path>,
) -> UserConfig {
    let mut default = default_user_config();
    app_config.apply_to_user_config(&mut default);

    let Ok(store) = TaskQueueStore::new(state_database_path) else {
        return default;
    };

    match store.read_user_preferences() {
        Ok(Some(stored)) => {
            let preferences = UserPreferences::from_stored(stored, &default);
            UserConfig::from_parts(app_config, preferences)
        }
        Ok(None) => {
            let migrated = config_dir.map_or_else(
                || default.clone(),
                |config_dir| {
                    legacy_toml::load_legacy_user_config_from_dir(config_dir, default.clone())
                },
            );
            let _ = store.replace_user_preferences(&migrated.user_preferences().to_stored());
            migrated
        }
        Err(_) => default,
    }
}

pub(crate) fn save_user_preferences(
    store: &TaskQueueStore,
    preferences: &UserPreferences,
) -> StoreResult<()> {
    store.replace_user_preferences(&preferences.to_stored())
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

fn sidebar_favorites_from_stored(
    favorites: &[StoredSidebarFavorite],
) -> Vec<SidebarFavoriteConfig> {
    favorites
        .iter()
        .map(|favorite| SidebarFavoriteConfig {
            label: favorite.label.clone(),
            path: favorite.path.to_path_buf(),
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

fn network_connections_from_stored(
    connections: &[StoredNetworkConnection],
) -> Vec<SavedNetworkConnection> {
    let mut restored = Vec::new();
    for stored in connections {
        let Some(protocol) = NetworkProtocol::from_config_value(&stored.protocol) else {
            continue;
        };
        if let Ok(connection) = NetworkConnection::new(
            NetworkConnectionId::new(stored.id.trim()),
            stored.label.trim().to_owned(),
            protocol,
            &stored.uri,
        ) {
            restored.push(SavedNetworkConnection::new(connection, stored.auto_connect));
        }
    }
    restored
}

fn stored_context_menu_layouts(
    preferences: &ContextMenuPreferences,
) -> StoredContextMenuLayouts {
    let values = preferences.to_config_values();
    let layout = |items: &Vec<(String, bool)>| StoredContextMenuLayout {
        items: items
            .iter()
            .map(|(id, visible)| StoredContextMenuItemEntry {
                id: id.clone(),
                visible: *visible,
            })
            .collect(),
    };
    StoredContextMenuLayouts {
        file_entry: layout(&values.file_entry),
        file_blank: layout(&values.file_blank),
        trash: layout(&values.trash),
        search: layout(&values.search),
        search_entry_types: layout(&values.search_entry_types),
        list_columns: layout(&values.list_columns),
        sidebar_bookmark: layout(&values.sidebar_bookmark),
        sidebar_device: layout(&values.sidebar_device),
        network_connection: layout(&values.network_connection),
    }
}

fn context_menu_preferences_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
) -> ContextMenuPreferences {
    let Some(layouts) = &stored.context_menu_layouts else {
        return default.context_menus.clone();
    };
    let config_values = |layout: &StoredContextMenuLayout| {
        layout
            .items
            .iter()
            .map(|entry| (entry.id.clone(), entry.visible))
            .collect::<Vec<(String, bool)>>()
    };
    ContextMenuPreferences::from_config_values(&ContextMenuLayoutConfigValues {
        file_entry: config_values(&layouts.file_entry),
        file_blank: config_values(&layouts.file_blank),
        trash: config_values(&layouts.trash),
        search: config_values(&layouts.search),
        search_entry_types: config_values(&layouts.search_entry_types),
        list_columns: config_values(&layouts.list_columns),
        sidebar_bookmark: config_values(&layouts.sidebar_bookmark),
        sidebar_device: config_values(&layouts.sidebar_device),
        network_connection: config_values(&layouts.network_connection),
    })
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
        .collect();
    let sort = ListSortPreference {
        field: sort_field_from_config_value(&stored.list_sort_field)
            .unwrap_or(default.list_view_preferences.sort().field),
        direction: sort_direction_from_config_value(&stored.list_sort_direction)
            .unwrap_or(default.list_view_preferences.sort().direction),
    };
    ListViewPreferences::new(columns, sort)
}

fn window_controls_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
) -> WindowControlsConfig {
    let layout = WindowChromeLayout::from_config_value(&stored.window_chrome_layout)
        .unwrap_or(default.window_controls.layout());
    let placements = stored
        .window_controls
        .iter()
        .filter_map(|placement| {
            let kind = WindowControlKind::from_config_value(&placement.kind)?;
            let side = WindowControlSide::from_config_value(&placement.side)
                .unwrap_or_else(|| default.window_controls.placement(kind).side());
            Some(WindowControlPlacement::new(
                kind,
                side,
                WindowControlVisibility::from(placement.visible),
            ))
        })
        .collect();
    WindowControlsConfig::from_partial_placements(layout, placements)
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

fn stored_shortcuts(shortcuts: &ShortcutConfig) -> Vec<StoredShortcutBinding> {
    shortcuts
        .toml_table()
        .into_iter()
        .filter_map(|(action_key, value)| {
            let binding = value.as_str()?.to_owned();
            Some(StoredShortcutBinding {
                action_key,
                binding,
            })
        })
        .collect()
}

fn shortcut_config_from_stored(shortcuts: &[StoredShortcutBinding]) -> ShortcutConfig {
    let mut config = ShortcutConfig::defaults();
    let table = shortcuts
        .iter()
        .map(|binding| {
            (
                binding.action_key.clone(),
                toml::Value::String(binding.binding.clone()),
            )
        })
        .collect();
    config.apply_toml_table(&table);
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUSTOM_JSON: &str = r##"{
        "version": 1,
        "light": {
            "background": "#ffffff",
            "surface": "#f6f8fa",
            "text": "#1f2328",
            "muted_text": "#59636e",
            "primary": "#0969da",
            "success": "#1a7f37",
            "warning": "#9a6700",
            "danger": "#d1242f"
        },
        "dark": {
            "background": "#0d1117",
            "surface": "#151b23",
            "text": "#f0f6fc",
            "muted_text": "#9198a1",
            "primary": "#4493f8",
            "success": "#3fb950",
            "warning": "#d29922",
            "danger": "#f85149"
        }
    }"##;

    #[test]
    fn custom_color_scheme_roundtrips_and_invalid_modes_fall_back_independently() {
        let default = default_user_config();
        let imported = CustomColorScheme::from_json(CUSTOM_JSON).expect("valid custom scheme");
        let mut config = default.clone();
        config.custom_color_scheme = imported.clone();
        config.color_scheme = ColorSchemePreset::Custom;

        let stored = config.user_preferences().to_stored();
        let restored = UserPreferences::from_stored(stored, &default);
        assert_eq!(restored.custom_color_scheme, imported);
        assert_eq!(restored.color_scheme, ColorSchemePreset::Custom);

        let mut partially_invalid = config.user_preferences().to_stored();
        let custom = partially_invalid
            .custom_color_scheme
            .as_mut()
            .expect("stored custom scheme");
        custom.dark.as_mut().expect("stored dark set").text = "bad".to_owned();
        let restored = UserPreferences::from_stored(partially_invalid, &default);
        assert_eq!(restored.custom_color_scheme.light, imported.light);
        assert_eq!(
            restored.custom_color_scheme.dark,
            default.custom_color_scheme.dark
        );
    }

    #[test]
    fn missing_custom_snapshot_uses_default_without_affecting_old_selection() {
        let mut stored = StoredUserPreferences::default();
        stored.color_scheme = "custom".to_owned();
        let restored = UserPreferences::from_stored(stored, &default_user_config());
        assert_eq!(restored.color_scheme, ColorSchemePreset::Custom);
        assert_eq!(
            restored.custom_color_scheme,
            default_user_config().custom_color_scheme
        );
    }

    #[test]
    fn theme_selection_roundtrips_through_stored_preferences() {
        for color_scheme in ColorSchemePreset::ALL {
            let mut config = default_user_config();
            config.theme_mode = ThemeMode::Dark;
            config.color_scheme = color_scheme;

            let stored = config.user_preferences().to_stored();
            let restored = UserPreferences::from_stored(stored, &default_user_config());

            assert_eq!(restored.theme_mode, ThemeMode::Dark);
            assert_eq!(restored.color_scheme, color_scheme);
        }
    }

    #[test]
    fn right_preview_panel_roundtrips_and_missing_width_falls_back() {
        let default = default_user_config();
        let mut config = default.clone();
        config.right_preview_panel_open = true;
        config.right_preview_panel_width = 480.0;
        config.right_preview_preview_ratio = 0.8;

        let stored = config.user_preferences().to_stored();
        assert_eq!(stored.right_preview_panel_open, true);
        assert_eq!(stored.right_preview_panel_width, Some(480.0));
        // f32 比例升 f64 存储,只保留精度容差内相等。
        assert!((stored.right_preview_preview_ratio.unwrap() - 0.8).abs() < 1e-6);
        let restored = UserPreferences::from_stored(stored, &default);
        assert!(restored.right_preview_panel_open);
        assert_eq!(restored.right_preview_panel_width, 480.0);
        assert_eq!(restored.right_preview_preview_ratio, 0.8);

        // 旧版本数据缺宽度字段:回退默认宽度,开关回退关闭。
        let mut legacy = StoredUserPreferences::default();
        legacy.right_preview_panel_width = None;
        legacy.right_preview_preview_ratio = None;
        let restored = UserPreferences::from_stored(legacy, &default);
        assert!(!restored.right_preview_panel_open);
        assert_eq!(
            restored.right_preview_panel_width,
            default.right_preview_panel_width
        );
        assert_eq!(
            restored.right_preview_preview_ratio,
            default.right_preview_preview_ratio
        );
    }

    #[test]
    fn invalid_theme_selection_falls_back_to_current_defaults() {
        for (theme_mode, color_scheme) in [("sepia", "claude"), ("dark", "unknown")] {
            let mut stored = StoredUserPreferences::default();
            stored.theme_mode = theme_mode.to_owned();
            stored.color_scheme = color_scheme.to_owned();

            let restored = UserPreferences::from_stored(stored, &default_user_config());

            assert_eq!(restored.theme_mode, ThemeMode::Automatic);
            assert_eq!(restored.color_scheme, ColorSchemePreset::Default);
        }
    }

    #[test]
    fn density_levels_roundtrip_independently_with_icon_mirror() {
        let default = default_user_config();
        let mut config = default.clone();
        config.columns_view_density = ViewDensityLevel::from_index(1);
        config.list_view_density = ViewDensityLevel::from_index(4);
        config.icons_view_density = ViewDensityLevel::from_index(6);
        config.icon_grid_size = config.icons_view_density.icon_grid_size();

        let stored = config.user_preferences().to_stored();
        assert_eq!(
            (
                stored.columns_view_density,
                stored.list_view_density,
                stored.icons_view_density,
                stored.icon_grid_size,
            ),
            (Some(1), Some(4), Some(6), 160)
        );

        let restored = UserPreferences::from_stored(stored, &default);
        assert_eq!(restored.columns_view_density.index(), 1);
        assert_eq!(restored.list_view_density.index(), 4);
        assert_eq!(restored.icons_view_density.index(), 6);
        assert_eq!(restored.icon_grid_size, 160);
    }

    #[test]
    fn density_boundary_migrates_legacy_values_and_clamps_new_indexes() {
        let default = default_user_config();
        let mut legacy_config = default.clone();
        legacy_config.icon_grid_size = 160;
        assert_eq!(
            legacy_config.user_preferences().icons_view_density.index(),
            6
        );

        let mut stored = default.user_preferences().to_stored();
        stored.columns_view_density = None;
        stored.list_view_density = None;
        stored.icons_view_density = None;
        stored.icon_grid_size = 120;
        let migrated = UserPreferences::from_stored(stored, &default);
        assert_eq!(migrated.columns_view_density, ViewDensityLevel::DEFAULT);
        assert_eq!(migrated.list_view_density, ViewDensityLevel::DEFAULT);
        assert_eq!(migrated.icons_view_density.index(), 4);
        assert_eq!(migrated.icon_grid_size, 128);

        let mut stored = migrated.to_stored();
        stored.columns_view_density = Some(u8::MAX);
        stored.list_view_density = Some(0);
        stored.icons_view_density = Some(u8::MAX);
        stored.icon_grid_size = 64;
        let normalized = UserPreferences::from_stored(stored, &default);
        assert_eq!(normalized.columns_view_density.index(), 8);
        assert_eq!(normalized.list_view_density.index(), 0);
        assert_eq!(normalized.icons_view_density.index(), 8);
        assert_eq!(normalized.to_stored().icon_grid_size, 192);

        let mut stored = normalized.to_stored();
        stored.icons_view_density = Some(1);
        stored.icon_grid_size = 192;
        let preferred = UserPreferences::from_stored(stored, &default);
        assert_eq!(preferred.icons_view_density.index(), 1);
        assert_eq!(preferred.icon_grid_size, 80);
    }
}
