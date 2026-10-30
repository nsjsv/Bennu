//! 文件选择器的预览偏好：只从主程序的状态库读预览那一段设置。
//! 绝不写主程序的库；设置文档的其余键与这里无关，主程序后续增删字段不会波及 portal。

use std::path::Path;

use bennu_preview::preview_config::{
    preview_preferences_from_stored, PreviewPreferences, StoredPreviewPreferences,
};
use bennu_state::{UnreadablePreferences, UserSettingsStore};
use serde::{Deserialize, Serialize};

/// 设置文档里预览用到的键（flatten 让它们留在文档顶层，与主程序的布局一致）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PreviewSettings {
    #[serde(flatten)]
    preview: StoredPreviewPreferences,
}

/// 库不可用、没有设置行、整份设置读不懂都返回 Err，调用方回落共享 crate 的默认规则；
/// 个别键读不懂时只有那几个键回落默认。
pub(super) fn load_preview_preferences(database_path: &Path) -> Result<PreviewPreferences, String> {
    let read = UserSettingsStore::new(database_path)
        .and_then(|store| store.read_user_preferences(&PreviewSettings::default()))
        .map_err(|error| format!("Could not read user preferences: {error}"))?;
    let Some(read) = read else {
        return Err("No stored user preferences row".to_owned());
    };
    if read.unreadable == Some(UnreadablePreferences::Whole) {
        return Err("Invalid user preferences payload".to_owned());
    }
    Ok(preview_preferences_from_stored(&read.preferences.preview))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bennu_preview::preview_config::PreviewExtensionRules;
    use bennu_state::USER_PREFERENCES_KEY;

    use super::*;

    fn fixture_database(payload: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("state.sqlite");
        UserSettingsStore::new(&path).expect("fixture store");
        if let Some(payload) = payload {
            rusqlite::Connection::open(&path)
                .expect("fixture database")
                .execute(
                    "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
                     VALUES (?1, ?2, 0)",
                    rusqlite::params![USER_PREFERENCES_KEY, payload],
                )
                .expect("fixture row");
        }
        (dir, path)
    }

    #[test]
    fn loader_resolves_stored_payload_with_legacy_migration() {
        let payload = r#"{
            "network_list_thumbnail_downloads_enabled": false,
            "max_preview_file_bytes": 4194304,
            "preview_text_size_bytes": null,
            "preview_image_size_bytes": 1048576,
            "preview_extension_rules": {"text": ["md"], "image": []},
            "preview_directory_expand_levels": 2,
            "show_hidden_files": false,
            "sidebar_width": 180.0,
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
        let (_dir, path) = fixture_database(Some(payload));

        let preferences = load_preview_preferences(&path).expect("loader resolves stored payload");

        assert_eq!(preferences.size_limits.text_bytes, 4194304);
        assert_eq!(preferences.size_limits.image_bytes, 1048576);
        assert_eq!(preferences.size_limits.video_bytes, 4194304);
        assert_eq!(preferences.extension_rules.text, vec!["md".to_owned()]);
        assert_eq!(preferences.extension_rules.image, Vec::<String>::new());
        assert_eq!(
            preferences.extension_rules.video,
            PreviewExtensionRules::default_rules().video
        );
        assert_eq!(preferences.directory_expand_levels, 2);
    }

    #[test]
    fn a_damaged_unrelated_setting_does_not_cost_the_preview_preferences() {
        let (_dir, path) = fixture_database(Some(
            r#"{"preview_directory_expand_levels": 2, "show_hidden_files": "yes"}"#,
        ));

        let preferences = load_preview_preferences(&path).expect("preview keys are readable");

        assert_eq!(preferences.directory_expand_levels, 2);
    }

    #[test]
    fn loader_falls_back_to_error_on_missing_or_corrupt_state() {
        // 库打不开：数据库所在目录建不出来。
        let dir = tempfile::tempdir().expect("temp dir");
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"").expect("blocker file");
        assert!(load_preview_preferences(&blocker.join("state.sqlite")).is_err());

        // 库在但主程序从未保存过设置。
        let (_dir, no_row) = fixture_database(None);
        assert!(load_preview_preferences(&no_row).is_err());

        // 载荷损坏。
        let (_dir, corrupt) = fixture_database(Some("not json"));
        assert!(load_preview_preferences(&corrupt).is_err());
    }
}
