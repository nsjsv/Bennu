use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "file-operation-store-test-{}-{}-{}",
        std::process::id(),
        current_time_ms(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    ))
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

#[test]
fn column_widths_roundtrip_replace_and_clear() {
    let (store, root) = test_store();
    store
        .replace_column_widths(
            HashMap::from([(0, 240.5), (2, 360.0)]),
            Some(1_920.0),
        )
        .unwrap();

    assert_eq!(
        store.read_column_widths().unwrap(),
        HashMap::from([(0, 240.5), (2, 360.0)])
    );
    assert_eq!(
        store.read_column_width_reference_content_width().unwrap(),
        Some(1_920.0)
    );

    store
        .replace_column_widths(HashMap::from([(1, 128.0)]), Some(1_280.0))
        .unwrap();

    assert_eq!(
        store.read_column_widths().unwrap(),
        HashMap::from([(1, 128.0)])
    );
    assert_eq!(
        store.read_column_width_reference_content_width().unwrap(),
        Some(1_280.0)
    );

    store.replace_column_widths(HashMap::new(), None).unwrap();

    assert!(store.read_column_widths().unwrap().is_empty());
    assert_eq!(
        store.read_column_width_reference_content_width().unwrap(),
        None
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn column_width_reference_roundtrip_without_widths() {
    let (store, root) = test_store();
    store
        .replace_column_widths(HashMap::new(), Some(800.0))
        .unwrap();

    assert!(store.read_column_widths().unwrap().is_empty());
    assert_eq!(
        store.read_column_width_reference_content_width().unwrap(),
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
    store
        .replace_column_widths(HashMap::from([(0, 240.0), (2, 360.0)]), Some(1_600.0))
        .unwrap();

    store.clear_tasks().unwrap();

    assert!(store.read_tasks().unwrap().is_empty());
    assert_eq!(
        store.read_column_widths().unwrap(),
        HashMap::from([(0, 240.0), (2, 360.0)])
    );
    assert_eq!(
        store.read_column_width_reference_content_width().unwrap(),
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
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(session.clone()),
            user_preferences: Some(StoredUserPreferences {
                search_history: vec!["report".to_owned()],
                ..StoredUserPreferences::default()
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

    assert_eq!(store.read_browser_session().unwrap(), Some(session));
    assert_eq!(
        store
            .read_user_preferences()
            .unwrap()
            .unwrap()
            .search_history,
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
    store.replace_browser_session(&previous_session).unwrap();
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&stored_recoverable_copy())
        .unwrap();
    let recoverable_task_id = claimed.task_id;
    drop(claimed.runner_lease);

    let error = store
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(stored_browser_session(
                Path::new("/tmp/new-session"),
            )),
            user_preferences: None,
            interrupted_recoverable_tasks: Vec::new(),
            transient_task_ids: vec![recoverable_task_id],
        })
        .unwrap_err();

    assert!(matches!(error, StoreError::InvalidRecoverableOperation(_)));
    assert_eq!(
        store.read_browser_session().unwrap(),
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
        .commit_application_shutdown(StoredApplicationShutdown {
            browser_session: StoredBrowserSessionShutdown::Persist(stored_browser_session(
                Path::new("/tmp/new-session"),
            )),
            user_preferences: Some(StoredUserPreferences {
                search_history: vec!["report".to_owned()],
                ..StoredUserPreferences::default()
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
    assert!(store.read_browser_session().unwrap().is_none());
    assert!(store.read_user_preferences().unwrap().is_none());
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

    store.replace_browser_session(&first).unwrap();
    assert_eq!(store.read_browser_session().unwrap(), Some(first.clone()));

    let second = StoredBrowserSession {
        panes: Vec::new(),
        layout: StoredBrowserPaneLayout::Single { active: 0 },
    };
    store.replace_browser_session(&second).unwrap();
    assert_eq!(store.read_browser_session().unwrap(), Some(second));

    store.clear_browser_session().unwrap();
    assert_eq!(store.read_browser_session().unwrap(), None);
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

#[test]
fn user_preferences_roundtrip_replace() {
    let (store, root) = test_store();
    assert_eq!(store.read_user_preferences().unwrap(), None);
    let defaults = StoredUserPreferences::default();
    assert_eq!(defaults.preview_text_size_bytes, None);
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
        max_preview_file_bytes: None,
        preview_text_size_bytes: Some(8 * 1024 * 1024),
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
        icon_grid_size: 160,
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
    assert_eq!(store.read_user_preferences().unwrap(), Some(first.clone()));

    let second = StoredUserPreferences {
        show_hidden_files: false,
        sidebar_favorites: Some(Vec::new()),
        network_connections: Vec::new(),
        shortcuts: Vec::new(),
        ..first
    };
    store.replace_user_preferences(&second).unwrap();

    assert_eq!(store.read_user_preferences().unwrap(), Some(second));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unreadable_user_preferences_payload_is_deleted_on_read() {
    let (store, root) = test_store();
    let connection = Connection::open(store.db_path()).unwrap();
    connection
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![user_preferences::USER_PREFERENCES_KEY, "{", 1_i64],
        )
        .unwrap();
    drop(connection);

    assert!(store.read_user_preferences().is_err());
    let connection = Connection::open(store.db_path()).unwrap();
    let preference_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM user_preferences", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(preference_count, 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_user_preferences_without_optional_fields_get_defaults() {
    let (store, root) = test_store();
    let mut payload = serde_json::to_value(StoredUserPreferences::default()).unwrap();
    let object = payload.as_object_mut().expect("preferences payload object");
    object.remove("list_view_columns");
    object.remove("list_sort_field");
    object.remove("list_sort_direction");
    object.remove("list_directory_size_display_mode");
    object.remove("icon_grid_size");
    object.remove("columns_view_density");
    object.remove("list_view_density");
    object.remove("icons_view_density");
    object.remove("window_chrome_layout");
    object.remove("window_controls");
    object.remove("search_history");
    object.remove("theme_mode");
    object.remove("color_scheme");
    object.remove("visible_column_count");
    let payload_json = serde_json::to_string(&payload).unwrap();
    let connection = Connection::open(store.db_path()).unwrap();
    connection
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![user_preferences::USER_PREFERENCES_KEY, payload_json, 1_i64],
        )
        .unwrap();
    drop(connection);

    let preferences = store
        .read_user_preferences()
        .unwrap()
        .expect("preferences load");

    assert_eq!(preferences.list_view_columns.len(), 9);
    assert_eq!(preferences.list_view_columns[0].kind, "name");
    assert_eq!(preferences.list_view_columns[4].kind, "owner");
    assert!(!preferences.list_view_columns[4].visible);
    assert_eq!(preferences.list_sort_field, "name");
    assert_eq!(preferences.list_sort_direction, "ascending");
    assert_eq!(preferences.list_directory_size_display_mode, "item_count");
    assert_eq!(preferences.language_setting, "system");
    assert_eq!(preferences.icon_grid_size, 96);
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
    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_user_preferences_ignore_removed_search_fields() {
    let (store, root) = test_store();
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
    let connection = Connection::open(store.db_path()).unwrap();
    connection
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![user_preferences::USER_PREFERENCES_KEY, payload_json, 1_i64],
        )
        .unwrap();
    drop(connection);

    let preferences = store
        .read_user_preferences()
        .unwrap()
        .expect("preferences load");

    assert!(preferences.network_list_thumbnail_downloads_enabled);
    assert_eq!(preferences.language_setting, "chinese");
    assert_eq!(preferences.icon_grid_size, 96);
    assert_eq!(preferences.max_preview_file_bytes, Some(4194304));
    assert_eq!(preferences.preview_text_size_bytes, None);
    let _ = fs::remove_dir_all(root);
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

    let restored = store.read_browser_session().unwrap().unwrap();

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

    store.replace_browser_session(&session).unwrap();

    let restored = store.read_browser_session().unwrap().unwrap();
    let restored_path = restored.panes[0].tabs[0].directory.to_path_buf();
    assert_eq!(
        restored_path.as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
    let _ = fs::remove_dir_all(root);
}
