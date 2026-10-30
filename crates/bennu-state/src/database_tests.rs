use std::sync::mpsc;
use std::time::Duration;

use rusqlite::Connection;
use tempfile::tempdir;

use super::*;
use crate::{
    BrowserSessionStore, StoredOperation, StoredPath, StoredTaskStatus, TaskQueueStore,
    UserSettingsStore,
};

/// 上一版发布时建库用的语句原文。故意不引用 `crate::SCHEMA_SQL`：本测试要证明的是
/// 新版本打开一个旧版本建出来的库时，既不改它的结构，也读得出里面的数据。
const PREVIOUS_RELEASE_SCHEMA: &str = r#"
CREATE TABLE task_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    progress_fraction REAL,
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE ui_column_view_preferences (
    preference_key TEXT PRIMARY KEY,
    value_real REAL NOT NULL
);
CREATE TABLE browser_session (
    session_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE user_preferences (
    preference_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE transfer_journal (
    task_id INTEGER NOT NULL REFERENCES task_queue(id) ON DELETE CASCADE,
    transfer_index INTEGER NOT NULL,
    relative_path_json TEXT NOT NULL,
    operation_kind TEXT NOT NULL,
    source_path_json TEXT NOT NULL,
    requested_target_path_json TEXT NOT NULL,
    conflict_strategy TEXT NOT NULL,
    verification TEXT NOT NULL,
    checkpoint_kind TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (task_id, transfer_index, relative_path_json)
);
"#;

const PREVIOUS_RELEASE_TASK: &str =
    r#"{"kind":"delete_permanently","paths":[{"encoding":"unix_bytes","bytes":[47,115,114,118]}]}"#;

const PREVIOUS_RELEASE_SESSION: &str = r#"{"panes":[{"id":0,"tabs":[{"id":0,"directory":{"encoding":"unix_bytes","bytes":[47,115,114,118]},"is_trash_view":false,"selected":null,"selected_paths":[],"deepest_open_column_directory":null,"expanded_directories":[],"view_mode":"list","back_stack":[],"forward_stack":[]}],"active_tab_id":0,"column_browser_viewport":{"offset_x":0.0,"width":800.0},"column_viewports":[]}],"layout":{"kind":"single","active":0}}"#;

const PREVIOUS_RELEASE_PREFERENCES: &str = r#"{"network_list_thumbnail_downloads_enabled":false,"show_hidden_files":true,"sidebar_width":248.0,"sidebar_favorites":null,"network_connections":[],"terminal_emulator":"automatic","file_operation_verification":"basic_metadata","browser_view_mode":"columns","startup_location":"home","startup_custom_directory":{"encoding":"unix_bytes","bytes":[]},"save_view_state":false,"shortcuts":[]}"#;

fn table_exists(connection: &Connection, name: &str) -> bool {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

#[test]
fn every_open_of_one_file_shares_one_handle() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    std::fs::create_dir(directory.path().join("nested")).unwrap();

    let first = StateDatabase::open(&path).unwrap();
    let respelled = StateDatabase::open(directory.path().join("nested/../state.sqlite")).unwrap();
    let through_a_store = TaskQueueStore::new(&path).unwrap();

    assert!(Arc::ptr_eq(&first.open, &respelled.open));
    assert!(Arc::ptr_eq(&first.open, &through_a_store.database.open));
}

#[test]
fn tables_are_created_once_while_the_database_stays_open() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let first = StateDatabase::open(&path).unwrap();
    let outside = Connection::open(&path).unwrap();
    outside
        .execute_batch("DROP TABLE ui_column_view_preferences")
        .unwrap();

    let second = StateDatabase::open(&path).unwrap();
    assert!(
        !table_exists(&outside, "ui_column_view_preferences"),
        "opening an already open database must not run the schema again"
    );

    drop((first, second));
    let reopened = StateDatabase::open(&path).unwrap();
    assert!(table_exists(&outside, "ui_column_view_preferences"));
    drop(reopened);
}

#[test]
fn the_read_connection_refuses_writes() {
    let directory = tempdir().unwrap();
    let database = StateDatabase::open(directory.path().join("state.sqlite")).unwrap();

    assert!(database
        .read()
        .execute("DELETE FROM task_queue", [])
        .is_err());
    assert!(database
        .write()
        .execute("DELETE FROM task_queue", [])
        .is_ok());
}

#[test]
fn reading_tasks_is_not_blocked_by_an_open_write_transaction() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    store
        .insert_task(&StoredOperation::EmptyTrash)
        .expect("insert task");

    let writer = store.database.write();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader_store = store.clone();
    std::thread::spawn(move || {
        let _ = sender.send(reader_store.read_tasks());
    });
    // 读取与写入共用一把锁时，这里会一直等到超时。
    let tasks = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("a read must not wait for the writer")
        .unwrap();
    writer.execute_batch("ROLLBACK").unwrap();

    assert_eq!(tasks.len(), 1);
}

#[test]
fn a_database_from_the_previous_release_opens_unchanged() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let previous = Connection::open(&path).unwrap();
    previous.pragma_update(None, "journal_mode", "WAL").unwrap();
    previous.execute_batch(PREVIOUS_RELEASE_SCHEMA).unwrap();
    previous
        .execute(
            "INSERT INTO task_queue (id, operation_kind, payload_json, status, progress_fraction,
                                     error, created_at_ms, updated_at_ms)
             VALUES (7, 'delete_permanently', ?1, 'completed', 1.0, NULL, 10, 20)",
            [PREVIOUS_RELEASE_TASK],
        )
        .unwrap();
    previous
        .execute_batch(
            "INSERT INTO ui_column_view_preferences VALUES ('column_width.0', 222.0);
             INSERT INTO ui_column_view_preferences
                 VALUES ('column_width.reference_content_width', 1000.0);",
        )
        .unwrap();
    previous
        .execute(
            "INSERT INTO browser_session VALUES ('main', ?1, 30)",
            [PREVIOUS_RELEASE_SESSION],
        )
        .unwrap();
    previous
        .execute(
            "INSERT INTO user_preferences VALUES ('main', ?1, 40)",
            [PREVIOUS_RELEASE_PREFERENCES],
        )
        .unwrap();

    let database = StateDatabase::open(&path).unwrap();
    let store = TaskQueueStore::from(database.clone());
    let settings = UserSettingsStore::from(database.clone());
    let sessions = BrowserSessionStore::from(database);

    let tasks = store.read_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, 7);
    assert_eq!(tasks[0].status, StoredTaskStatus::Completed);
    assert_eq!(
        tasks[0].operation,
        StoredOperation::DeletePermanently {
            paths: vec![StoredPath::from_path(std::path::Path::new("/srv"))],
        }
    );
    assert_eq!(settings.read_column_widths().unwrap().get(&0), Some(&222.0));
    assert_eq!(
        settings
            .read_column_width_reference_content_width()
            .unwrap(),
        Some(1000.0)
    );
    let session = sessions
        .read_browser_session()
        .unwrap()
        .expect("session row");
    assert_eq!(session.panes.len(), 1);
    // 文档字段由应用定义；这里只确认旧行的内容原样读得出来。
    let preferences = settings
        .read_user_preferences(&serde_json::json!({}))
        .unwrap()
        .expect("preferences row");
    assert!(preferences.unreadable.is_none());
    assert_eq!(preferences.preferences["sidebar_width"], 248.0);
    assert_eq!(preferences.preferences["show_hidden_files"], true);
    // 旧库缺的表由新版本补上，已有的表一张不动。
    assert!(table_exists(&previous, "transfer_manifest"));
    assert!(table_exists(&previous, "transfer_journal"));
}
