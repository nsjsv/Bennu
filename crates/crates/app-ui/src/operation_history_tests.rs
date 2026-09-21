use std::ffi::OsString;
use std::sync::OnceLock;

use tokio::sync::Mutex;

use super::*;

static TRASH_ENVIRONMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct ScopedEnvironmentVariable {
    name: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvironmentVariable {
    fn set(name: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, previous }
    }
}

impl Drop for ScopedEnvironmentVariable {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.name, previous);
        } else {
            std::env::remove_var(self.name);
        }
    }
}

fn transfer(source: &str, target: &str) -> CompletedTransfer {
    CompletedTransfer {
        source: PathBuf::from(source),
        target: PathBuf::from(target),
    }
}

#[test]
fn completed_path_migrations_are_component_aware_longest_and_single_pass() {
    let outcome = FileOperationOutcome::BatchRename {
        renames: vec![
            CompletedBatchRename {
                from: PathBuf::from("/workspace/old"),
                to: PathBuf::from("/workspace/new"),
            },
            CompletedBatchRename {
                from: PathBuf::from("/workspace/new"),
                to: PathBuf::from("/workspace/final"),
            },
            CompletedBatchRename {
                from: PathBuf::from("/workspace/old/nested"),
                to: PathBuf::from("/workspace/special"),
            },
        ],
    };
    let migrations = outcome.completed_path_migrations();

    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/old"), &migrations),
        PathBuf::from("/workspace/new")
    );
    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/old/file.txt"), &migrations),
        PathBuf::from("/workspace/new/file.txt")
    );
    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/old-copy"), &migrations),
        PathBuf::from("/workspace/old-copy")
    );
    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/old/nested/file.txt"), &migrations,),
        PathBuf::from("/workspace/special/file.txt")
    );
    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/new/file.txt"), &migrations),
        PathBuf::from("/workspace/final/file.txt")
    );
}

#[test]
fn non_replayable_move_still_reports_completed_path_migration() {
    let outcome = FileOperationOutcome::Move {
        transfers: vec![transfer("/workspace/old", "/archive/old")],
        history_eligibility: FileOperationHistoryEligibility::NotReplayable,
    };
    let migrations = outcome.completed_path_migrations();
    let mut history = FileOperationHistory::new();

    history.accept_completed(1, &outcome);

    assert_eq!(
        path_after_completed_migrations(Path::new("/workspace/old/child"), &migrations),
        PathBuf::from("/archive/old/child")
    );
    assert!(history.take_undo_operation().is_none());
}

#[test]
fn undo_move_reverses_completed_transfers() {
    let item = FileOperationHistoryItem::Move {
        transfers: vec![transfer("/a/one", "/b/one"), transfer("/a/two", "/b/two")],
    };

    let operation = item.undo_operation().expect("undo operation");

    let QueuedFileOperation::Move { transfers, .. } = operation else {
        panic!("expected move");
    };
    assert_eq!(transfers[0].source, PathBuf::from("/b/two"));
    assert_eq!(transfers[0].target, PathBuf::from("/a/two"));
    assert_eq!(transfers[1].source, PathBuf::from("/b/one"));
    assert_eq!(transfers[1].target, PathBuf::from("/a/one"));
}

#[test]
fn pending_history_task_id_remaps_after_durable_insert() {
    let mut history = FileOperationHistory::new();
    history.undo_stack.push(FileOperationHistoryItem::Move {
        transfers: vec![transfer("/a", "/b")],
    });
    let (_operation, pending) = history.take_undo_operation().expect("undo operation");
    history.track_pending(u64::MAX - 1, pending);

    history.remap_pending_task(u64::MAX - 1, 42);

    assert!(!history.is_replaying(u64::MAX - 1));
    assert!(history.is_replaying(42));
}

#[test]
fn failed_undo_move_splits_completed_and_remaining_transfers() {
    let mut history = FileOperationHistory::new();
    history.undo_stack.push(FileOperationHistoryItem::Move {
        transfers: vec![transfer("/a/one", "/b/one"), transfer("/a/two", "/b/two")],
    });
    let (_operation, pending) = history.take_undo_operation().expect("undo move");
    history.track_pending(1, pending);

    history.accept_failed(1, &[transfer("/b/two", "/a/two")]);

    let (remaining_undo, _) = history
        .take_undo_operation()
        .expect("remaining undo transfer");
    let QueuedFileOperation::Move {
        transfers: remaining_undo_transfers,
        ..
    } = remaining_undo
    else {
        panic!("expected remaining undo move");
    };
    assert_eq!(remaining_undo_transfers.len(), 1);
    assert_eq!(remaining_undo_transfers[0].source, PathBuf::from("/b/one"));
    assert_eq!(remaining_undo_transfers[0].target, PathBuf::from("/a/one"));

    let (completed_redo, _) = history
        .take_redo_operation()
        .expect("completed transfer redo");
    let QueuedFileOperation::Move {
        transfers: completed_redo_transfers,
        ..
    } = completed_redo
    else {
        panic!("expected completed redo move");
    };
    assert_eq!(completed_redo_transfers.len(), 1);
    assert_eq!(completed_redo_transfers[0].source, PathBuf::from("/a/two"));
    assert_eq!(completed_redo_transfers[0].target, PathBuf::from("/b/two"));
}

#[test]
fn failed_redo_move_splits_completed_and_remaining_transfers() {
    let mut history = FileOperationHistory::new();
    history.redo_stack.push(FileOperationHistoryItem::Move {
        transfers: vec![transfer("/a/one", "/b/one"), transfer("/a/two", "/b/two")],
    });
    let (_operation, pending) = history.take_redo_operation().expect("redo move");
    history.track_pending(1, pending);

    history.accept_failed(1, &[transfer("/a/one", "/b/one")]);

    let (remaining_redo, _) = history
        .take_redo_operation()
        .expect("remaining redo transfer");
    let QueuedFileOperation::Move {
        transfers: remaining_redo_transfers,
        ..
    } = remaining_redo
    else {
        panic!("expected remaining redo move");
    };
    assert_eq!(remaining_redo_transfers.len(), 1);
    assert_eq!(remaining_redo_transfers[0].source, PathBuf::from("/a/two"));
    assert_eq!(remaining_redo_transfers[0].target, PathBuf::from("/b/two"));

    let (completed_undo, _) = history
        .take_undo_operation()
        .expect("completed transfer undo");
    let QueuedFileOperation::Move {
        transfers: completed_undo_transfers,
        ..
    } = completed_undo
    else {
        panic!("expected completed undo move");
    };
    assert_eq!(completed_undo_transfers.len(), 1);
    assert_eq!(completed_undo_transfers[0].source, PathBuf::from("/b/one"));
    assert_eq!(completed_undo_transfers[0].target, PathBuf::from("/a/one"));
}

#[test]
fn trash_tracking_warning_does_not_create_undo_from_historical_paths() {
    let tracked_path = PathBuf::from("/workspace/tracked");
    let untracked_path = PathBuf::from("/workspace/untracked");
    let mut history = FileOperationHistory::new();
    let outcome = FileOperationOutcome::Trash {
        paths: vec![tracked_path.clone()],
        entries: vec![TrashRestoreEntry::from_historical_paths(
            PathBuf::from("/trash/files/tracked"),
            PathBuf::from("/trash/info/tracked.trashinfo"),
            tracked_path,
        )],
        tracking_warnings: vec![file_core::TrashTrackingWarning {
            path: untracked_path,
            message: "post-commit scan failed".to_owned(),
        }],
    };

    history.accept_completed(7, &outcome);

    assert!(history.take_undo_operation().is_none());
    assert!(outcome.completion_warning().is_some());
}

#[test]
fn trash_tracking_warning_bounds_rendered_details() {
    let tracking_warnings = (0..8)
        .map(|index| file_core::TrashTrackingWarning {
            path: PathBuf::from(format!("/workspace/item-{index}")),
            message: "post-commit scan failed".to_owned(),
        })
        .collect();
    let warning = FileOperationOutcome::Trash {
        paths: Vec::new(),
        entries: Vec::new(),
        tracking_warnings,
    }
    .completion_warning()
    .expect("tracking warning");

    assert!(warning.contains("/workspace/item-4"));
    assert!(!warning.contains("/workspace/item-5"));
    assert!(warning.ends_with("... (+3)"));
}

#[tokio::test(flavor = "current_thread")]
async fn verified_trash_outcome_with_untracked_peer_creates_only_the_executable_undo_entry() {
    let _environment_guard = TRASH_ENVIRONMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .await;
    let fixture = tempfile::tempdir().unwrap();
    let _xdg_data_home =
        ScopedEnvironmentVariable::set("XDG_DATA_HOME", &fixture.path().join("data"));
    let source = fixture.path().join("tracked.txt");
    std::fs::write(&source, b"payload").unwrap();
    let entry = match file_core::trash_path_with_restore_entry(&source)
        .await
        .unwrap()
    {
        file_core::TrashCommitOutcome::Tracked(entry) => *entry,
        file_core::TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("Trash entry was not tracked: {}", warning.message)
        }
    };
    let mut history = FileOperationHistory::new();
    history.accept_completed(
        7,
        &FileOperationOutcome::Trash {
            paths: vec![entry.original_path.clone()],
            entries: vec![entry.clone()],
            tracking_warnings: vec![file_core::TrashTrackingWarning {
                path: fixture.path().join("untracked.txt"),
                message: "post-commit scan failed".to_owned(),
            }],
        },
    );

    let (undo, _) = history.take_undo_operation().expect("verified undo entry");
    let QueuedFileOperation::Restore { mut entries } = undo else {
        panic!("expected restore operation");
    };
    assert_eq!(entries.len(), 1);
    assert!(entries[0].has_verified_identity());
    file_core::delete_trash_entry(entries.remove(0))
        .await
        .unwrap();
}

#[test]
fn normal_operation_clears_redo_stack() {
    let mut history = FileOperationHistory::new();
    history
        .undo_stack
        .push(FileOperationHistoryItem::CreateEmptyFile {
            path: PathBuf::from("/tmp/New File"),
        });
    let (_operation, pending) = history.take_undo_operation().expect("undo");
    history.track_pending(1, pending);
    history.accept_completed(
        1,
        &FileOperationOutcome::Trash {
            paths: vec![PathBuf::from("/tmp/New File")],
            entries: Vec::new(),
            tracking_warnings: Vec::new(),
        },
    );
    assert_eq!(history.redo_stack.len(), 1);

    history.accept_completed(
        2,
        &FileOperationOutcome::CreateDirectory {
            path: PathBuf::from("/tmp/New Folder"),
        },
    );

    assert!(history.redo_stack.is_empty());
    assert_eq!(history.undo_stack.len(), 1);
}
