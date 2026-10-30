use std::os::unix::fs::PermissionsExt;

use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::{
    BrowserSessionStore, StateDatabase, StoredApplicationShutdown, StoredBrowserSessionShutdown,
};

/// 应用设置文档的最小替身：读写规则对任何"JSON 对象"文档成立，不依赖具体字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Document {
    terminal_emulator: String,
    show_hidden_files: bool,
    sidebar_width: f64,
    network_connections: Vec<Connection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Connection {
    id: String,
    label: String,
    uri: String,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            terminal_emulator: "automatic".to_owned(),
            show_hidden_files: false,
            sidebar_width: 180.0,
            network_connections: Vec::new(),
        }
    }
}

fn store_with_payload(payload: &str) -> (UserSettingsStore, TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let store = UserSettingsStore::new(directory.path().join("state.sqlite")).unwrap();
    store
        .database
        .write()
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, 1)",
            rusqlite::params![USER_PREFERENCES_KEY, payload],
        )
        .unwrap();
    (store, directory)
}

fn stored_payload(store: &UserSettingsStore) -> String {
    read_payload(&store.database.read())
        .unwrap()
        .expect("preferences row")
}

fn read_back(store: &UserSettingsStore) -> UserPreferencesRead<Document> {
    store
        .read_user_preferences(&Document::default())
        .unwrap()
        .expect("preferences row")
}

fn default_payload_with(changes: Value) -> String {
    let mut payload = payload_object(&Document::default()).unwrap();
    for (key, value) in changes.as_object().expect("changes object") {
        payload.insert(key.clone(), value.clone());
    }
    Value::Object(payload).to_string()
}

fn backups(directory: &TempDir) -> Vec<PathBuf> {
    let mut backups = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("user-preferences-"))
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups
}

#[test]
fn an_unreadable_payload_stays_in_place_after_reading() {
    let (store, _directory) = store_with_payload("{");

    let read = read_back(&store);

    assert_eq!(read.unreadable, Some(UnreadablePreferences::Whole));
    assert_eq!(read.preferences, Document::default());
    assert_eq!(read.raw_payload, "{");
    assert_eq!(stored_payload(&store), "{");
}

#[test]
fn unreadable_settings_fall_back_to_the_callers_defaults() {
    let defaults = Document {
        terminal_emulator: "foot".to_owned(),
        ..Document::default()
    };
    let (whole, _whole_directory) = store_with_payload("{");
    let (one_key, _one_key_directory) =
        store_with_payload(&default_payload_with(json!({"terminal_emulator": 5})));

    for store in [whole, one_key] {
        let read = store
            .read_user_preferences(&defaults)
            .unwrap()
            .expect("preferences row");
        assert_eq!(read.preferences.terminal_emulator, "foot");
    }
}

#[test]
fn a_key_of_the_wrong_type_falls_back_alone() {
    let payload = default_payload_with(json!({
        "show_hidden_files": "yes",
        "sidebar_width": 321.0,
    }));
    let (store, _directory) = store_with_payload(&payload);

    let read = read_back(&store);

    assert_eq!(
        read.unreadable,
        Some(UnreadablePreferences::Keys(vec![
            "show_hidden_files".to_owned()
        ]))
    );
    assert_eq!(read.preferences.sidebar_width, 321.0);
    assert_eq!(
        read.preferences.show_hidden_files,
        Document::default().show_hidden_files
    );
}

#[test]
fn one_unreadable_list_item_does_not_drop_the_others() {
    let payload = default_payload_with(json!({
        "network_connections": [
            {"id": "nas", "label": "NAS", "uri": "smb://nas/share"},
            {"id": 5},
        ],
    }));
    let (store, _directory) = store_with_payload(&payload);

    let read = read_back(&store);

    assert_eq!(
        read.unreadable,
        Some(UnreadablePreferences::Keys(vec![
            "network_connections".to_owned()
        ]))
    );
    assert_eq!(
        read.preferences.network_connections,
        [Connection {
            id: "nas".to_owned(),
            label: "NAS".to_owned(),
            uri: "smb://nas/share".to_owned(),
        }]
    );
}

#[test]
fn a_key_written_before_it_existed_is_not_reported_as_unreadable() {
    let mut payload = payload_object(&Document::default()).unwrap();
    payload.remove("show_hidden_files");
    payload.insert("sidebar_width".to_owned(), json!(300.0));
    let (store, _directory) = store_with_payload(&Value::Object(payload).to_string());

    let read = read_back(&store);

    assert_eq!(read.unreadable, None);
    assert_eq!(read.preferences.sidebar_width, 300.0);
}

#[test]
fn keys_this_version_does_not_know_survive_a_save() {
    let payload = default_payload_with(json!({"setting_from_a_newer_version": {"level": 3}}));
    let (store, _directory) = store_with_payload(&payload);

    store
        .replace_user_preferences(&Document::default())
        .unwrap();

    let saved: Value = serde_json::from_str(&stored_payload(&store)).unwrap();
    assert_eq!(saved["setting_from_a_newer_version"], json!({"level": 3}));
}

#[test]
fn keys_this_version_does_not_know_survive_the_shutdown_commit() {
    let payload = default_payload_with(json!({"setting_from_a_newer_version": true}));
    let (store, _directory) = store_with_payload(&payload);

    store
        .database
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Skip,
            user_preferences: Some(Document::default()),
            interrupted_recoverable_tasks: Vec::new(),
            transient_task_ids: Vec::new(),
        })
        .unwrap();

    let saved: Value = serde_json::from_str(&stored_payload(&store)).unwrap();
    assert_eq!(saved["setting_from_a_newer_version"], json!(true));
}

#[test]
fn an_unreadable_payload_is_backed_up_before_it_is_overwritten() {
    let (store, directory) = store_with_payload("{ not json");

    store
        .replace_user_preferences(&Document::default())
        .unwrap();

    let backups = backups(&directory);
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "{ not json");
    assert_eq!(read_back(&store).unreadable, None);
}

#[test]
fn repairing_backs_up_the_original_then_writes_the_repaired_settings() {
    let original = default_payload_with(json!({
        "show_hidden_files": "yes",
        "setting_from_a_newer_version": 7,
    }));
    let (store, directory) = store_with_payload(&original);
    let repaired = Document {
        sidebar_width: 280.0,
        ..Document::default()
    };

    let backup = store
        .replace_unreadable_user_preferences(&original, &repaired)
        .unwrap();

    assert!(backup.write_back.is_ok());
    assert_eq!(backup.path.parent(), Some(directory.path()));
    assert_eq!(fs::read_to_string(&backup.path).unwrap(), original);
    let read = read_back(&store);
    assert_eq!(read.unreadable, None);
    assert_eq!(read.preferences, repaired);
    let saved: Value = serde_json::from_str(&read.raw_payload).unwrap();
    assert_eq!(saved["setting_from_a_newer_version"], json!(7));
}

#[test]
fn repairing_writes_nothing_when_the_backup_cannot_be_written() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let store = UserSettingsStore::new(&database).unwrap();
    store
        .database
        .write()
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, '{', 1)",
            rusqlite::params![USER_PREFERENCES_KEY],
        )
        .unwrap();
    // 备份和数据库在同一目录：目录只读时备份写不进去。
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let result = store.replace_unreadable_user_preferences("{", &Document::default());

    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(stored_payload(&store), "{");
}

#[test]
fn a_failed_write_back_still_reports_the_backup() {
    let (store, directory) = store_with_payload("{");
    // 库拒绝更新设置行：备份写得进目录，写回写不进库。已打开的连接不受文件权限
    // 影响，所以用触发器注入失败。
    rusqlite::Connection::open(store.db_path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_preferences_update
             BEFORE UPDATE ON user_preferences
             BEGIN SELECT RAISE(FAIL, 'injected write-back failure'); END;",
        )
        .unwrap();

    let backup = store
        .replace_unreadable_user_preferences("{", &Document::default())
        .unwrap();

    assert!(backup.write_back.is_err());
    let backups = backups(&directory);
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0], backup.path);
    assert_eq!(fs::read_to_string(&backup.path).unwrap(), "{");
    assert_eq!(stored_payload(&store), "{");
}

#[test]
fn an_unreadable_session_stays_in_place_after_reading() {
    let directory = tempfile::tempdir().unwrap();
    let database = StateDatabase::open(directory.path().join("state.sqlite")).unwrap();
    database
        .write()
        .execute(
            "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
             VALUES (?1, '{', 1)",
            rusqlite::params![crate::browser_session::BROWSER_SESSION_KEY],
        )
        .unwrap();

    assert!(BrowserSessionStore::from(database.clone())
        .read_browser_session()
        .is_err());

    let remaining: i64 = database
        .read()
        .query_row("SELECT COUNT(*) FROM browser_session", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 1);
}
