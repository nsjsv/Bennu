use std::fs;
use std::path::PathBuf;

use desktop_linux::{
    DisplayRendererGpu, DisplayRendererGpuClass, NetworkConnection, NetworkConnectionId,
    NetworkProtocol, TerminalEmulator,
};
use file_core::{FileOperationVerification, SortDirection, SortField};
use file_operation_store::{
    StoredNetworkConnection, StoredUserPreferences, StoredWindowControlPlacement, TaskQueueStore,
};

use crate::model::{
    BrowserViewMode, ListColumnKind, ListDirectorySizeDisplayMode, WindowChromeLayout,
    WindowControlKind, WindowControlSide, WindowControlVisibility,
};
use crate::network_connections::SavedNetworkConnection;
use crate::shortcuts::ShortcutBindingId;

use super::*;

#[test]
fn parses_legacy_toml_user_config_for_migration() {
    let parsed = legacy_toml::parse_toml_user_config(
        r#"
thumbnail_cache_dir = "/tmp/thumbnails"
network_list_thumbnail_downloads_enabled = true
max_preview_file_bytes = 4194304
show_hidden_files = true
sidebar_width = 260.5
terminal_emulator = "ghostty"
rendering_backend = "gpu"
file_operation_verification = "strong"
browser_view_mode = "list"
startup_location = "custom"
startup_custom_directory = "/workspace"
save_view_state = true
shortcuts = { focus_path_input = "Ctrl+Alt+L" }
sidebar_favorites = [
  { label = "Downloads", path = "/home/user/Downloads" },
  { path = "/srv/projects" },
]
network_connections = [
  { id = "nas", label = "NAS", protocol = "smb", uri = "smb://server/share" },
  { id = "docs", label = "", protocol = "webdav", uri = "https://user@example.test/docs", auto_connect = true },
  { id = "sftp", label = "SFTP", protocol = "sftp", uri = "sftp://user@sftp.example.test/srv/share" },
]
"#,
        default_user_config(),
    );

    assert_eq!(parsed.thumbnail_cache_dir, PathBuf::from("/tmp/thumbnails"));
    assert!(parsed.network_list_thumbnail_downloads_enabled);
    assert_eq!(
        parsed.preview_size_limits,
        PreviewFileSizeLimits::from_legacy_global_bytes(4 * 1024 * 1024)
    );
    assert!(parsed.show_hidden_files);
    assert_eq!(parsed.language_setting, UiLanguageSetting::System);
    assert_eq!(parsed.sidebar_width, 260.5);
    assert_eq!(parsed.terminal_emulator, TerminalEmulator::Ghostty);
    assert_eq!(
        parsed.rendering_gpu_preference,
        RenderingGpuPreference::HighPerformanceGpu
    );
    assert_eq!(
        parsed.file_operation_verification,
        FileOperationVerification::Strong
    );
    assert_eq!(parsed.browser_view_mode, BrowserViewMode::List);
    assert_eq!(
        parsed.startup_location_policy,
        StartupLocationPolicy::CustomDirectory
    );
    assert_eq!(parsed.startup_custom_directory, PathBuf::from("/workspace"));
    assert!(!parsed.save_view_state);
    assert_eq!(
        parsed
            .shortcuts
            .binding(ShortcutBindingId::FocusPathInput)
            .config_value(),
        "Ctrl+Alt+L"
    );
    assert_eq!(parsed.sidebar_favorites.as_ref().map(Vec::len), Some(2));
    assert_eq!(parsed.network_connections.len(), 3);
    assert_eq!(
        parsed.network_connections[1].connection.uri,
        "davs://user@example.test/docs"
    );
    assert!(parsed.network_connections[1].auto_connect);
    assert_eq!(
        parsed.list_directory_size_display_mode,
        ListDirectorySizeDisplayMode::ItemCount
    );
}

#[test]
fn app_config_toml_contains_only_startup_level_keys() {
    let app_config = AppConfig {
        thumbnail_cache_dir: PathBuf::from("/tmp/thumbnails"),
        rendering_gpu_preference: RenderingGpuPreference::HighPerformanceGpu,
        search_content_indexing_enabled: false,
        search_max_extract_bytes: 4096,
        renderer_probe_cache: None,
    };

    let content = app_config::toml_app_config_content(&app_config).unwrap();
    let document = content.parse::<toml::Table>().unwrap();

    assert_eq!(document.len(), 4);
    assert_eq!(
        document
            .get("thumbnail_cache_dir")
            .and_then(toml::Value::as_str),
        Some("/tmp/thumbnails")
    );
    assert_eq!(
        document
            .get("rendering_backend")
            .and_then(toml::Value::as_str),
        Some("gpu")
    );
    assert_eq!(
        document
            .get("search_content_indexing_enabled")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        document
            .get("search_max_extract_bytes")
            .and_then(toml::Value::as_integer),
        Some(4096)
    );
    for key in [
        "show_hidden_files",
        "sidebar_width",
        "sidebar_favorites",
        "network_connections",
        "shortcuts",
        "startup_location",
        "max_preview_file_bytes",
        "list_directory_size_display_mode",
    ] {
        assert!(
            !document.contains_key(key),
            "app config must not contain {key}"
        );
    }

    let parsed = app_config::parse_toml_app_config(&content, app_config::default_app_config());
    assert_eq!(parsed, app_config);
}

#[test]
fn writes_app_config_without_user_preferences() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let path = temp_dir.path().join("config.toml");
    let app_config = AppConfig {
        thumbnail_cache_dir: temp_dir.path().join("thumbnails"),
        rendering_gpu_preference: RenderingGpuPreference::DisplayGpu,
        search_content_indexing_enabled: true,
        search_max_extract_bytes: 8192,
        renderer_probe_cache: None,
    };

    app_config::write_app_config(&path, &app_config).expect("write app config");

    let content = fs::read_to_string(path).expect("read app config");
    assert!(content.starts_with("# File Manager application configuration\n"));
    assert!(content.contains("rendering_backend = \"display\""));
    assert!(content.contains("search_content_indexing_enabled = true"));
    assert!(content.contains("search_max_extract_bytes = 8192"));
    assert!(!content.contains("show_hidden_files"));
    assert!(!content.contains("shortcuts"));
}

#[test]
fn user_preferences_round_trip_through_sqlite() {
    let temp_dir = tempfile::tempdir().expect("create temp state dir");
    let state_database_path = temp_dir.path().join("state.sqlite");
    let store = TaskQueueStore::new(&state_database_path).expect("create state store");
    let app_config = AppConfig {
        thumbnail_cache_dir: PathBuf::from("/var/cache/file-manager/thumbs"),
        rendering_gpu_preference: RenderingGpuPreference::HighPerformanceGpu,
        search_content_indexing_enabled: false,
        search_max_extract_bytes: 1234,
        renderer_probe_cache: None,
    };
    let mut config = default_user_config();
    config.show_hidden_files = true;
    config.language_setting = UiLanguageSetting::Chinese;
    config.sidebar_width = 245.0;
    config.sidebar_favorites = Some(vec![SidebarFavoriteConfig {
        label: "Projects".to_owned(),
        path: PathBuf::from("/srv/projects"),
    }]);
    config.network_connections = vec![SavedNetworkConnection::new(
        NetworkConnection::new(
            NetworkConnectionId::new("nas"),
            "NAS",
            NetworkProtocol::Smb,
            "smb://server/share",
        )
        .unwrap(),
        true,
    )];
    config.startup_location_policy = StartupLocationPolicy::PreviousSession;
    config.startup_custom_directory = PathBuf::from("/workspace");
    config.save_view_state = config.startup_location_policy.saves_view_state();
    config.browser_view_mode = BrowserViewMode::List;
    config
        .window_controls
        .select_layout(WindowChromeLayout::SeparateTitleBar);
    config
        .window_controls
        .set_visibility(WindowControlKind::Minimize, WindowControlVisibility::Hidden);
    config
        .window_controls
        .move_to_side(WindowControlKind::Close, WindowControlSide::Left);
    config.icon_grid_size = 160;
    config
        .list_view_preferences
        .set_column_visible(ListColumnKind::Kind, false);
    config
        .list_view_preferences
        .set_column_visible(ListColumnKind::Owner, true);
    config
        .list_view_preferences
        .set_column_width(ListColumnKind::Name, 280.0);
    config
        .list_view_preferences
        .set_column_width(ListColumnKind::Owner, 140.0);
    config
        .list_view_preferences
        .move_column_right(ListColumnKind::Name);
    config
        .list_view_preferences
        .move_column_to(ListColumnKind::Owner, ListColumnKind::Name);
    config
        .list_view_preferences
        .select_sort_column(ListColumnKind::Size);
    config.list_directory_size_display_mode = ListDirectorySizeDisplayMode::RecursiveTotalSize;
    config.terminal_emulator = TerminalEmulator::Ghostty;
    config.file_operation_verification = FileOperationVerification::Strong;
    config.network_list_thumbnail_downloads_enabled = true;
    config
        .preview_size_limits
        .set_limit(crate::config::PreviewFileSizeKind::Video, 8 * 1024 * 1024);
    config.search_history.record_submission("report");
    config.search_history.record_submission("images");
    let mut shortcut_table = toml::Table::new();
    shortcut_table.insert(
        "focus_path_input".to_owned(),
        toml::Value::String("Ctrl+Alt+L".to_owned()),
    );
    config.shortcuts.apply_toml_table(&shortcut_table);

    save_user_preferences(&store, &config.user_preferences()).expect("save preferences");

    let loaded = user_preferences::load_user_config_from_sources(
        app_config.clone(),
        &state_database_path,
        None,
    );

    assert_eq!(loaded.thumbnail_cache_dir, app_config.thumbnail_cache_dir);
    assert_eq!(
        loaded.rendering_gpu_preference,
        app_config.rendering_gpu_preference
    );
    assert!(loaded.show_hidden_files);
    assert_eq!(loaded.language_setting, UiLanguageSetting::Chinese);
    assert_eq!(loaded.sidebar_width, 245.0);
    assert_eq!(loaded.sidebar_favorites, config.sidebar_favorites);
    assert_eq!(loaded.network_connections, config.network_connections);
    assert_eq!(
        loaded.startup_location_policy,
        StartupLocationPolicy::PreviousSession
    );
    assert_eq!(loaded.startup_custom_directory, PathBuf::from("/workspace"));
    assert!(loaded.save_view_state);
    assert_eq!(loaded.browser_view_mode, BrowserViewMode::List);
    assert_eq!(loaded.window_controls, config.window_controls);
    assert_eq!(loaded.icon_grid_size, 160);
    let loaded_columns = loaded
        .list_view_preferences
        .columns()
        .iter()
        .map(|column| (column.kind, column.visible, column.width))
        .collect::<Vec<_>>();
    assert_eq!(loaded_columns[0].0, ListColumnKind::Modified);
    assert_eq!(loaded_columns[1].0, ListColumnKind::Owner);
    assert!(loaded_columns[1].1);
    assert_eq!(loaded_columns[1].2, 140.0);
    assert_eq!(loaded_columns[2].0, ListColumnKind::Name);
    assert_eq!(loaded_columns[2].2, 280.0);
    assert!(loaded
        .list_view_preferences
        .columns()
        .iter()
        .find(|column| column.kind == ListColumnKind::Kind)
        .is_some_and(|column| !column.visible));
    assert_eq!(loaded.list_view_preferences.sort().field, SortField::Size);
    assert_eq!(
        loaded.list_view_preferences.sort().direction,
        SortDirection::Ascending
    );
    assert_eq!(
        loaded.list_directory_size_display_mode,
        ListDirectorySizeDisplayMode::RecursiveTotalSize
    );
    assert_eq!(loaded.terminal_emulator, TerminalEmulator::Ghostty);
    assert_eq!(
        loaded.file_operation_verification,
        FileOperationVerification::Strong
    );
    assert!(loaded.network_list_thumbnail_downloads_enabled);
    assert_eq!(loaded.preview_size_limits.video_bytes, 8 * 1024 * 1024);
    assert_eq!(loaded.search_history.entries(), ["images", "report"]);
    assert_eq!(
        loaded
            .shortcuts
            .binding(ShortcutBindingId::FocusPathInput)
            .config_value(),
        "Ctrl+Alt+L"
    );
}

#[test]
fn stored_window_controls_are_normalized_at_config_boundary() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.window_chrome_layout = "future-layout".to_owned();
    stored.window_controls = vec![
        StoredWindowControlPlacement {
            kind: "close".to_owned(),
            side: "left".to_owned(),
            visible: false,
        },
        StoredWindowControlPlacement {
            kind: "close".to_owned(),
            side: "right".to_owned(),
            visible: true,
        },
        StoredWindowControlPlacement {
            kind: "minimize".to_owned(),
            side: "invalid".to_owned(),
            visible: false,
        },
        StoredWindowControlPlacement {
            kind: "future-control".to_owned(),
            side: "left".to_owned(),
            visible: true,
        },
    ];

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(
        preferences.window_controls.layout(),
        WindowChromeLayout::IntegratedNavigation
    );
    assert_eq!(preferences.window_controls.placements().len(), 3);
    let close = preferences
        .window_controls
        .placement(WindowControlKind::Close);
    assert_eq!(close.side(), WindowControlSide::Left);
    assert!(close.visibility().is_visible());
    let minimize = preferences
        .window_controls
        .placement(WindowControlKind::Minimize);
    assert_eq!(minimize.side(), WindowControlSide::Right);
    assert_eq!(minimize.visibility(), WindowControlVisibility::Hidden);
    assert_eq!(
        preferences
            .window_controls
            .placement(WindowControlKind::MaximizeRestore)
            .side(),
        WindowControlSide::Right
    );
}

#[test]
fn stored_preferences_drop_removed_list_columns_and_restore_supported_defaults() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.list_view_columns = vec![
        file_operation_store::StoredListViewColumn {
            kind: "name".to_owned(),
            width: 280.0,
            visible: true,
        },
        file_operation_store::StoredListViewColumn {
            kind: "extension".to_owned(),
            width: 150.0,
            visible: true,
        },
        file_operation_store::StoredListViewColumn {
            kind: "size".to_owned(),
            width: 88.0,
            visible: true,
        },
    ];

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(
        preferences
            .list_view_preferences
            .columns()
            .iter()
            .map(|column| column.kind)
            .collect::<Vec<_>>(),
        vec![
            ListColumnKind::Name,
            ListColumnKind::Size,
            ListColumnKind::Modified,
            ListColumnKind::Kind,
            ListColumnKind::Owner,
            ListColumnKind::Group,
            ListColumnKind::Permissions,
            ListColumnKind::Accessed,
            ListColumnKind::Created,
        ]
    );
    assert!(preferences
        .list_view_preferences
        .columns()
        .iter()
        .find(|column| column.kind == ListColumnKind::Name)
        .is_some_and(|column| column.visible));
}

#[test]
fn stored_preferences_skip_password_bearing_network_connections() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.network_connections = vec![
        StoredNetworkConnection {
            id: "bad".to_owned(),
            label: "Bad".to_owned(),
            protocol: "webdav".to_owned(),
            uri: "davs://user:secret@example.test/docs".to_owned(),
            auto_connect: true,
        },
        StoredNetworkConnection {
            id: "good".to_owned(),
            label: "Good".to_owned(),
            protocol: "smb".to_owned(),
            uri: "smb://server/share".to_owned(),
            auto_connect: false,
        },
    ];

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(preferences.network_connections.len(), 1);
    assert_eq!(
        preferences.network_connections[0].connection.id.as_str(),
        "good"
    );
}

#[test]
fn stored_preferences_derive_view_state_saving_from_startup_location() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.startup_location = "custom".to_owned();
    stored.save_view_state = true;

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(
        preferences.startup_location_policy,
        StartupLocationPolicy::CustomDirectory
    );
    assert!(!preferences.save_view_state);
}

#[test]
fn stored_preferences_invalid_language_setting_falls_back_to_system() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.language_setting = "unsupported".to_owned();

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(preferences.language_setting, UiLanguageSetting::System);
}

#[test]
fn stored_preferences_default_list_directory_size_mode_is_item_count() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.list_directory_size_display_mode = "unknown".to_owned();

    let preferences = UserPreferences::from_stored(stored, &default);

    assert_eq!(
        preferences.list_directory_size_display_mode,
        ListDirectorySizeDisplayMode::ItemCount
    );
}

#[test]
fn icon_grid_mode_uses_stable_config_value() {
    assert_eq!(
        browser_view_mode_from_config_value("icons"),
        Some(BrowserViewMode::Icons)
    );
    assert_eq!(
        browser_view_mode_config_value(BrowserViewMode::Icons),
        "icons"
    );
}

#[test]
fn stored_icon_grid_size_is_normalized_at_config_boundary() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.icons_view_density = None;
    stored.icon_grid_size = 1;
    assert_eq!(
        UserPreferences::from_stored(stored.clone(), &default).icon_grid_size,
        MIN_ICON_GRID_SIZE
    );

    stored.icon_grid_size = u32::MAX;
    assert_eq!(
        UserPreferences::from_stored(stored, &default).icon_grid_size,
        MAX_ICON_GRID_SIZE
    );
}

#[test]
fn stored_visible_column_count_is_normalized_at_config_boundary() {
    let default = default_user_config();
    let mut stored = default.user_preferences().to_stored();
    stored.visible_column_count = 0;
    assert_eq!(
        UserPreferences::from_stored(stored.clone(), &default).visible_column_count,
        MIN_VISIBLE_COLUMN_COUNT
    );

    stored.visible_column_count = u32::MAX;
    assert_eq!(
        UserPreferences::from_stored(stored, &default).visible_column_count,
        MAX_VISIBLE_COLUMN_COUNT
    );
}

#[test]
fn legacy_icon_grid_preferences_are_loaded_and_normalized() {
    let parsed = legacy_toml::parse_toml_user_config(
        "browser_view_mode = \"icons\"\nicon_grid_size = 1000\n",
        default_user_config(),
    );

    assert_eq!(parsed.browser_view_mode, BrowserViewMode::Icons);
    assert_eq!(parsed.icon_grid_size, MAX_ICON_GRID_SIZE);
    assert_eq!(parsed.icons_view_density.index(), 8);

    let parsed = legacy_toml::parse_toml_user_config(
        "icon_grid_size = 80\ncolumns_view_density = 255\nlist_view_density = -1\nicons_view_density = 4\n",
        default_user_config(),
    );
    assert_eq!(
        (
            parsed.columns_view_density.index(),
            parsed.list_view_density.index(),
            parsed.icons_view_density.index(),
            parsed.icon_grid_size,
        ),
        (8, 0, 4, 128)
    );
}

#[test]
fn migrates_legacy_toml_preferences_to_sqlite_once() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(
        config_dir.join(CONFIG_FILE_NAME),
        r#"
thumbnail_cache_dir = "/tmp/thumbnails"
rendering_backend = "gpu"
show_hidden_files = true
sidebar_width = 250.0
startup_location = "previous_session"
startup_custom_directory = "/workspace"
save_view_state = true
shortcuts = { focus_path_input = "Ctrl+Alt+L" }
sidebar_favorites = [{ label = "Projects", path = "/srv/projects" }]
network_connections = [
  { id = "bad", label = "Bad", protocol = "webdav", uri = "davs://user:secret@example.test/docs", auto_connect = true },
  { id = "good", label = "Good", protocol = "smb", uri = "smb://server/share" },
]
"#,
    )
    .expect("write legacy config");
    let app_config =
        app_config::load_app_config_from_dir(&config_dir, app_config::default_app_config());
    let state_database_path = temp_dir.path().join("state.sqlite");

    let loaded = user_preferences::load_user_config_from_sources(
        app_config,
        &state_database_path,
        Some(&config_dir),
    );

    assert!(loaded.show_hidden_files);
    assert_eq!(loaded.sidebar_width, 250.0);
    assert_eq!(
        loaded.startup_location_policy,
        StartupLocationPolicy::PreviousSession
    );
    assert_eq!(loaded.startup_custom_directory, PathBuf::from("/workspace"));
    assert!(loaded.save_view_state);
    assert_eq!(loaded.sidebar_favorites.as_ref().map(Vec::len), Some(1));
    assert_eq!(loaded.network_connections.len(), 1);
    assert_eq!(loaded.network_connections[0].connection.id.as_str(), "good");
    assert_eq!(
        loaded
            .shortcuts
            .binding(ShortcutBindingId::FocusPathInput)
            .config_value(),
        "Ctrl+Alt+L"
    );

    let store = TaskQueueStore::new(&state_database_path).expect("open migrated store");
    let stored = store
        .read_user_preferences()
        .expect("read migrated preferences")
        .expect("migrated preferences");
    assert_eq!(stored.network_connections.len(), 1);
    assert_eq!(stored.network_connections[0].id, "good");
    assert!(!stored.network_connections[0].uri.contains("secret"));
}

#[test]
fn invalid_legacy_values_fall_back_to_defaults() {
    let default = default_user_config();
    let parsed = legacy_toml::parse_toml_user_config(
        r#"
show_hidden_files = "maybe"
network_list_thumbnail_downloads_enabled = "maybe"
max_preview_file_bytes = 0
sidebar_width = "wide"
terminal_emulator = "missing"
rendering_backend = "metal"
file_operation_verification = "maybe"
browser_view_mode = "cover-flow"
icon_grid_size = 1
startup_location = "moon"
startup_custom_directory = ""
save_view_state = "maybe"
"#,
        default.clone(),
    );

    assert_eq!(parsed.show_hidden_files, default.show_hidden_files);
    assert_eq!(
        parsed.network_list_thumbnail_downloads_enabled,
        default.network_list_thumbnail_downloads_enabled
    );
    assert_eq!(parsed.preview_size_limits, default.preview_size_limits);
    assert_eq!(parsed.sidebar_width, default.sidebar_width);
    assert_eq!(parsed.terminal_emulator, default.terminal_emulator);
    assert_eq!(
        parsed.rendering_gpu_preference,
        default.rendering_gpu_preference
    );
    assert_eq!(
        parsed.file_operation_verification,
        default.file_operation_verification
    );
    assert_eq!(parsed.browser_view_mode, default.browser_view_mode);
    assert_eq!(parsed.icon_grid_size, MIN_ICON_GRID_SIZE);
    assert_eq!(
        parsed.startup_location_policy,
        default.startup_location_policy
    );
    assert_eq!(
        parsed.startup_custom_directory,
        default.startup_custom_directory
    );
    assert_eq!(parsed.save_view_state, default.save_view_state);
    assert_eq!(
        parsed.list_directory_size_display_mode,
        default.list_directory_size_display_mode
    );
}

#[test]
fn default_user_config_keeps_expected_preview_defaults() {
    let config = default_user_config();

    assert!(!config.network_list_thumbnail_downloads_enabled);
    assert_eq!(
        config.preview_size_limits,
        PreviewFileSizeLimits::with_default_limits()
    );
    assert_eq!(config.preview_size_limits.text_bytes, 25 * 1024 * 1024);
    assert_eq!(config.preview_size_limits.image_bytes, 100 * 1024 * 1024);
    assert_eq!(config.preview_size_limits.video_bytes, 1024 * 1024 * 1024);
    assert_eq!(config.preview_size_limits.audio_bytes, 200 * 1024 * 1024);
    assert_eq!(config.preview_size_limits.archive_bytes, 25 * 1024 * 1024);
    assert_eq!(config.preview_size_limits.document_bytes, 100 * 1024 * 1024);
    assert_eq!(config.preview_directory_expand_levels, 1);
    assert_eq!(config.startup_location_policy, StartupLocationPolicy::Home);
    assert_eq!(config.icon_grid_size, DEFAULT_ICON_GRID_SIZE);
    assert_eq!(config.visible_column_count, DEFAULT_VISIBLE_COLUMN_COUNT);
    assert!(!config.save_view_state);
    assert_eq!(
        config.list_directory_size_display_mode,
        ListDirectorySizeDisplayMode::ItemCount
    );
}

#[test]
fn normalizes_legacy_sidebar_width_from_config() {
    let narrow = legacy_toml::parse_toml_user_config("sidebar_width = 20\n", default_user_config());
    let wide = legacy_toml::parse_toml_user_config("sidebar_width = 1200\n", default_user_config());

    assert_eq!(narrow.sidebar_width, MIN_SIDEBAR_WIDTH);
    assert_eq!(wide.sidebar_width, MAX_SIDEBAR_WIDTH);
}

#[test]
fn maps_rendering_gpu_preferences_to_iced_backend() {
    assert_eq!(
        RenderingGpuPreference::DisplayGpu.iced_backend_candidates(),
        "wgpu"
    );
    assert_eq!(
        RenderingGpuPreference::HighPerformanceGpu.iced_backend_candidates(),
        "wgpu"
    );
}

#[test]
fn maps_display_gpu_preference_to_display_matched_wgpu_power() {
    let integrated_gpu =
        DisplayRendererGpu::from_drm_ids(DisplayRendererGpuClass::Integrated, "0x1002", "0x15bf");
    let discrete_gpu =
        DisplayRendererGpu::from_drm_ids(DisplayRendererGpuClass::Discrete, "0x10de", "0x28e0");

    assert_eq!(
        RenderingGpuPreference::DisplayGpu.wgpu_power_preference(Some(&integrated_gpu)),
        Some("low")
    );
    assert_eq!(
        RenderingGpuPreference::DisplayGpu.wgpu_power_preference(Some(&discrete_gpu)),
        Some("high")
    );
    assert_eq!(
        RenderingGpuPreference::DisplayGpu.wgpu_power_preference(None),
        Some("none")
    );
    assert_eq!(
        RenderingGpuPreference::HighPerformanceGpu.wgpu_power_preference(Some(&integrated_gpu)),
        Some("high")
    );
}

#[test]
fn maps_display_gpu_preference_to_forced_vulkan_device_selection() {
    let integrated_gpu =
        DisplayRendererGpu::from_drm_ids(DisplayRendererGpuClass::Integrated, "0x1002", "0x15bf");

    assert_eq!(
        RenderingGpuPreference::DisplayGpu.mesa_vulkan_device_select(Some(&integrated_gpu)),
        Some(String::from("1002:15bf!"))
    );
    assert_eq!(
        RenderingGpuPreference::DisplayGpu.mesa_vulkan_device_select(None),
        None
    );
    assert_eq!(
        RenderingGpuPreference::HighPerformanceGpu.mesa_vulkan_device_select(Some(&integrated_gpu)),
        None
    );
}

#[test]
fn defaults_to_display_gpu_preference() {
    assert_eq!(
        default_user_config().rendering_gpu_preference,
        RenderingGpuPreference::DisplayGpu
    );
}

#[test]
fn per_kind_preview_limits_override_legacy_global_value() {
    let parsed = legacy_toml::parse_toml_user_config(
        r#"
max_preview_file_bytes = 4194304
max_preview_text_bytes = 1048576
max_preview_video_bytes = 0
preview_directory_expand_levels = 3
"#,
        default_user_config(),
    );

    assert_eq!(parsed.preview_size_limits.text_bytes, 1024 * 1024);
    assert_eq!(parsed.preview_size_limits.video_bytes, 0);
    assert_eq!(parsed.preview_size_limits.image_bytes, 4 * 1024 * 1024);
    assert_eq!(parsed.preview_directory_expand_levels, 3);
}

#[test]
fn normalizes_preview_extension_input() {
    assert_eq!(
        normalize_preview_extension(" .TXT ").as_deref(),
        Some("txt")
    );
    assert_eq!(normalize_preview_extension("..md"), Some("md".to_owned()));
    assert_eq!(
        normalize_preview_extension("tar.gz"),
        Some("tar.gz".to_owned())
    );
    assert_eq!(normalize_preview_extension("  "), None);
    assert_eq!(normalize_preview_extension("."), None);
    assert_eq!(normalize_preview_extension("my ext"), None);
}

#[test]
fn extension_rules_match_case_insensitively_and_support_compound_suffixes() {
    let rules = PreviewExtensionRules::default_rules();

    assert!(rules.matches(PreviewFileSizeKind::Text, &PathBuf::from("notes.TXT")));
    assert!(rules.matches(
        PreviewFileSizeKind::Archive,
        &PathBuf::from("bundle.tar.gz")
    ));
    assert!(rules.matches(PreviewFileSizeKind::Archive, &PathBuf::from("bundle.TGZ")));
    assert!(rules.matches(PreviewFileSizeKind::Document, &PathBuf::from("report.PDF")));
    assert!(!rules.matches(PreviewFileSizeKind::Text, &PathBuf::from("notes.bin")));
    assert!(!rules.matches(PreviewFileSizeKind::Archive, &PathBuf::from("archive.gz")));
}

#[test]
fn extension_rules_default_lists_mirror_builtin_types() {
    let rules = PreviewExtensionRules::default_rules();
    for (kind, expected) in [
        (PreviewFileSizeKind::Text, "txt"),
        (PreviewFileSizeKind::Image, "png"),
        (PreviewFileSizeKind::Video, "mp4"),
        (PreviewFileSizeKind::Audio, "mp3"),
        (PreviewFileSizeKind::Sqlite, "sqlite"),
        (PreviewFileSizeKind::Archive, "7z"),
        (PreviewFileSizeKind::Document, "docx"),
    ] {
        assert!(
            rules
                .list(kind)
                .iter()
                .any(|candidate| candidate == expected),
            "default {kind:?} list should contain {expected}"
        );
        assert_eq!(PreviewExtensionRules::default_list(kind), *rules.list(kind));
    }
}

#[test]
fn preview_extension_rules_roundtrip_through_stored_preferences() {
    let default = default_user_config();
    let mut config = default.clone();
    config.preview_extension_rules.text = vec!["nfo".to_owned(), "cfg".to_owned()];
    config.preview_extension_rules.image = Vec::new();

    let stored = config.user_preferences().to_stored();
    let restored = UserPreferences::from_stored(stored, &default);
    assert_eq!(restored.preview_extension_rules.text, vec!["nfo", "cfg"]);
    // 用户显式清空是合法选择，不能回退默认列表。
    assert!(restored.preview_extension_rules.image.is_empty());
    // 未触发的类型保持默认。
    assert_eq!(
        restored.preview_extension_rules.video,
        default.preview_extension_rules.video
    );

    // 旧版本存储缺该字段时整体回退默认。
    let stored = StoredUserPreferences::default();
    let restored = UserPreferences::from_stored(stored, &default);
    assert_eq!(
        restored.preview_extension_rules,
        default.preview_extension_rules
    );

    // 单类型缺失时该类型回退默认，其他类型保留用户值。
    let mut stored = config.user_preferences().to_stored();
    let mut rules = stored.preview_extension_rules.take().expect("stored rules");
    rules.text = None;
    stored.preview_extension_rules = Some(rules);
    let restored = UserPreferences::from_stored(stored, &default);
    assert_eq!(
        restored.preview_extension_rules.text,
        default.preview_extension_rules.text
    );
    assert!(restored.preview_extension_rules.image.is_empty());
}

#[test]
fn renderer_probe_cache_section_survives_a_toml_round_trip() {
    let stored = RendererProbeCacheRecord {
        version: 1,
        backend: "vulkan".to_owned(),
        rendering_gpu_preference: "display".to_owned(),
        wgpu_power_preference: Some("low".to_owned()),
        mesa_vulkan_device_select: Some("1002:15bf!".to_owned()),
        vulkan_loader_driver_select: Some("*amd*,*radeon*".to_owned()),
        display_gpu_device_select: Some("1002:15bf!".to_owned()),
    };
    let mut app_config = app_config::default_app_config();
    app_config.renderer_probe_cache = Some(stored.clone());

    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let path = temp_dir.path().join("config.toml");
    app_config::write_app_config(&path, &app_config).expect("write app config");

    let loaded = app_config::load_app_config_from_dir(temp_dir.path(), app_config::default_app_config())
        .renderer_probe_cache
        .expect("cache survives round trip");
    assert_eq!(loaded, stored);
}

#[test]
fn damaged_renderer_probe_cache_section_drops_cache_but_keeps_other_keys() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let path = temp_dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
thumbnail_cache_dir = "/tmp/thumbnails"
rendering_backend = "gpu"

[renderer_probe_cache]
version = "one"
backend = "vulkan"
"#,
    )
    .expect("write damaged config");

    let loaded = app_config::load_app_config_from_dir(temp_dir.path(), app_config::default_app_config());

    assert_eq!(loaded.renderer_probe_cache, None);
    assert_eq!(loaded.rendering_gpu_preference, RenderingGpuPreference::HighPerformanceGpu);
    assert_eq!(
        loaded.thumbnail_cache_dir,
        PathBuf::from("/tmp/thumbnails")
    );
}

#[test]
fn preserving_save_keeps_stored_probe_cache_section() {
    let stored = RendererProbeCacheRecord {
        version: 1,
        backend: "gl".to_owned(),
        rendering_gpu_preference: "gpu".to_owned(),
        wgpu_power_preference: Some("high".to_owned()),
        mesa_vulkan_device_select: None,
        vulkan_loader_driver_select: None,
        display_gpu_device_select: None,
    };
    let mut stored_config = app_config::default_app_config();
    stored_config.renderer_probe_cache = Some(stored.clone());

    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let path = temp_dir.path().join("config.toml");
    app_config::write_app_config(&path, &stored_config).expect("write stored config");

    let user_persisted = AppConfig::from_user_config(&default_user_config());
    app_config::save_app_config_preserving_probe_cache_at(&path, &user_persisted)
        .expect("preserving save");

    let reloaded =
        app_config::load_app_config_from_dir(temp_dir.path(), app_config::default_app_config());
    assert_eq!(reloaded.renderer_probe_cache, Some(stored));
    assert_eq!(
        reloaded.rendering_gpu_preference,
        user_persisted.rendering_gpu_preference
    );
}
