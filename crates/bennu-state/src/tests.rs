use super::*;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use crate::browser_session::BROWSER_SESSION_KEY;

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "bennu-state-test-{}-{}-{}",
        std::process::id(),
        current_time_ms(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn settings(store: &TaskQueueStore) -> UserSettingsStore {
    store.database().clone().into()
}

fn sessions(store: &TaskQueueStore) -> BrowserSessionStore {
    store.database().clone().into()
}

fn test_store() -> (TaskQueueStore, PathBuf) {
    let root = test_root();
    let store = TaskQueueStore::new(root.join("state.sqlite")).unwrap();
    (store, root)
}

#[test]
fn insert_read_update_and_delete_task() {
    let (store, root) = test_store();
    let operation = StoredOperation::Copy {
        transfers: vec![StoredTransfer {
            source: StoredPath::from_path(Path::new("/tmp/source")),
            target: StoredPath::from_path(Path::new("/tmp/target")),
            conflict_strategy: StoredTransferConflictStrategy::Fail,
        }],
        verification: StoredFileOperationVerification::BasicMetadata,
        recovery_version: None,
    };

    let id = store.insert_task(&operation).unwrap();
    let task = store.read_task(id).unwrap().unwrap();
    assert_eq!(task.id, id);
    assert_eq!(task.operation, operation);
    assert_eq!(task.status, StoredTaskStatus::Pending);
    assert_eq!(task.progress, StoredProgress::pending());
    assert_eq!(task.error, None);

    store.update_status(id, StoredTaskStatus::Running).unwrap();
    store
        .update_progress(id, StoredProgress::with_fraction(0.5))
        .unwrap();
    store.update_error(id, Some("boom")).unwrap();

    let updated = store.read_task(id).unwrap().unwrap();
    assert_eq!(updated.status, StoredTaskStatus::Running);
    assert_eq!(updated.progress, StoredProgress::with_fraction(0.5));
    assert_eq!(updated.error.as_deref(), Some("boom"));

    store
        .update_task_state(
            id,
            StoredTaskStatus::Completed,
            StoredProgress::with_fraction(1.0),
            None,
        )
        .unwrap();
    let completed = store.read_task(id).unwrap().unwrap();
    assert_eq!(completed.status, StoredTaskStatus::Completed);
    assert_eq!(completed.progress, StoredProgress::with_fraction(1.0));
    assert_eq!(completed.error, None);

    store.delete_task(id).unwrap();
    assert!(store.read_task(id).unwrap().is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn marks_unfinished_tasks_failed_without_touching_failed_rows() {
    let (store, root) = test_store();
    let operation = StoredOperation::CreateDirectory {
        parent: StoredPath::from_path(Path::new("/tmp")),
    };
    let pending_id = store.insert_task(&operation).unwrap();
    let failed_id = store.insert_task(&operation).unwrap();
    store
        .update_status(failed_id, StoredTaskStatus::Failed)
        .unwrap();
    store.update_error(failed_id, Some("old error")).unwrap();
    let completed_id = store.insert_task(&operation).unwrap();
    store
        .update_status(completed_id, StoredTaskStatus::Completed)
        .unwrap();
    let canceled_id = store.insert_task(&operation).unwrap();
    store
        .update_status(canceled_id, StoredTaskStatus::Canceled)
        .unwrap();

    store
        .mark_unfinished_tasks_failed("recovered after restart")
        .unwrap();

    let recovered = store.read_task(pending_id).unwrap().unwrap();
    assert_eq!(recovered.status, StoredTaskStatus::Failed);
    assert_eq!(recovered.error.as_deref(), Some("recovered after restart"));

    let already_failed = store.read_task(failed_id).unwrap().unwrap();
    assert_eq!(already_failed.status, StoredTaskStatus::Failed);
    assert_eq!(already_failed.error.as_deref(), Some("old error"));
    let completed = store.read_task(completed_id).unwrap().unwrap();
    assert_eq!(completed.status, StoredTaskStatus::Completed);
    let canceled = store.read_task(canceled_id).unwrap().unwrap();
    assert_eq!(canceled.status, StoredTaskStatus::Canceled);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_tasks_removes_only_selected_rows() {
    let (store, root) = test_store();
    let operation = StoredOperation::CreateDirectory {
        parent: StoredPath::from_path(Path::new("/tmp")),
    };
    let first_id = store.insert_task(&operation).unwrap();
    let kept_id = store.insert_task(&operation).unwrap();
    let third_id = store.insert_task(&operation).unwrap();

    store.delete_tasks(&[first_id, third_id]).unwrap();

    assert!(store.read_task(first_id).unwrap().is_none());
    assert!(store.read_task(third_id).unwrap().is_none());
    assert!(store.read_task(kept_id).unwrap().is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_tasks_rolls_back_when_any_id_is_invalid() {
    let (store, root) = test_store();
    let operation = StoredOperation::CreateDirectory {
        parent: StoredPath::from_path(Path::new("/tmp")),
    };
    let first_id = store.insert_task(&operation).unwrap();
    let second_id = store.insert_task(&operation).unwrap();

    assert!(store.delete_tasks(&[first_id, u64::MAX]).is_err());

    assert!(store.read_task(first_id).unwrap().is_some());
    assert!(store.read_task(second_id).unwrap().is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn clear_tasks_removes_all_rows() {
    let (store, root) = test_store();
    let operation = StoredOperation::CreateDirectory {
        parent: StoredPath::from_path(Path::new("/tmp")),
    };
    store.insert_task(&operation).unwrap();
    store.insert_task(&operation).unwrap();

    store.clear_tasks().unwrap();

    assert!(store.read_tasks().unwrap().is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_permanently_operation_roundtrips_through_json_and_database() {
    let (store, root) = test_store();
    let operation = StoredOperation::DeletePermanently {
        paths: vec![
            StoredPath::from_path(Path::new("/tmp/network/file.txt")),
            StoredPath::from_path(Path::new("/tmp/network/folder")),
        ],
    };
    let json = serde_json::to_string(&operation).unwrap();
    let decoded: StoredOperation = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, operation);
    assert_eq!(operation.kind(), "delete_permanently");

    let id = store.insert_task(&operation).unwrap();
    let task = store.read_task(id).unwrap().unwrap();

    assert_eq!(task.operation, operation);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn rename_operation_keeps_non_utf8_new_name_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let (store, root) = test_store();
    let new_name = OsString::from_vec(b"caf\xe9.txt".to_vec());
    let operation = StoredOperation::Rename {
        path: StoredPath::from_path(Path::new("/docs/old.txt")),
        new_name: StoredFileName::from_os_str(&new_name),
    };

    let id = store.insert_task(&operation).unwrap();
    let task = store.read_task(id).unwrap().unwrap();

    let StoredOperation::Rename {
        new_name: stored, ..
    } = task.operation
    else {
        panic!("expected a rename operation");
    };
    assert_eq!(stored.to_os_string(), new_name);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rename_rows_written_with_text_new_names_still_decode() {
    let (store, root) = test_store();
    let legacy_payload = r#"{"kind":"rename","path":{"encoding":"utf8","value":"/docs/old.txt"},"new_name":"new.txt"}"#;
    store
        .database
        .write()
        .execute(
            "INSERT INTO task_queue (
                operation_kind, payload_json, status, progress_fraction, error,
                created_at_ms, updated_at_ms
            ) VALUES ('rename', ?1, 'failed', NULL, NULL, 0, 0)",
            params![legacy_payload],
        )
        .unwrap();

    let tasks = store.read_tasks().unwrap();

    assert_eq!(tasks.len(), 1);
    let StoredOperation::Rename { new_name, .. } = &tasks[0].operation else {
        panic!("expected a rename operation");
    };
    assert_eq!(new_name.to_os_string(), OsString::from("new.txt"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ungather_rows_written_before_created_directories_still_decode() {
    let legacy_payload = r#"{"kind":"ungather_new_folder","directory":{"encoding":"utf8","value":"/docs/folder"},"restore_targets":[{"encoding":"utf8","value":"/docs/a.txt"}]}"#;

    let decoded: StoredOperation = serde_json::from_str(legacy_payload).unwrap();

    let StoredOperation::UngatherNewFolder {
        directory,
        restore_targets,
        created_directories,
    } = decoded
    else {
        panic!("expected an ungather operation, got {decoded:?}");
    };
    assert_eq!(directory.to_path_buf(), PathBuf::from("/docs/folder"));
    assert_eq!(restore_targets.len(), 1);
    assert!(created_directories.is_empty());
}

#[test]
fn ungather_operation_roundtrips_created_directories() {
    let (store, root) = test_store();
    let operation = StoredOperation::UngatherNewFolder {
        directory: StoredPath::from_path(Path::new("/docs/p/q")),
        restore_targets: vec![StoredPath::from_path(Path::new("/docs/a.txt"))],
        created_directories: vec![
            StoredPath::from_path(Path::new("/docs/p")),
            StoredPath::from_path(Path::new("/docs/p/q")),
        ],
    };

    let id = store.insert_task(&operation).unwrap();
    let task = store.read_task(id).unwrap().unwrap();

    assert_eq!(task.operation, operation);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn column_widths_roundtrip_replace_and_clear() {
    let (store, root) = test_store();
    settings(&store)
        .replace_column_widths(HashMap::from([(0, 240.5), (2, 360.0)]), Some(1_920.0))
        .unwrap();

    assert_eq!(
        settings(&store).read_column_widths().unwrap(),
        HashMap::from([(0, 240.5), (2, 360.0)])
    );
    assert_eq!(
        settings(&store)
            .read_column_width_reference_content_width()
            .unwrap(),
        Some(1_920.0)
    );

    settings(&store)
        .replace_column_widths(HashMap::from([(1, 128.0)]), Some(1_280.0))
        .unwrap();

    assert_eq!(
        settings(&store).read_column_widths().unwrap(),
        HashMap::from([(1, 128.0)])
    );
    assert_eq!(
        settings(&store)
            .read_column_width_reference_content_width()
            .unwrap(),
        Some(1_280.0)
    );

    settings(&store)
        .replace_column_widths(HashMap::new(), None)
        .unwrap();

    assert!(settings(&store).read_column_widths().unwrap().is_empty());
    assert_eq!(
        settings(&store)
            .read_column_width_reference_content_width()
            .unwrap(),
        None
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn column_width_reference_roundtrip_without_widths() {
    let (store, root) = test_store();
    settings(&store)
        .replace_column_widths(HashMap::new(), Some(800.0))
        .unwrap();

    assert!(settings(&store).read_column_widths().unwrap().is_empty());
    assert_eq!(
        settings(&store)
            .read_column_width_reference_content_width()
            .unwrap(),
        Some(800.0)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn clear_tasks_keeps_column_widths() {
    let (store, root) = test_store();
    let operation = StoredOperation::CreateDirectory {
        parent: StoredPath::from_path(Path::new("/tmp")),
    };
    store.insert_task(&operation).unwrap();
    settings(&store)
        .replace_column_widths(HashMap::from([(0, 240.0), (2, 360.0)]), Some(1_600.0))
        .unwrap();

    store.clear_tasks().unwrap();

    assert!(store.read_tasks().unwrap().is_empty());
    assert_eq!(
        settings(&store).read_column_widths().unwrap(),
        HashMap::from([(0, 240.0), (2, 360.0)])
    );
    assert_eq!(
        settings(&store)
            .read_column_width_reference_content_width()
            .unwrap(),
        Some(1_600.0)
    );
    let _ = fs::remove_dir_all(root);
}

fn stored_browser_session(path: &Path) -> StoredBrowserSession {
    StoredBrowserSession {
        panes: vec![StoredBrowserPane {
            id: 0,
            tabs: vec![StoredBrowserTab {
                id: 0,
                directory: StoredPath::from_path(path),
                is_trash_view: false,
                selected: None,
                selected_paths: Vec::new(),
                deepest_open_column_directory: None,
                expanded_directories: Vec::new(),
                view_mode: StoredBrowserViewMode::List,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
            }],
            active_tab_id: 0,
            column_browser_viewport: StoredColumnBrowserViewport {
                offset_x: 0.0,
                width: 800.0,
            },
            column_viewports: Vec::new(),
        }],
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    }
}

fn stored_recoverable_copy() -> StoredOperation {
    StoredOperation::Copy {
        transfers: vec![StoredTransfer {
            source: StoredPath::from_path(Path::new("/tmp/source")),
            target: StoredPath::from_path(Path::new("/tmp/target")),
            conflict_strategy: StoredTransferConflictStrategy::Fail,
        }],
        verification: StoredFileOperationVerification::BasicMetadata,
        recovery_version: Some(TRANSFER_JOURNAL_VERSION),
    }
}

#[test]
fn application_shutdown_commits_session_recovery_and_transient_cleanup_atomically() {
    let (store, root) = test_store();
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&stored_recoverable_copy())
        .unwrap();
    let recoverable_task_id = claimed.task_id;
    drop(claimed.runner_lease);
    let transient_task_id = store
        .insert_task(&StoredOperation::CreateDirectory {
            parent: StoredPath::from_path(Path::new("/tmp")),
        })
        .unwrap();
    let session = stored_browser_session(Path::new("/tmp/final-session"));

    store
        .database()
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(session.clone()),
            user_preferences: Some(SettingsDocument {
                search_history: vec!["report".to_owned()],
            }),
            interrupted_recoverable_tasks: vec![StoredInterruptedRecoverableTask {
                task_id: recoverable_task_id,
                status: StoredTaskStatus::RecoveryPending,
                progress: StoredProgress::with_fraction(0.25),
                error: Some("application stopped".to_owned()),
            }],
            transient_task_ids: vec![transient_task_id],
        })
        .unwrap();

    assert_eq!(
        sessions(&store).read_browser_session().unwrap(),
        Some(session)
    );
    assert_eq!(
        stored_preferences(&store).unwrap().search_history,
        ["report"]
    );
    assert_eq!(
        store
            .read_task(recoverable_task_id)
            .unwrap()
            .unwrap()
            .status,
        StoredTaskStatus::RecoveryPending
    );
    assert!(store.read_task(transient_task_id).unwrap().is_none());
    assert!(!store
        .read_transfer_recovery(recoverable_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn application_shutdown_rejects_transient_id_with_recovery_and_rolls_back() {
    let (store, root) = test_store();
    let previous_session = stored_browser_session(Path::new("/tmp/previous-session"));
    sessions(&store)
        .replace_browser_session(&previous_session)
        .unwrap();
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&stored_recoverable_copy())
        .unwrap();
    let recoverable_task_id = claimed.task_id;
    drop(claimed.runner_lease);

    let error = store
        .database()
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(stored_browser_session(
                Path::new("/tmp/new-session"),
            )),
            user_preferences: None::<SettingsDocument>,
            interrupted_recoverable_tasks: Vec::new(),
            transient_task_ids: vec![recoverable_task_id],
        })
        .unwrap_err();

    assert!(matches!(error, StoreError::InvalidRecoverableOperation(_)));
    assert_eq!(
        sessions(&store).read_browser_session().unwrap(),
        Some(previous_session)
    );
    assert!(store.read_task(recoverable_task_id).unwrap().is_some());
    assert!(!store
        .read_transfer_recovery(recoverable_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn application_shutdown_rolls_back_prior_task_writes_when_session_write_fails() {
    let (store, root) = test_store();
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&stored_recoverable_copy())
        .unwrap();
    let recoverable_task_id = claimed.task_id;
    drop(claimed.runner_lease);
    let transient_task_id = store
        .insert_task(&StoredOperation::CreateDirectory {
            parent: StoredPath::from_path(Path::new("/tmp")),
        })
        .unwrap();
    let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_shutdown_session
             BEFORE INSERT ON browser_session
             BEGIN
                 SELECT RAISE(FAIL, 'injected shutdown session failure');
             END;",
        )
        .unwrap();

    let error = store
        .database()
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(stored_browser_session(
                Path::new("/tmp/new-session"),
            )),
            user_preferences: Some(SettingsDocument {
                search_history: vec!["report".to_owned()],
            }),
            interrupted_recoverable_tasks: vec![StoredInterruptedRecoverableTask {
                task_id: recoverable_task_id,
                status: StoredTaskStatus::RecoveryPending,
                progress: StoredProgress::with_fraction(0.5),
                error: Some("application stopped".to_owned()),
            }],
            transient_task_ids: vec![transient_task_id],
        })
        .unwrap_err();

    assert!(matches!(error, StoreError::Sqlite(_)));
    assert_eq!(
        store
            .read_task(recoverable_task_id)
            .unwrap()
            .unwrap()
            .status,
        StoredTaskStatus::Pending
    );
    assert!(store.read_task(transient_task_id).unwrap().is_some());
    assert!(sessions(&store).read_browser_session().unwrap().is_none());
    assert!(stored_preferences(&store).is_none());
    assert!(!store
        .read_transfer_recovery(recoverable_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn browser_session_roundtrip_replace_and_clear() {
    let (store, root) = test_store();
    let first = StoredBrowserSession {
        panes: vec![StoredBrowserPane {
            id: 0,
            tabs: vec![StoredBrowserTab {
                id: 0,
                directory: StoredPath::from_path(Path::new("/home/user")),
                is_trash_view: false,
                selected: Some(StoredPath::from_path(Path::new("/home/user/file.txt"))),
                selected_paths: vec![StoredPath::from_path(Path::new("/home/user/file.txt"))],
                deepest_open_column_directory: Some(StoredPath::from_path(Path::new(
                    "/home/user/Documents",
                ))),
                expanded_directories: vec![StoredPath::from_path(Path::new("/home/user/Projects"))],
                view_mode: StoredBrowserViewMode::Icons,
                back_stack: vec![StoredPath::from_path(Path::new("/home/user/Downloads"))],
                forward_stack: Vec::new(),
            }],
            active_tab_id: 0,
            column_browser_viewport: StoredColumnBrowserViewport {
                offset_x: 320.0,
                width: 900.0,
            },
            column_viewports: vec![StoredColumnViewport {
                directory: StoredPath::from_path(Path::new("/home/user")),
                offset_y: 2.0,
                height: 400.0,
            }],
        }],
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    };

    sessions(&store).replace_browser_session(&first).unwrap();
    assert_eq!(
        sessions(&store).read_browser_session().unwrap(),
        Some(first.clone())
    );

    let second = StoredBrowserSession {
        panes: Vec::new(),
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    };
    sessions(&store).replace_browser_session(&second).unwrap();
    assert_eq!(
        sessions(&store).read_browser_session().unwrap(),
        Some(second)
    );

    sessions(&store).clear_browser_session().unwrap();
    assert_eq!(sessions(&store).read_browser_session().unwrap(), None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn icon_browser_view_mode_uses_stable_json_value() {
    assert_eq!(
        serde_json::to_string(&StoredBrowserViewMode::Icons).unwrap(),
        "\"icons\""
    );
    assert_eq!(
        serde_json::from_str::<StoredBrowserViewMode>("\"icons\"").unwrap(),
        StoredBrowserViewMode::Icons
    );
}

/// 设置文档的最小替身：存储层只关心"一个 JSON 对象"，具体字段由应用定义。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct SettingsDocument {
    #[serde(default)]
    search_history: Vec<String>,
}

fn stored_preferences(store: &TaskQueueStore) -> Option<SettingsDocument> {
    settings(store)
        .read_user_preferences(&SettingsDocument::default())
        .unwrap()
        .map(|read| read.preferences)
}

#[test]
fn browser_session_missing_column_browser_viewport_defaults_to_start() {
    let (store, root) = test_store();
    let session = StoredBrowserSession {
        panes: vec![StoredBrowserPane {
            id: 0,
            tabs: vec![StoredBrowserTab {
                id: 0,
                directory: StoredPath::from_path(Path::new("/home/user")),
                is_trash_view: false,
                selected: None,
                selected_paths: Vec::new(),
                deepest_open_column_directory: None,
                expanded_directories: Vec::new(),
                view_mode: StoredBrowserViewMode::Columns,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
            }],
            active_tab_id: 0,
            column_browser_viewport: StoredColumnBrowserViewport {
                offset_x: 480.0,
                width: 900.0,
            },
            column_viewports: Vec::new(),
        }],
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    };
    let mut payload = serde_json::to_value(&session).unwrap();
    payload["panes"][0]
        .as_object_mut()
        .unwrap()
        .remove("column_browser_viewport");
    let payload_json = serde_json::to_string(&payload).unwrap();
    let connection = Connection::open(store.db_path()).unwrap();
    connection
        .execute(
            "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![BROWSER_SESSION_KEY, payload_json, 1_i64],
        )
        .unwrap();
    drop(connection);

    let restored = sessions(&store).read_browser_session().unwrap().unwrap();

    assert_eq!(
        restored.panes[0].column_browser_viewport,
        StoredColumnBrowserViewport::default()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unreadable_old_task_payload_rows_are_deleted_on_read() {
    let (store, root) = test_store();
    let connection = Connection::open(store.db_path()).unwrap();
    connection
        .execute(
            "INSERT INTO task_queue (
                operation_kind, payload_json, status, progress_fraction, error,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, NULL, NULL, 1, 1)",
            params![
                "search_index",
                r#"{"kind":"search_index","root":{"encoding":"utf8","value":"/workspace"},"index_dir":{"encoding":"utf8","value":"/cache/old"},"selected_paths":[],"include_hidden":false}"#,
                StoredTaskStatus::Pending.as_str(),
            ],
        )
        .unwrap();
    drop(connection);

    assert!(store.read_tasks().unwrap().is_empty());
    let connection = Connection::open(store.db_path()).unwrap();
    let task_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM task_queue", [], |row| row.get(0))
        .unwrap();
    assert_eq!(task_count, 0);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn non_utf8_unix_path_roundtrips_through_json_and_database() {
    let (store, root) = test_store();
    let path = PathBuf::from(OsString::from_vec(b"/tmp/non-utf8-\xFF".to_vec()));
    let operation = StoredOperation::Trash {
        paths: vec![StoredPath::from_path(&path)],
    };
    let json = serde_json::to_string(&operation).unwrap();
    let decoded: StoredOperation = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, operation);

    let id = store.insert_task(&operation).unwrap();
    let task = store.read_task(id).unwrap().unwrap();
    let StoredOperation::Trash { paths } = task.operation else {
        panic!("expected trash operation");
    };
    assert_eq!(
        paths[0].to_path_buf().as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn browser_session_preserves_non_utf8_unix_paths() {
    let (store, root) = test_store();
    let path = PathBuf::from(OsString::from_vec(b"/tmp/session-\xFF".to_vec()));
    let session = StoredBrowserSession {
        panes: vec![StoredBrowserPane {
            id: 0,
            tabs: vec![StoredBrowserTab {
                id: 0,
                directory: StoredPath::from_path(&path),
                is_trash_view: false,
                selected: None,
                selected_paths: Vec::new(),
                deepest_open_column_directory: None,
                expanded_directories: Vec::new(),
                view_mode: StoredBrowserViewMode::Columns,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
            }],
            active_tab_id: 0,
            column_browser_viewport: StoredColumnBrowserViewport::default(),
            column_viewports: Vec::new(),
        }],
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    };

    sessions(&store).replace_browser_session(&session).unwrap();

    let restored = sessions(&store).read_browser_session().unwrap().unwrap();
    let restored_path = restored.panes[0].tabs[0].directory.to_path_buf();
    assert_eq!(
        restored_path.as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
    let _ = fs::remove_dir_all(root);
}
