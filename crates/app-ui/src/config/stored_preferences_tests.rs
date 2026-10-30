//! 设置文档经 `UserSettingsStore` 落盘再读回：新字段原样往返，旧数据缺的字段取默认值。

use std::path::Path;

use bennu_preview::preview_config::StoredPreviewPreferences;
use bennu_sidebar::{StoredNetworkConnection, StoredSidebarFavorite};
use bennu_state::{UserSettingsStore, USER_PREFERENCES_KEY};
use bennu_theme::{StoredCustomColorScheme, StoredCustomColorSet};
use rusqlite::{params, Connection};

use super::*;

fn test_store() -> (UserSettingsStore, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let store = UserSettingsStore::new(directory.path().join("state.sqlite")).unwrap();
    (store, directory)
}

fn stored_preferences(store: &UserSettingsStore) -> Option<StoredUserPreferences> {
    store
        .read_user_preferences(&StoredUserPreferences::default())
        .unwrap()
        .map(|read| read.preferences)
}

fn insert_raw_preferences(store: &UserSettingsStore, payload_json: &str) {
    Connection::open(store.db_path())
        .unwrap()
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![USER_PREFERENCES_KEY, payload_json, 1_i64],
        )
        .unwrap();
}

#[test]
fn user_preferences_roundtrip_replace() {
    let (store, _directory) = test_store();
    assert_eq!(stored_preferences(&store), None);
    let defaults = StoredUserPreferences::default();
    assert_eq!(defaults.preview.preview_text_size_bytes, None);
    assert_eq!(
        (
            defaults.columns_view_density,
            defaults.list_view_density,
            defaults.icons_view_density,
        ),
        (Some(2), Some(2), Some(2))
    );
    let first = StoredUserPreferences {
        network_list_thumbnail_downloads_enabled: true,
        preview: StoredPreviewPreferences {
            max_preview_file_bytes: None,
            preview_text_size_bytes: Some(8 * 1024 * 1024),
            ..StoredPreviewPreferences::default()
        },
        show_hidden_files: true,
        language_setting: "chinese".to_owned(),
        sidebar_width: 248.0,
        sidebar_favorites: Some(vec![StoredSidebarFavorite {
            label: "Projects".to_owned(),
            path: StoredPath::from_path(Path::new("/srv/projects")),
        }]),
        network_connections: vec![StoredNetworkConnection {
            id: "nas".to_owned(),
            label: "NAS".to_owned(),
            protocol: "smb".to_owned(),
            uri: "smb://server/share".to_owned(),
            auto_connect: true,
        }],
        terminal_emulator: "ghostty".to_owned(),
        file_operation_verification: "strong".to_owned(),
        browser_view_mode: "list".to_owned(),
        icon_grid_size: None,
        columns_view_density: Some(1),
        list_view_density: Some(4),
        icons_view_density: Some(6),
        startup_location: "previous_session".to_owned(),
        startup_custom_directory: StoredPath::from_path(Path::new("/workspace")),
        save_view_state: true,
        shortcuts: vec![StoredShortcutBinding {
            action_key: "focus_path_input".to_owned(),
            binding: "Ctrl+Alt+L".to_owned(),
        }],
        list_view_columns: vec![
            StoredListViewColumn {
                kind: "name".to_owned(),
                width: 300.0,
                visible: true,
            },
            StoredListViewColumn {
                kind: "size".to_owned(),
                width: 110.0,
                visible: false,
            },
        ],
        list_sort_field: "size".to_owned(),
        list_sort_direction: "descending".to_owned(),
        list_directory_size_display_mode: "recursive_total_size".to_owned(),
        window_chrome_layout: "separate_title_bar".to_owned(),
        window_controls: vec![
            StoredWindowControlPlacement {
                kind: "close".to_owned(),
                side: "left".to_owned(),
                visible: true,
            },
            StoredWindowControlPlacement {
                kind: "minimize".to_owned(),
                side: "right".to_owned(),
                visible: false,
            },
            StoredWindowControlPlacement {
                kind: "maximize_restore".to_owned(),
                side: "right".to_owned(),
                visible: true,
            },
        ],
        search_history: vec!["report".to_owned(), "images".to_owned()],
        theme_mode: "dark".to_owned(),
        color_scheme: "custom".to_owned(),
        custom_color_scheme: Some(StoredCustomColorScheme {
            light: Some(StoredCustomColorSet {
                background: "#ffffff".to_owned(),
                surface: "#f6f8fa".to_owned(),
                text: "#1f2328".to_owned(),
                muted_text: "#59636e".to_owned(),
                primary: "#0969da".to_owned(),
                success: "#1a7f37".to_owned(),
                warning: "#9a6700".to_owned(),
                danger: "#d1242f".to_owned(),
            }),
            dark: Some(StoredCustomColorSet {
                background: "#0d1117".to_owned(),
                surface: "#151b23".to_owned(),
                text: "#f0f6fc".to_owned(),
                muted_text: "#9198a1".to_owned(),
                primary: "#4493f8".to_owned(),
                success: "#3fb950".to_owned(),
                warning: "#d29922".to_owned(),
                danger: "#f85149".to_owned(),
            }),
        }),
        ..defaults
    };

    store.replace_user_preferences(&first).unwrap();
    assert_eq!(stored_preferences(&store), Some(first.clone()));

    let second = StoredUserPreferences {
        show_hidden_files: false,
        sidebar_favorites: Some(Vec::new()),
        network_connections: Vec::new(),
        shortcuts: Vec::new(),
        ..first
    };
    store.replace_user_preferences(&second).unwrap();

    assert_eq!(stored_preferences(&store), Some(second));
}

#[test]
fn legacy_user_preferences_without_optional_fields_get_defaults() {
    let (store, _directory) = test_store();
    let mut payload = serde_json::to_value(StoredUserPreferences::default()).unwrap();
    let object = payload.as_object_mut().expect("preferences payload object");
    object.remove("list_view_columns");
    object.remove("list_sort_field");
    object.remove("list_sort_direction");
    object.remove("list_directory_size_display_mode");
    object.remove("columns_view_density");
    object.remove("list_view_density");
    object.remove("icons_view_density");
    object.remove("window_chrome_layout");
    object.remove("window_controls");
    object.remove("search_history");
    object.remove("theme_mode");
    object.remove("color_scheme");
    object.remove("visible_column_count");
    insert_raw_preferences(&store, &serde_json::to_string(&payload).unwrap());

    let preferences = stored_preferences(&store).expect("preferences load");

    assert_eq!(preferences.list_view_columns.len(), 9);
    assert_eq!(preferences.list_view_columns[0].kind, "name");
    assert_eq!(preferences.list_view_columns[4].kind, "owner");
    assert!(!preferences.list_view_columns[4].visible);
    assert_eq!(preferences.list_sort_field, "name");
    assert_eq!(preferences.list_sort_direction, "ascending");
    assert_eq!(preferences.list_directory_size_display_mode, "item_count");
    assert_eq!(preferences.language_setting, "system");
    assert_eq!(preferences.icon_grid_size, None);
    assert_eq!(preferences.columns_view_density, None);
    assert_eq!(preferences.list_view_density, None);
    assert_eq!(preferences.icons_view_density, None);
    assert_eq!(preferences.window_chrome_layout, "integrated_navigation");
    assert_eq!(preferences.visible_column_count, 3);
    assert_eq!(preferences.window_controls.len(), 3);
    assert_eq!(preferences.window_controls[0].kind, "minimize");
    assert_eq!(preferences.window_controls[0].side, "right");
    assert!(preferences.window_controls[0].visible);
    assert_eq!(preferences.window_controls[2].kind, "close");
    assert!(preferences.window_controls[2].visible);
    assert!(preferences.search_history.is_empty());
    assert_eq!(preferences.theme_mode, "automatic");
    assert_eq!(preferences.color_scheme, "default");
}

#[test]
fn legacy_user_preferences_ignore_removed_search_fields() {
    let (store, _directory) = test_store();
    let payload_json = serde_json::json!({
        "search_index_exclude_patterns": ["target/"],
        "search_index_media_scope": "images",
        "search_index_directory_error_policy": "abort",
        "search_mode": "indexed",
        "search_mode_prompt": "completed",
        "network_list_thumbnail_downloads_enabled": true,
        "max_preview_file_bytes": 4194304,
        "show_hidden_files": true,
        "language_setting": "chinese",
        "sidebar_width": 240.0,
        "sidebar_favorites": null,
        "network_connections": [],
        "terminal_emulator": "automatic",
        "file_operation_verification": "basic_metadata",
        "browser_view_mode": "columns",
        "startup_location": "home",
        "startup_custom_directory": {"encoding": "utf8", "value": ""},
        "save_view_state": false,
        "shortcuts": [],
        "list_view_columns": StoredUserPreferences::default().list_view_columns,
        "list_sort_field": "name",
        "list_sort_direction": "ascending",
        "list_directory_size_display_mode": "item_count"
    })
    .to_string();
    insert_raw_preferences(&store, &payload_json);

    let preferences = stored_preferences(&store).expect("preferences load");

    assert!(preferences.network_list_thumbnail_downloads_enabled);
    assert_eq!(preferences.language_setting, "chinese");
    assert_eq!(preferences.icon_grid_size, None);
    assert_eq!(preferences.preview.max_preview_file_bytes, Some(4194304));
    assert_eq!(preferences.preview.preview_text_size_bytes, None);
}

/// 预览偏好段拆出（平铺字段时代）之前的整份用户偏好文档样本：仅含无 serde 缺省的
/// 必需键 + 全部 preview 键，字段名与旧文档逐字一致。
const LEGACY_DOCUMENT: &str = r#"{
        "network_list_thumbnail_downloads_enabled": false,
        "max_preview_file_bytes": 4194304,
        "preview_text_size_bytes": null,
        "preview_image_size_bytes": 1048576,
        "preview_video_size_bytes": null,
        "preview_audio_size_bytes": 5242880,
        "preview_archive_size_bytes": null,
        "preview_document_size_bytes": null,
        "preview_sqlite_size_bytes": null,
        "preview_extension_rules": {
            "text": ["txt", "log"],
            "image": [],
            "video": null,
            "audio": ["flac"],
            "sqlite": null,
            "archive": null,
            "document": null
        },
        "preview_directory_expand_levels": 2,
        "show_hidden_files": true,
        "sidebar_width": 200.0,
        "sidebar_favorites": null,
        "network_connections": [],
        "terminal_emulator": "automatic",
        "file_operation_verification": "basic_metadata",
        "browser_view_mode": "columns",
        "startup_location": "home",
        "startup_custom_directory": {"encoding": "unix_bytes", "bytes": []},
        "save_view_state": false,
        "shortcuts": []
    }"#;

/// 字节级兼容门：flatten 组合后的默认序列化必须与下沉前的平铺布局
/// 逐字一致（字段名、顺序、null 形态），否则旧库新读/新写旧读都会
/// 出现语义外的裁剪差异。布局变更必须经迁移评审，不许静默改。
#[test]
fn stored_user_preferences_default_json_layout_is_frozen() {
    const FROZEN_DEFAULT_JSON: &str = "{\"network_list_thumbnail_downloads_enabled\":false,\"max_preview_file_bytes\":null,\"preview_text_size_bytes\":null,\"preview_image_size_bytes\":null,\"preview_video_size_bytes\":null,\"preview_audio_size_bytes\":null,\"preview_archive_size_bytes\":null,\"preview_document_size_bytes\":null,\"preview_sqlite_size_bytes\":null,\"preview_extension_rules\":null,\"preview_directory_expand_levels\":null,\"show_hidden_files\":false,\"language_setting\":\"system\",\"sidebar_width\":180.0,\"right_preview_panel_open\":false,\"right_preview_panel_width\":null,\"right_preview_preview_ratio\":null,\"sidebar_favorites\":null,\"network_connections\":[],\"terminal_emulator\":\"automatic\",\"terminal_shell\":null,\"file_operation_verification\":\"basic_metadata\",\"browser_view_mode\":\"columns\",\"columns_view_density\":2,\"list_view_density\":2,\"icons_view_density\":2,\"visible_column_count\":3,\"startup_location\":\"home\",\"startup_custom_directory\":{\"encoding\":\"unix_bytes\",\"bytes\":[]},\"save_view_state\":false,\"shortcuts\":[],\"list_view_columns\":[{\"kind\":\"name\",\"width\":320.0,\"visible\":true},{\"kind\":\"modified\",\"width\":168.0,\"visible\":true},{\"kind\":\"size\",\"width\":96.0,\"visible\":true},{\"kind\":\"kind\",\"width\":96.0,\"visible\":true},{\"kind\":\"owner\",\"width\":120.0,\"visible\":false},{\"kind\":\"group\",\"width\":120.0,\"visible\":false},{\"kind\":\"permissions\",\"width\":128.0,\"visible\":false},{\"kind\":\"accessed\",\"width\":168.0,\"visible\":false},{\"kind\":\"created\",\"width\":168.0,\"visible\":false}],\"list_sort_field\":\"name\",\"list_sort_direction\":\"ascending\",\"list_directory_size_display_mode\":\"item_count\",\"file_grouping\":null,\"window_chrome_layout\":\"integrated_navigation\",\"window_controls\":[{\"kind\":\"minimize\",\"side\":\"right\",\"visible\":true},{\"kind\":\"maximize_restore\",\"side\":\"right\",\"visible\":true},{\"kind\":\"close\",\"side\":\"right\",\"visible\":true}],\"search_history\":[],\"last_search_scope\":null,\"theme_mode\":\"automatic\",\"color_scheme\":\"default\",\"custom_color_scheme\":null,\"launch_window_policy\":\"open_new_window\",\"column_width_adjust_mode\":\"per_column\",\"context_menu_layouts\":null,\"transfer_download_dir\":null,\"transfer_device_alias\":null,\"transfer_trusted_devices\":[]}";
    let serialized = serde_json::to_string(&StoredUserPreferences::default())
        .expect("serialize stored preferences");
    assert_eq!(serialized, FROZEN_DEFAULT_JSON);
}

#[test]
fn full_legacy_document_roundtrips_through_stored_user_preferences() {
    let parsed: StoredUserPreferences =
        serde_json::from_str(LEGACY_DOCUMENT).expect("legacy document parses");
    let serialized = serde_json::to_string(&parsed).expect("serialize roundtrip");
    // 读旧写新字节等值：旧文档重新落盘不产生任何布局差异。
    let reparsed: StoredUserPreferences =
        serde_json::from_str(&serialized).expect("reparsed roundtrip");
    assert_eq!(parsed, reparsed);
    assert_eq!(
        serde_json::to_string(&reparsed).expect("serialize reparsed"),
        serialized
    );
    assert_eq!(parsed.preview.preview_directory_expand_levels, Some(2));
    assert_eq!(
        parsed
            .preview
            .preview_extension_rules
            .as_ref()
            .and_then(|rules| rules.text.clone()),
        Some(vec!["txt".to_owned(), "log".to_owned()])
    );
}
