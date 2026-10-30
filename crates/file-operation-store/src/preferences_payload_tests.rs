use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::{StoredApplicationShutdown, StoredBrowserSessionShutdown, StoredNetworkConnection};

fn store_with_payload(payload: &str) -> (TaskQueueStore, TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    store
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, 1)",
            rusqlite::params![USER_PREFERENCES_KEY, payload],
        )
        .unwrap();
    (store, directory)
}

fn stored_payload(store: &TaskQueueStore) -> String {
    read_payload(&store.connection().unwrap())
        .unwrap()
        .expect("preferences row")
}

fn default_payload_with(changes: Value) -> String {
    let mut payload = default_payload_object();
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

    let read = store.read_user_preferences().unwrap().expect("row exists");

    assert_eq!(read.unreadable, Some(UnreadablePreferences::Whole));
    assert_eq!(read.preferences, StoredUserPreferences::default());
    assert_eq!(read.raw_payload, "{");
    assert_eq!(stored_payload(&store), "{");
}

#[test]
fn a_key_of_the_wrong_type_falls_back_alone() {
    let payload = default_payload_with(json!({
        "show_hidden_files": "yes",
        "sidebar_width": 321.0,
    }));
    let (store, _directory) = store_with_payload(&payload);

    let read = store.read_user_preferences().unwrap().expect("row exists");

    assert_eq!(
        read.unreadable,
        Some(UnreadablePreferences::Keys(vec![
            "show_hidden_files".to_owned()
        ]))
    );
    assert_eq!(read.preferences.sidebar_width, 321.0);
    assert_eq!(
        read.preferences.show_hidden_files,
        StoredUserPreferences::default().show_hidden_files
    );
}

#[test]
fn one_unreadable_list_item_does_not_drop_the_others() {
    let payload = default_payload_with(json!({
        "network_connections": [
            {"id": "nas", "label": "NAS", "protocol": "smb", "uri": "smb://nas/share", "auto_connect": true},
            {"id": 5},
        ],
    }));
    let (store, _directory) = store_with_payload(&payload);

    let read = store.read_user_preferences().unwrap().expect("row exists");

    assert_eq!(
        read.unreadable,
        Some(UnreadablePreferences::Keys(vec![
            "network_connections".to_owned()
        ]))
    );
    assert_eq!(
        read.preferences.network_connections,
        [StoredNetworkConnection {
            id: "nas".to_owned(),
            label: "NAS".to_owned(),
            protocol: "smb".to_owned(),
            uri: "smb://nas/share".to_owned(),
            auto_connect: true,
        }]
    );
}

#[test]
fn a_key_written_before_it_existed_is_not_reported_as_unreadable() {
    let mut payload = default_payload_object();
    payload.remove("show_hidden_files");
    payload.insert("sidebar_width".to_owned(), json!(300.0));
    let (store, _directory) = store_with_payload(&Value::Object(payload).to_string());

    let read = store.read_user_preferences().unwrap().expect("row exists");

    assert_eq!(read.unreadable, None);
    assert_eq!(read.preferences.sidebar_width, 300.0);
}

#[test]
fn keys_this_version_does_not_know_survive_a_save() {
    let payload = default_payload_with(json!({"setting_from_a_newer_version": {"level": 3}}));
    let (store, _directory) = store_with_payload(&payload);

    store
        .replace_user_preferences(&StoredUserPreferences::default())
        .unwrap();

    let saved: Value = serde_json::from_str(&stored_payload(&store)).unwrap();
    assert_eq!(saved["setting_from_a_newer_version"], json!({"level": 3}));
}

#[test]
fn keys_this_version_does_not_know_survive_the_shutdown_commit() {
    let payload = default_payload_with(json!({"setting_from_a_newer_version": true}));
    let (store, _directory) = store_with_payload(&payload);

    store
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Skip,
            user_preferences: Some(StoredUserPreferences::default()),
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
        .replace_user_preferences(&StoredUserPreferences::default())
        .unwrap();

    let backups = backups(&directory);
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "{ not json");
    assert_eq!(
        store.read_user_preferences().unwrap().unwrap().unreadable,
        None
    );
}

#[test]
fn a_backup_is_a_new_file_next_to_the_database() {
    let (store, directory) = store_with_payload("{}");

    let backup = store.back_up_user_preferences("original").unwrap();

    assert_eq!(backup.parent(), Some(directory.path()));
    assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
}

#[test]
fn an_unreadable_session_stays_in_place_after_reading() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    store
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
             VALUES (?1, '{', 1)",
            rusqlite::params![crate::BROWSER_SESSION_KEY],
        )
        .unwrap();

    assert!(store.read_browser_session().is_err());

    let remaining: i64 = store
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM browser_session", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 1);
}
