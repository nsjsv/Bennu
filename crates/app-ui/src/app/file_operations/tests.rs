use std::collections::HashSet;
use std::path::PathBuf;

use file_search::SearchScope;

use super::*;
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserTab, ExpandedDirectory, ExpandedDirectoryStatus,
    ListDirectorySummary, SplitAxis,
};
use crate::operation_history::FileOperationOutcome;
use crate::shortcuts::FileSelectionDirection;
use crate::thumbnail_cache::ColumnViewport;

fn remember_summary(browser: &mut FileBrowser, path: &std::path::Path, count: usize, size: u64) {
    browser
        .list_directory_summary_cache
        .remember_direct_child_count(path.to_path_buf(), count);
    let request = browser
        .list_directory_summary_cache
        .start_request(path.to_path_buf(), true)
        .expect("recursive request");
    assert!(browser.list_directory_summary_cache.store_summary(
        &request,
        ListDirectorySummary {
            direct_child_count: count,
            recursive_total_size_bytes: Some(size),
            files_total_size_bytes: Some(size),
            hidden_files_total_size_bytes: Some(0),
        }
    ));
}

fn finish_queued_rename(browser: &mut FileBrowser, from: PathBuf, to: PathBuf) {
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Rename {
            path: from.clone(),
            new_name: to
                .file_name()
                .expect("rename target name")
                .to_string_lossy()
                .into_owned(),
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued rename")
        .id;
    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename { from, to }),
    ));
}

fn direct_move_commit_message(
    task_id: u64,
    source: PathBuf,
    target: PathBuf,
    checkpoint_revision: u64,
) -> Message {
    Message::FileOperationDirectMovesCommitted {
        task_id,
        commits: vec![crate::commands::DurableDirectMoveCommit::for_test(
            file_core::TransferWorkKey::top_level(0),
            source,
            target,
            checkpoint_revision,
        )],
    }
}

fn direct_move_commit_batch_message(
    task_id: u64,
    commits: Vec<(usize, PathBuf, PathBuf, u64)>,
) -> Message {
    Message::FileOperationDirectMovesCommitted {
        task_id,
        commits: commits
            .into_iter()
            .map(|(transfer_index, source, target, checkpoint_revision)| {
                crate::commands::DurableDirectMoveCommit::for_test(
                    file_core::TransferWorkKey::top_level(transfer_index as u64),
                    source,
                    target,
                    checkpoint_revision,
                )
            })
            .collect(),
    }
}

fn direct_move_renamed_batch_message(moves: Vec<(PathBuf, PathBuf)>) -> Message {
    Message::FileOperationMovesRenamed {
        task_id: 0,
        moves: moves
            .into_iter()
            .map(|(source, target)| crate::operation_history::CompletedTransfer { source, target })
            .collect(),
    }
}

fn loaded_expanded_directory() -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

#[test]
fn file_operation_events_preserve_queue_panel_visibility() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let operation_directory = tempfile::tempdir().expect("create operation directory");
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(operation_directory.path().join("state.sqlite"))
            .expect("create operation store"),
    );

    drop(browser.enqueue_file_operation(QueuedFileOperation::Copy {
        transfers: vec![crate::operation_queue::QueuedTransfer::new(
            PathBuf::from("/workspace/copy-source"),
            PathBuf::from("/workspace/copy-target"),
        )],
        verification: file_core::FileOperationVerification::default(),
    }));
    assert!(!browser.operation_queue.is_panel_open());
    assert_eq!(browser.operation_queue.unread_count(), 1);

    drop(browser.update(Message::FileOperationIndicatorPressed));
    assert!(browser.operation_queue.is_panel_open());
    assert_eq!(browser.operation_queue.unread_count(), 0);

    drop(browser.enqueue_file_operation(QueuedFileOperation::Move {
        transfers: vec![crate::operation_queue::QueuedTransfer::new(
            PathBuf::from("/workspace/move-source"),
            PathBuf::from("/workspace/move-target"),
        )],
        verification: file_core::FileOperationVerification::default(),
    }));
    assert!(browser.operation_queue.is_panel_open());
    assert_eq!(browser.operation_queue.unread_count(), 0);

    drop(browser.update(Message::FileOperationIndicatorPressed));
    assert!(!browser.operation_queue.is_panel_open());

    drop(
        browser.enqueue_file_operation(QueuedFileOperation::CreateDirectory {
            parent: PathBuf::from("/workspace"),
        }),
    );
    assert!(!browser.operation_queue.is_panel_open());
    assert_eq!(browser.operation_queue.unread_count(), 1);
}

#[test]
fn progress_and_completion_keep_queue_panel_closed() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(
        browser.enqueue_file_operation(QueuedFileOperation::CreateDirectory {
            parent: PathBuf::from("/workspace"),
        }),
    );
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued operation")
        .id;
    assert!(!browser.operation_queue.is_panel_open());

    drop(browser.update(Message::FileOperationProgressed(
        task_id,
        crate::operation_progress::FileOperationProgressUpdate::IndeterminateItems {
            completed: 1,
            total: 2,
        },
    )));
    assert!(!browser.operation_queue.is_panel_open());

    drop(browser.update(Message::FileOperationFinished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    )));
    assert!(!browser.operation_queue.is_panel_open());
}

#[test]
fn rename_input_history_undoes_and_redoes_complete_snapshots() {
    let mut history = RenameInputHistory::default();
    let mut current_value = String::from("report.txt");

    history.apply_input_change(&mut current_value, String::from("report-1.txt"));
    history.apply_input_change(&mut current_value, String::from("report-2.txt"));
    history.undo(&mut current_value);
    assert_eq!(current_value, "report-1.txt");
    history.undo(&mut current_value);
    assert_eq!(current_value, "report.txt");
    history.redo(&mut current_value);
    assert_eq!(current_value, "report-1.txt");
    history.redo(&mut current_value);
    assert_eq!(current_value, "report-2.txt");
}

#[test]
fn rename_input_history_clears_redo_branch_after_new_input() {
    let mut history = RenameInputHistory::default();
    let mut current_value = String::from("report.txt");

    history.apply_input_change(&mut current_value, String::from("report-1.txt"));
    history.undo(&mut current_value);
    history.apply_input_change(&mut current_value, String::from("report-final.txt"));
    history.redo(&mut current_value);

    assert_eq!(current_value, "report-final.txt");
}

#[test]
fn beginning_new_rename_session_resets_input_history() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.begin_rename(PathBuf::from("/workspace/first.txt")));
    drop(browser.apply_rename_input_change(String::from("first-draft.txt")));
    drop(browser.begin_rename(PathBuf::from("/workspace/second.txt")));
    drop(browser.undo_rename_input_change());

    assert_eq!(browser.rename_input, "second.txt");

    drop(browser.apply_rename_input_change(String::from("second-draft.txt")));
    drop(browser.undo_rename_input_change());
    drop(browser.begin_rename(PathBuf::from("/workspace/third.txt")));
    drop(browser.redo_rename_input_change());

    assert_eq!(browser.rename_input, "third.txt");
}

#[test]
fn completed_background_operation_preserves_active_rename_history() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let edited_path = PathBuf::from("/workspace/report.txt");

    drop(browser.begin_rename(edited_path.clone()));
    drop(browser.apply_rename_input_change(String::from("report-draft.txt")));
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::DeletePermanently {
            paths: vec![PathBuf::from("/workspace/obsolete.txt")],
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued task")
        .id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    ));

    assert_eq!(browser.renaming, Some(edited_path));
    assert_eq!(browser.rename_input, "report-draft.txt");
    drop(browser.undo_rename_input_change());
    assert_eq!(browser.rename_input, "report.txt");
}

#[test]
fn duplicate_completion_is_rejected_before_history_side_effects() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/old.txt");
    let destination = PathBuf::from("/workspace/new.txt");
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Rename {
            path: source.clone(),
            new_name: "new.txt".to_owned(),
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued rename")
        .id;
    let completion = FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
        from: source,
        to: destination,
    });

    drop(browser.accept_file_operation_finished(task_id, completion.clone()));
    drop(browser.accept_file_operation_finished(task_id, completion));

    assert!(browser.operation_history.take_undo_operation().is_some());
    assert!(browser.operation_history.take_undo_operation().is_none());
}

#[test]
fn moved_renames_migrate_paths_before_terminal_completion() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("old");
    let destination = workspace.path().join("new");
    browser.current_dir = source.join("nested");
    browser.sync_active_tab_state();
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(workspace.path().join("state.sqlite")).unwrap(),
    );
    drop(browser.enqueue_file_operation(QueuedFileOperation::Move {
        transfers: vec![crate::operation_queue::QueuedTransfer::new(
            source.clone(),
            destination.clone(),
        )],
        verification: file_core::FileOperationVerification::BasicMetadata,
    }));
    let task_id = browser.operation_queue.tasks().last().unwrap().id;

    drop(browser.update(direct_move_renamed_batch_message(vec![(
        source.clone(),
        destination.clone(),
    )])));

    assert_eq!(browser.current_dir, destination.join("nested"));
    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
            transfers: vec![crate::operation_history::CompletedTransfer {
                source: source.clone(),
                target: destination.clone(),
            }],
            history_eligibility:
                crate::operation_history::FileOperationHistoryEligibility::Replayable,
        }),
    ));
    drop(browser.update(direct_move_commit_message(
        task_id,
        source,
        workspace.path().join("stale"),
        2,
    )));
    assert_eq!(browser.current_dir, destination.join("nested"));
}

#[test]
fn moved_renames_batch_migrates_all_paths_with_one_pane_and_search_refresh() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let workspace = tempfile::tempdir().unwrap();
    let first_source = workspace.path().join("first-old");
    let first_target = workspace.path().join("first-new");
    let second_source = workspace.path().join("second-old");
    let second_target = workspace.path().join("second-new");
    std::fs::create_dir_all(first_source.join("nested")).unwrap();
    std::fs::create_dir_all(&second_source).unwrap();
    browser.current_dir = first_source.join("nested");
    browser.renaming = Some(second_source.join("report.txt"));
    browser.sync_active_tab_state();
    drop(browser.update_search_input("report".to_owned()));
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(workspace.path().join("state.sqlite")).unwrap(),
    );
    drop(browser.enqueue_file_operation(QueuedFileOperation::Move {
        transfers: vec![
            crate::operation_queue::QueuedTransfer::new(first_source.clone(), first_target.clone()),
            crate::operation_queue::QueuedTransfer::new(
                second_source.clone(),
                second_target.clone(),
            ),
        ],
        verification: file_core::FileOperationVerification::BasicMetadata,
    }));
    let task_id = browser.operation_queue.tasks().last().unwrap().id;
    let pane_generation = browser.directory_load_generation;
    let search_generation = browser.search_workspace.as_ref().unwrap().run.generation;

    drop(browser.update(direct_move_renamed_batch_message(vec![
        (first_source, first_target.clone()),
        (second_source, second_target.clone()),
    ])));

    assert_eq!(browser.current_dir, first_target.join("nested"));
    assert_eq!(browser.renaming, Some(second_target.join("report.txt")));
    // renamed 走增量应用：路径迁移照常，但不触发全量重扫（generation 不变）。
    // 目录未在 discovery 中显示时跳过立即应用，由 watcher 与对账兜底。
    assert_eq!(browser.directory_load_generation, pane_generation);
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        search_generation + 1
    );

    // journal 落盘只失效 summaries 重算，不再整页 reload，也不重复迁移路径。
    drop(browser.update(direct_move_commit_batch_message(
        task_id,
        vec![(
            0,
            workspace.path().join("stale"),
            workspace.path().join("wrong"),
            2,
        )],
    )));
    assert_eq!(browser.directory_load_generation, pane_generation);
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        search_generation + 1
    );
    assert_eq!(browser.current_dir, first_target.join("nested"));
}

#[tokio::test]
async fn direct_move_batch_rejects_late_scan_when_current_directory_path_is_unchanged() {
    let workspace = tempfile::tempdir().unwrap();
    let current_directory = workspace.path().to_path_buf();
    let source = current_directory.join("old.txt");
    let target = current_directory.join("new.txt");
    std::fs::write(&source, b"late scan entry").unwrap();
    let stale_scan = file_core::scan_directory(&current_directory, Default::default())
        .await
        .unwrap();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = current_directory.clone();
    browser.sync_active_tab_state();
    let stale_request = browser.next_directory_load_request(current_directory.clone());

    drop(browser.migrate_completed_paths(&[CompletedPathMigration::new(source, target)]));
    drop(browser.reload_visible_panes_after_file_operation());

    assert_eq!(browser.current_dir, current_directory);
    assert_eq!(
        browser.directory_load_generation,
        stale_request.generation + 1
    );
    assert!(browser.entries.is_empty());

    drop(browser.accept_complete_directory_fixture(stale_request, stale_scan));
    assert!(browser.entries.is_empty());
}

#[test]
fn direct_move_commit_during_shutdown_does_not_migrate_paths() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/old");
    browser.current_dir = source.join("nested");
    browser.sync_active_tab_state();
    browser.application_shutdown_phase =
        crate::app::application_shutdown::ApplicationShutdownPhase::ClosingWindows;

    drop(browser.update(direct_move_commit_message(
        1,
        source.clone(),
        PathBuf::from("/workspace/new"),
        2,
    )));

    assert_eq!(browser.current_dir, source.join("nested"));
}

#[test]
fn completed_path_migration_updates_inline_rename_and_restarts_frozen_root_search() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("old");
    let destination = workspace.path().join("new");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    browser.current_dir = source.join("nested");
    browser.renaming = Some(source.join("nested/report.txt"));
    browser.sync_active_tab_state();
    drop(browser.update_search_input("report".to_owned()));
    let previous_search_generation = browser.search_workspace.as_ref().unwrap().run.generation;

    finish_queued_rename(&mut browser, source.clone(), destination.clone());

    assert_eq!(
        browser.renaming,
        Some(destination.join("nested/report.txt"))
    );
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.generation > previous_search_generation);
    assert!(
        !workspace.accepts_indexed_outcome(crate::model::IndexedSearchRequest {
            generation: previous_search_generation,
            cursor: None,
        },)
    );
    let active_query = workspace
        .run
        .active_query
        .as_ref()
        .expect("restarted query");
    assert_eq!(
        active_query.scope,
        SearchScope::Directory(source.join("nested"))
    );
}

#[test]
fn completed_non_migration_operation_restarts_active_search_workspace() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let root = tempfile::tempdir().unwrap();
    browser.current_dir = root.path().to_path_buf();
    browser.sync_active_tab_state();
    drop(browser.submit_search());
    let first_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::DeletePermanently {
            paths: vec![root.path().join("obsolete.txt")],
        })
        .error()
        .is_none());
    let task_id = browser.operation_queue.tasks().last().unwrap().id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    ));

    assert!(browser.search_workspace.as_ref().unwrap().run.generation > first_generation);
}

#[test]
fn completed_rename_migrates_another_pane_hidden_tab_history_and_columns() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/old");
    let destination = PathBuf::from("/workspace/new");
    browser.current_dir = PathBuf::from("/workspace/active");
    browser
        .column_return_targets
        .insert(source.join("column"), source.join("column/child"));
    browser.sync_active_tab_state();

    let active_tab_id = 10;
    let hidden_tab_id = 11;
    let mut inactive_pane = browser.capture_active_pane_snapshot();
    inactive_pane.id = BrowserPaneId(1);
    inactive_pane.current_dir = source.join("nested");
    inactive_pane.selected = Some(source.join("nested/file.txt"));
    inactive_pane
        .selected_paths
        .insert(source.join("nested/file.txt"));
    inactive_pane.deepest_open_column_directory = Some(source.join("nested"));
    inactive_pane
        .expanded_directories
        .insert(source.join("nested"), loaded_expanded_directory());
    inactive_pane.column_viewports.insert(
        source.join("nested"),
        ColumnViewport {
            offset_y: 24.0,
            height: 300.0,
        },
    );
    inactive_pane.back_stack = vec![source.join("back"), PathBuf::from("/workspace/old-copy")];
    inactive_pane.forward_stack = vec![source.join("forward")];
    inactive_pane.tabs = vec![
        BrowserTab::directory(active_tab_id, source.join("nested")),
        {
            let mut hidden_tab = BrowserTab::directory(hidden_tab_id, source.join("hidden"));
            hidden_tab
                .expanded_directories
                .insert(source.join("hidden/expanded"), loaded_expanded_directory());
            hidden_tab.back_stack.push(source.join("hidden/back"));
            hidden_tab
        },
    ];
    inactive_pane.active_tab_id = active_tab_id;
    inactive_pane.sync_active_tab_state();
    browser.panes.push(inactive_pane);
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };

    finish_queued_rename(&mut browser, source, destination.clone());

    assert_eq!(browser.current_dir, PathBuf::from("/workspace/active"));
    let migrated_pane = browser.pane_by_id(BrowserPaneId(1)).expect("inactive pane");
    assert_eq!(migrated_pane.current_dir, destination.join("nested"));
    assert_eq!(
        migrated_pane.back_stack,
        vec![
            destination.join("back"),
            PathBuf::from("/workspace/old-copy")
        ]
    );
    assert_eq!(
        migrated_pane.forward_stack,
        vec![destination.join("forward")]
    );
    assert!(migrated_pane
        .expanded_directories
        .contains_key(&destination.join("nested")));
    assert!(migrated_pane
        .column_viewports
        .contains_key(&destination.join("nested")));
    assert!(migrated_pane.directory_load_generation > 0);
    let hidden_tab = migrated_pane
        .tabs
        .iter()
        .find(|tab| tab.id == hidden_tab_id)
        .expect("hidden tab");
    assert_eq!(hidden_tab.directory, destination.join("hidden"));
    assert_eq!(hidden_tab.back_stack, vec![destination.join("hidden/back")]);
    assert!(hidden_tab
        .expanded_directories
        .contains_key(&destination.join("hidden/expanded")));
    assert_eq!(
        browser
            .column_return_targets
            .get(&destination.join("column")),
        Some(&destination.join("column/child"))
    );
}

#[test]
fn undo_and_redo_reuse_success_completion_path_migration() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/old");
    let destination = PathBuf::from("/workspace/new");
    browser.current_dir = source.join("nested");
    browser.sync_active_tab_state();

    finish_queued_rename(&mut browser, source.clone(), destination.clone());
    assert_eq!(browser.current_dir, destination.join("nested"));

    drop(browser.undo_file_operation());
    let undo_task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued undo")
        .id;
    drop(browser.accept_file_operation_finished(
        undo_task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
            from: destination.clone(),
            to: source.clone(),
        }),
    ));
    assert_eq!(browser.current_dir, source.join("nested"));

    drop(browser.redo_file_operation());
    let redo_task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued redo")
        .id;
    drop(browser.accept_file_operation_finished(
        redo_task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
            from: source,
            to: destination.clone(),
        }),
    ));
    assert_eq!(browser.current_dir, destination.join("nested"));
}

#[test]
fn rejected_recoverable_history_replay_returns_item_to_original_stack() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/source");
    let target = PathBuf::from("/workspace/target");
    browser.operation_history.accept_completed(
        1,
        &FileOperationOutcome::Move {
            transfers: vec![crate::operation_history::CompletedTransfer {
                source: source.clone(),
                target: target.clone(),
            }],
            history_eligibility:
                crate::operation_history::FileOperationHistoryEligibility::Replayable,
        },
    );
    let (operation, pending_history) = browser
        .operation_history
        .take_undo_operation()
        .expect("move undo operation");

    drop(browser.enqueue_file_operation_with_history(operation, Some(pending_history)));

    let (restored_operation, _) = browser
        .operation_history
        .take_undo_operation()
        .expect("rejected move returned to undo stack");
    let QueuedFileOperation::Move {
        transfers,
        verification,
    } = restored_operation
    else {
        panic!("expected restored move undo");
    };
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].source, target);
    assert_eq!(transfers[0].target, source);
    assert_eq!(
        verification,
        file_core::FileOperationVerification::default()
    );
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn failed_move_migrates_paths_for_completed_transfers_only() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/old");
    let destination = PathBuf::from("/archive/old");
    browser.current_dir = source.join("nested");
    browser.sync_active_tab_state();
    let operation_directory = tempfile::tempdir().unwrap();
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(operation_directory.path().join("state.sqlite"))
            .unwrap(),
    );

    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Move {
            transfers: vec![crate::operation_queue::QueuedTransfer::new(
                source.clone(),
                destination.clone(),
            )],
            verification: file_core::FileOperationVerification::default(),
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued move")
        .id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::failed_after_completed_moves(
            "second transfer failed".to_owned(),
            vec![crate::operation_history::CompletedTransfer {
                source,
                target: destination.clone(),
            }],
        ),
    ));

    assert_eq!(browser.current_dir, destination.join("nested"));
    assert_eq!(
        browser
            .operation_queue
            .tasks()
            .last()
            .and_then(|task| task.error.as_deref()),
        Some("second transfer failed")
    );
}

#[test]
fn successful_trash_focuses_next_entry_after_keyboard_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let first = PathBuf::from("/workspace/first.txt");
    let deleted = PathBuf::from("/workspace/deleted.txt");
    let next = PathBuf::from("/workspace/next.txt");
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = crate::model::BrowserViewMode::Columns;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.entries = vec![
        file_core::DirectoryEntry::new(
            first.clone(),
            file_core::FileKind::File,
            file_core::EntryMetadata::default(),
            false,
            false,
            false,
        ),
        file_core::DirectoryEntry::new(
            deleted.clone(),
            file_core::FileKind::File,
            file_core::EntryMetadata::default(),
            false,
            false,
            false,
        ),
        file_core::DirectoryEntry::new(
            next.clone(),
            file_core::FileKind::File,
            file_core::EntryMetadata::default(),
            false,
            false,
            false,
        ),
    ]
    .into();

    browser.select_path(first);
    drop(browser.move_file_selection(FileSelectionDirection::Down));
    assert_eq!(browser.selected, Some(deleted.clone()));

    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Trash {
            paths: vec![deleted.clone()],
        })
        .error()
        .is_none());
    let task_id = browser.operation_queue.tasks().last().unwrap().id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Trash {
            paths: vec![deleted],
            entries: Vec::new(),
            tracking_warnings: Vec::new(),
        }),
    ));

    assert_eq!(browser.selected, Some(next.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([next]));
}

#[test]
fn finished_delete_operation_only_invalidates_affected_directory_chain() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let root = PathBuf::from("/workspace");
    let current_dir = root.join("project");
    let deleted_child = current_dir.join("todo.txt");
    let unrelated = root.join("archive");

    browser.current_dir = current_dir.clone();
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;

    remember_summary(&mut browser, &root, 3, 4096);
    remember_summary(&mut browser, &current_dir, 2, 2048);
    remember_summary(&mut browser, &unrelated, 1, 512);

    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::DeletePermanently {
            paths: vec![deleted_child],
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued task")
        .id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    ));

    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&current_dir)
        .is_none());
    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&root)
        .is_none());
    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&unrelated)
        .is_some());
}

#[test]
fn finished_directory_delete_operation_clears_cached_descendant_summaries() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let root = PathBuf::from("/workspace");
    let current_dir = root.join("project");
    let deleted_directory = current_dir.join("src");
    let deleted_descendant = deleted_directory.join("nested");
    let unrelated = root.join("archive");

    browser.current_dir = current_dir.clone();
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;

    remember_summary(&mut browser, &root, 3, 4096);
    remember_summary(&mut browser, &current_dir, 2, 2048);
    remember_summary(&mut browser, &deleted_directory, 4, 1536);
    remember_summary(&mut browser, &deleted_descendant, 1, 256);
    remember_summary(&mut browser, &unrelated, 1, 512);

    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::DeletePermanently {
            paths: vec![deleted_directory],
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued task")
        .id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    ));

    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&current_dir)
        .is_none());
    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&root)
        .is_none());
    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&deleted_descendant)
        .is_none());
    assert!(browser
        .list_directory_summary_cache
        .summary_for_path(&unrelated)
        .is_some());
}
