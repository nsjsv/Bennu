use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::{StoreError, StoreResult, StoredPath, TaskQueueStore};

pub const USER_PREFERENCES_KEY: &str = "main";
const DEFAULT_VIEW_DENSITY_INDEX: u8 = 2;

/// StoredUserPreferences.launch_window_policy 的合法取值。
pub const LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING: &str = "merge_into_existing";
pub const LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW: &str = "open_new_window";

/// StoredUserPreferences.column_width_adjust_mode 的合法取值。
pub const COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN: &str = "per_column";
pub const COLUMN_WIDTH_ADJUST_MODE_UNIFORM: &str = "uniform";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredUserPreferences {
    pub network_list_thumbnail_downloads_enabled: bool,
    #[serde(default)]
    pub max_preview_file_bytes: Option<u64>,
    #[serde(default)]
    pub preview_text_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_image_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_video_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_audio_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_archive_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_document_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_sqlite_size_bytes: Option<u64>,
    /// 空格预览的分类型后缀规则；None 表示旧版本数据，读取端回退内置
    /// 默认列表。单类型内 None 同样回退默认，空列表是合法用户选择。
    #[serde(default)]
    pub preview_extension_rules: Option<StoredPreviewExtensionRules>,
    #[serde(default)]
    pub preview_directory_expand_levels: Option<u8>,
    pub show_hidden_files: bool,
    #[serde(default = "default_language_setting")]
    pub language_setting: String,
    pub sidebar_width: f64,
    /// 右侧停靠预览面板开关;缺省 = 关(旧数据无此字段)。
    #[serde(default)]
    pub right_preview_panel_open: bool,
    /// 右侧停靠预览面板宽度;None = 旧数据,由 app 层回退默认宽度。
    #[serde(default)]
    pub right_preview_panel_width: Option<f64>,
    /// 面板内预览区高度占比;None = 旧数据,由 app 层回退默认比例。
    #[serde(default)]
    pub right_preview_preview_ratio: Option<f64>,
    pub sidebar_favorites: Option<Vec<StoredSidebarFavorite>>,
    pub network_connections: Vec<StoredNetworkConnection>,
    pub terminal_emulator: String,
    /// 内嵌终端使用的 shell;None 为旧版本数据,读取端回退系统登录 shell。
    #[serde(default)]
    pub terminal_shell: Option<String>,
    pub file_operation_verification: String,
    pub browser_view_mode: String,
    #[serde(default = "default_icon_grid_size")]
    pub icon_grid_size: u32,
    #[serde(default)]
    pub columns_view_density: Option<u8>,
    #[serde(default)]
    pub list_view_density: Option<u8>,
    #[serde(default)]
    pub icons_view_density: Option<u8>,
    #[serde(default = "default_visible_column_count")]
    pub visible_column_count: u32,
    pub startup_location: String,
    pub startup_custom_directory: StoredPath,
    pub save_view_state: bool,
    pub shortcuts: Vec<StoredShortcutBinding>,
    #[serde(default = "default_stored_list_view_columns")]
    pub list_view_columns: Vec<StoredListViewColumn>,
    #[serde(default = "default_list_sort_field")]
    pub list_sort_field: String,
    #[serde(default = "default_list_sort_direction")]
    pub list_sort_direction: String,
    #[serde(default = "default_list_directory_size_display_mode")]
    pub list_directory_size_display_mode: String,
    #[serde(default = "default_window_chrome_layout")]
    pub window_chrome_layout: String,
    #[serde(default = "default_stored_window_controls")]
    pub window_controls: Vec<StoredWindowControlPlacement>,
    #[serde(default)]
    pub search_history: Vec<String>,
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
    #[serde(default)]
    pub custom_color_scheme: Option<StoredCustomColorScheme>,
    #[serde(default = "default_launch_window_policy")]
    pub launch_window_policy: String,
    /// 多栏视图栏宽的调整方式;缺省 = 每栏独立(旧数据行为)。
    #[serde(default = "default_column_width_adjust_mode")]
    pub column_width_adjust_mode: String,
    /// 各右键菜单的项顺序与可见性;None = 旧版本数据,读取端回退内置默认。
    #[serde(default)]
    pub context_menu_layouts: Option<StoredContextMenuLayouts>,
}

/// 一份右键菜单的有序项列表;id 为 app 层定义的稳定字符串。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredContextMenuLayout {
    pub items: Vec<StoredContextMenuItemEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredContextMenuItemEntry {
    pub id: String,
    pub visible: bool,
}

/// 9 份可配置右键菜单的存储形态,字段与 app 层菜单页一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredContextMenuLayouts {
    pub file_entry: StoredContextMenuLayout,
    pub file_blank: StoredContextMenuLayout,
    pub trash: StoredContextMenuLayout,
    pub search: StoredContextMenuLayout,
    pub search_entry_types: StoredContextMenuLayout,
    pub list_columns: StoredContextMenuLayout,
    pub sidebar_bookmark: StoredContextMenuLayout,
    pub sidebar_device: StoredContextMenuLayout,
    pub network_connection: StoredContextMenuLayout,
}

impl Default for StoredUserPreferences {
    fn default() -> Self {
        Self {
            network_list_thumbnail_downloads_enabled: false,
            max_preview_file_bytes: None,
            preview_text_size_bytes: None,
            preview_image_size_bytes: None,
            preview_video_size_bytes: None,
            preview_audio_size_bytes: None,
            preview_archive_size_bytes: None,
            preview_document_size_bytes: None,
            preview_sqlite_size_bytes: None,
            preview_extension_rules: None,
            preview_directory_expand_levels: None,
            show_hidden_files: false,
            language_setting: default_language_setting(),
            sidebar_width: 180.0,
            right_preview_panel_open: false,
            right_preview_panel_width: None,
            right_preview_preview_ratio: None,
            sidebar_favorites: None,
            network_connections: Vec::new(),
            terminal_emulator: "automatic".to_owned(),
            terminal_shell: None,
            file_operation_verification: "basic_metadata".to_owned(),
            browser_view_mode: "columns".to_owned(),
            icon_grid_size: default_icon_grid_size(),
            columns_view_density: Some(DEFAULT_VIEW_DENSITY_INDEX),
            list_view_density: Some(DEFAULT_VIEW_DENSITY_INDEX),
            icons_view_density: Some(DEFAULT_VIEW_DENSITY_INDEX),
            visible_column_count: default_visible_column_count(),
            startup_location: "home".to_owned(),
            startup_custom_directory: StoredPath::from_path(Path::new("")),
            save_view_state: false,
            shortcuts: Vec::new(),
            list_view_columns: default_stored_list_view_columns(),
            list_sort_field: default_list_sort_field(),
            list_sort_direction: default_list_sort_direction(),
            list_directory_size_display_mode: default_list_directory_size_display_mode(),
            window_chrome_layout: default_window_chrome_layout(),
            window_controls: default_stored_window_controls(),
            search_history: Vec::new(),
            theme_mode: default_theme_mode(),
            color_scheme: default_color_scheme(),
            custom_color_scheme: None,
            launch_window_policy: default_launch_window_policy(),
            column_width_adjust_mode: default_column_width_adjust_mode(),
            context_menu_layouts: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoredPreviewExtensionRules {
    #[serde(default)]
    pub text: Option<Vec<String>>,
    #[serde(default)]
    pub image: Option<Vec<String>>,
    #[serde(default)]
    pub video: Option<Vec<String>>,
    #[serde(default)]
    pub audio: Option<Vec<String>>,
    #[serde(default)]
    pub sqlite: Option<Vec<String>>,
    #[serde(default)]
    pub archive: Option<Vec<String>>,
    #[serde(default)]
    pub document: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCustomColorScheme {
    #[serde(default)]
    pub light: Option<StoredCustomColorSet>,
    #[serde(default)]
    pub dark: Option<StoredCustomColorSet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoredCustomColorSet {
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub surface: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub muted_text: String,
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub success: String,
    #[serde(default)]
    pub warning: String,
    #[serde(default)]
    pub danger: String,
}

/// 新装/旧数据缺字段的默认策略：每次触发启动都打开新窗口。
fn default_launch_window_policy() -> String {
    LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW.to_owned()
}

/// 新装/旧数据缺字段的默认策略：保持每栏独立调宽的历史行为。
fn default_column_width_adjust_mode() -> String {
    COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN.to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredWindowControlPlacement {
    pub kind: String,
    pub side: String,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredListViewColumn {
    pub kind: String,
    pub width: f64,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSidebarFavorite {
    pub label: String,
    pub path: StoredPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredNetworkConnection {
    pub id: String,
    pub label: String,
    pub protocol: String,
    pub uri: String,
    pub auto_connect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredShortcutBinding {
    pub action_key: String,
    pub binding: String,
}

fn default_stored_list_view_columns() -> Vec<StoredListViewColumn> {
    [
        ("name", 320.0, true),
        ("modified", 168.0, true),
        ("size", 96.0, true),
        ("kind", 96.0, true),
        ("owner", 120.0, false),
        ("group", 120.0, false),
        ("permissions", 128.0, false),
        ("accessed", 168.0, false),
        ("created", 168.0, false),
    ]
    .into_iter()
    .map(|(kind, width, visible)| StoredListViewColumn {
        kind: kind.to_owned(),
        width,
        visible,
    })
    .collect()
}

fn default_list_sort_field() -> String {
    "name".to_owned()
}

fn default_list_sort_direction() -> String {
    "ascending".to_owned()
}

fn default_list_directory_size_display_mode() -> String {
    "item_count".to_owned()
}

fn default_theme_mode() -> String {
    "automatic".to_owned()
}

fn default_color_scheme() -> String {
    "default".to_owned()
}

fn default_language_setting() -> String {
    "system".to_owned()
}

fn default_icon_grid_size() -> u32 {
    96
}

fn default_visible_column_count() -> u32 {
    3
}

fn default_window_chrome_layout() -> String {
    "integrated_navigation".to_owned()
}

fn default_stored_window_controls() -> Vec<StoredWindowControlPlacement> {
    ["minimize", "maximize_restore", "close"]
        .into_iter()
        .map(|kind| StoredWindowControlPlacement {
            kind: kind.to_owned(),
            side: "right".to_owned(),
            visible: true,
        })
        .collect()
}

impl TaskQueueStore {
    pub fn read_user_preferences(&self) -> StoreResult<Option<StoredUserPreferences>> {
        let connection = self.connection()?;
        let payload_json = connection
            .query_row(
                "SELECT payload_json FROM user_preferences WHERE preference_key = ?1",
                rusqlite::params![USER_PREFERENCES_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match payload_json {
            Some(payload_json) => match serde_json::from_str(&payload_json) {
                Ok(preferences) => Ok(Some(preferences)),
                Err(error) => {
                    connection.execute(
                        "DELETE FROM user_preferences WHERE preference_key = ?1",
                        rusqlite::params![USER_PREFERENCES_KEY],
                    )?;
                    Err(StoreError::Json(error))
                }
            },
            None => Ok(None),
        }
    }

    pub fn replace_user_preferences(&self, preferences: &StoredUserPreferences) -> StoreResult<()> {
        let connection = self.connection()?;
        let payload_json = serde_json::to_string(preferences)?;
        connection.execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(preference_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![USER_PREFERENCES_KEY, payload_json, crate::current_time_ms()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod launch_window_policy_tests {
    use super::*;

    #[test]
    fn missing_preview_extension_rules_default_to_none() {
        let stored = StoredUserPreferences::default();
        let mut json = serde_json::to_value(&stored).expect("serialize preferences");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("preview_extension_rules");

        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        assert_eq!(parsed.preview_extension_rules, None);
    }

    #[test]
    fn preview_extension_rules_roundtrip_preserves_empty_lists() {
        let stored = StoredUserPreferences {
            preview_extension_rules: Some(StoredPreviewExtensionRules {
                text: Some(Vec::new()),
                ..StoredPreviewExtensionRules::default()
            }),
            ..StoredUserPreferences::default()
        };
        let json = serde_json::to_value(&stored).expect("serialize preferences");
        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        let rules = parsed
            .preview_extension_rules
            .expect("stored preview rules");
        assert_eq!(rules.text, Some(Vec::new()));
        assert_eq!(rules.image, None);
    }

    #[test]
    fn missing_launch_window_policy_defaults_to_open_new_window() {
        let stored = StoredUserPreferences::default();
        let mut json = serde_json::to_value(&stored).expect("serialize preferences");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("launch_window_policy");

        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        assert_eq!(
            parsed.launch_window_policy,
            LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW
        );
    }

    #[test]
    fn default_launch_window_policy_is_open_new_window() {
        assert_eq!(
            StoredUserPreferences::default().launch_window_policy,
            LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW
        );
    }

    #[test]
    fn missing_column_width_adjust_mode_defaults_to_per_column() {
        // 回归:旧偏好数据没有模式字段,必须回退到每栏独立的历史行为。
        let stored = StoredUserPreferences::default();
        let mut json = serde_json::to_value(&stored).expect("serialize preferences");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("column_width_adjust_mode");

        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        assert_eq!(
            parsed.column_width_adjust_mode,
            COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN
        );
        assert_eq!(
            StoredUserPreferences::default().column_width_adjust_mode,
            COLUMN_WIDTH_ADJUST_MODE_PER_COLUMN
        );
    }

    #[test]
    fn missing_right_preview_panel_fields_default_to_closed_and_unsized() {
        // 回归:旧偏好数据没有面板字段,serde 缺省回退必须同时覆盖开关与宽度。
        let stored = StoredUserPreferences::default();
        let mut json = serde_json::to_value(&stored).expect("serialize preferences");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("right_preview_panel_open");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("right_preview_panel_width");
        json.as_object_mut()
            .expect("preferences serialize to an object")
            .remove("right_preview_preview_ratio");

        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        assert!(!parsed.right_preview_panel_open);
        assert_eq!(parsed.right_preview_panel_width, None);
        assert_eq!(parsed.right_preview_preview_ratio, None);
    }

    #[test]
    fn right_preview_panel_fields_roundtrip() {
        let stored = StoredUserPreferences {
            right_preview_panel_open: true,
            right_preview_panel_width: Some(480.0),
            right_preview_preview_ratio: Some(0.8),
            ..StoredUserPreferences::default()
        };
        let json = serde_json::to_value(&stored).expect("serialize preferences");
        let parsed: StoredUserPreferences =
            serde_json::from_value(json).expect("deserialize preferences");

        assert!(parsed.right_preview_panel_open);
        assert_eq!(parsed.right_preview_panel_width, Some(480.0));
        assert_eq!(parsed.right_preview_preview_ratio, Some(0.8));
    }
}
