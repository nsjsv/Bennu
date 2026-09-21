use std::path::{Path, PathBuf};

use rusqlite::params;
use tempfile::tempdir;

use super::*;

fn recoverable_copy(transfers: Vec<StoredTransfer>) -> StoredOperation {
    StoredOperation::Copy {
        transfers,
        verification: StoredFileOperationVerification::Strong,
        recovery_version: Some(TRANSFER_JOURNAL_VERSION),
    }
}

fn transfer(source: &Path, target: &Path) -> StoredTransfer {
    StoredTransfer {
        source: StoredPath::from_path(source),
        target: StoredPath::from_path(target),
        conflict_strategy: StoredTransferConflictStrategy::KeepBoth,
    }
}

fn identity(size: u64) -> StoredFileIdentity {
    StoredFileIdentity {
        device: 10,
        inode: size + 20,
        object_kind: StoredFileObjectKind::RegularFile,
        size,
        modified_seconds: 100,
        modified_nanoseconds: 200,
        changed_seconds: 300,
        changed_nanoseconds: 400,
        symbolic_link_target: None,
    }
}

fn stored_checkpoint(kind: StoredTransferCheckpointKind) -> StoredTransferCheckpoint {
    StoredTransferCheckpoint::new(
        kind,
        format!(r#"{{"state":"{}"}}"#, kind.serde_state_name()),
    )
    .unwrap()
}

fn completed_checkpoint() -> StoredTransferCheckpoint {
    stored_checkpoint(StoredTransferCheckpointKind::Completed)
}

fn persist_initial_checkpoint(
    store: &TaskQueueStore,
    task_id: u64,
    checkpoint: &StoredTransferCheckpoint,
) {
    store
        .compare_and_swap_transfer_checkpoint(
            task_id,
            &StoredTransferWorkKey::top_level(0),
            0,
            checkpoint,
        )
        .unwrap();
}

#[test]
fn connection_enables_durable_journal_settings() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let connection = store.connection().unwrap();

    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    let foreign_keys: i64 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();

    assert_eq!(journal_mode, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(foreign_keys, 1);
}

#[test]
fn recoverable_task_runner_lease_is_exclusive_until_owner_drops() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&operation)
        .unwrap();
    let second_store = TaskQueueStore::new(database_path).unwrap();

    assert!(second_store
        .try_acquire_recoverable_task_runner(claimed.task_id)
        .unwrap()
        .is_none());
    let task_id = claimed.task_id;
    drop(claimed);
    assert!(second_store
        .try_acquire_recoverable_task_runner(task_id)
        .unwrap()
        .is_some());
}

#[test]
fn restore_coordinator_lease_prevents_split_task_claims() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let first_store = TaskQueueStore::new(&database_path).unwrap();
    let second_store = TaskQueueStore::new(database_path).unwrap();
    let coordinator = first_store
        .try_acquire_recoverable_restore_coordinator()
        .unwrap()
        .unwrap();

    assert!(second_store
        .try_acquire_recoverable_restore_coordinator()
        .unwrap()
        .is_none());
    drop(coordinator);
    assert!(second_store
        .try_acquire_recoverable_restore_coordinator()
        .unwrap()
        .is_some());
}

#[test]
fn startup_failure_marking_preserves_recoverable_task_state() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    store
        .update_status(task_id, StoredTaskStatus::Paused)
        .unwrap();

    store
        .mark_unfinished_tasks_failed("previous process stopped")
        .unwrap();

    assert_eq!(
        store.read_task(task_id).unwrap().unwrap().status,
        StoredTaskStatus::Paused
    );
}

#[test]
fn task_and_initial_transfer_journal_commit_atomically() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let connection = store.connection().unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_transfer_journal
             BEFORE INSERT ON transfer_journal
             BEGIN
                 SELECT RAISE(ABORT, 'injected journal failure');
             END;",
        )
        .unwrap();
    drop(connection);

    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    assert!(store.insert_recoverable_transfer_task(&operation).is_err());

    let connection = store.connection().unwrap();
    let task_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM task_queue", [], |row| row.get(0))
        .unwrap();
    let journal_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM transfer_journal", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(task_count, 0);
    assert_eq!(journal_count, 0);
}

#[test]
fn initial_journal_preserves_transfer_semantics() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![
        transfer(Path::new("/tmp/a"), Path::new("/target/a")),
        StoredTransfer {
            source: StoredPath::from_path(Path::new("/tmp/b")),
            target: StoredPath::from_path(Path::new("/target/b")),
            conflict_strategy: StoredTransferConflictStrategy::Replace,
        },
    ]);

    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    let snapshot = store.read_transfer_recovery(task_id).unwrap();

    assert_eq!(snapshot.journal_entries.len(), 2);
    assert_eq!(snapshot.journal_entries[0].key.transfer_index, 0);
    assert_eq!(
        snapshot.journal_entries[0].conflict_strategy,
        StoredTransferConflictStrategy::KeepBoth
    );
    assert_eq!(
        snapshot.journal_entries[1].conflict_strategy,
        StoredTransferConflictStrategy::Replace
    );
    assert_eq!(
        snapshot.journal_entries[1].verification,
        StoredFileOperationVerification::Strong
    );
    assert_eq!(
        snapshot.journal_entries[1].checkpoint,
        StoredTransferCheckpoint::awaiting_manifest()
    );
    assert_eq!(snapshot.journal_entries[1].revision, 0);
}

#[test]
fn manifest_and_checkpoint_update_are_atomic() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let duplicate_path = StoredPath::from_path(Path::new("file.txt"));
    let manifest = vec![
        StoredManifestEntry {
            transfer_index: 0,
            relative_path: duplicate_path.clone(),
            identity: identity(1),
        },
        StoredManifestEntry {
            transfer_index: 0,
            relative_path: duplicate_path,
            identity: identity(2),
        },
    ];
    let checkpoint = stored_checkpoint(StoredTransferCheckpointKind::StageCreationIntent);

    assert!(store
        .install_transfer_manifest_and_checkpoint(task_id, &key, 0, &manifest, &checkpoint)
        .is_err());

    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert!(snapshot.manifest_entries.is_empty());
    assert_eq!(snapshot.journal_entries[0].revision, 0);
    assert_eq!(
        snapshot.journal_entries[0].checkpoint,
        StoredTransferCheckpoint::awaiting_manifest()
    );
}

#[test]
fn interrupted_manifest_transaction_rolls_back_checkpoint_and_entries() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let task_id = store
        .insert_recoverable_transfer_task(&recoverable_copy(vec![transfer(
            Path::new("/tmp/source"),
            Path::new("/tmp/target"),
        )]))
        .unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let manifest = (0..100)
        .map(|index| StoredManifestEntry {
            transfer_index: 0,
            relative_path: StoredPath::from_path(Path::new(&format!("file-{index}"))),
            identity: identity(index),
        })
        .collect::<Vec<_>>();
    let checkpoint = stored_checkpoint(StoredTransferCheckpointKind::StageCreationIntent);
    let mut checks = 0;

    let revision = store
        .install_transfer_manifests_and_checkpoint_while(
            task_id,
            TransferManifestCheckpointUpdate {
                key: &key,
                expected_revision: 0,
                manifest_entries: &manifest,
                replacement_manifest_entries: &[],
                checkpoint: &checkpoint,
            },
            || {
                checks += 1;
                checks < 8
            },
        )
        .unwrap();

    assert_eq!(revision, None);
    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert!(snapshot.manifest_entries.is_empty());
    assert_eq!(snapshot.journal_entries[0].revision, 0);
    assert_eq!(
        snapshot.journal_entries[0].checkpoint,
        StoredTransferCheckpoint::awaiting_manifest()
    );
}

#[test]
fn replacement_manifest_and_checkpoint_commit_atomically() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let source = PathBuf::from("/tmp/source");
    let target = PathBuf::from("/tmp/target");
    let task_id = store
        .insert_recoverable_transfer_task(&recoverable_copy(vec![transfer(&source, &target)]))
        .unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let manifest = vec![StoredManifestEntry {
        transfer_index: 0,
        relative_path: StoredPath::from_path(Path::new("")),
        identity: identity(11),
    }];
    let replacement_manifest = vec![StoredManifestEntry {
        transfer_index: 0,
        relative_path: StoredPath::from_path(Path::new("")),
        identity: identity(22),
    }];
    let checkpoint = StoredTransferCheckpoint::awaiting_manifest();
    let connection = store.connection().unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_replacement_manifest
             BEFORE INSERT ON transfer_replacement_manifest
             BEGIN SELECT RAISE(ABORT, 'injected replacement failure'); END;",
        )
        .unwrap();

    assert!(store
        .install_transfer_manifests_and_checkpoint(
            task_id,
            &key,
            0,
            &manifest,
            &replacement_manifest,
            &checkpoint,
        )
        .is_err());
    let unchanged = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(unchanged.journal_entries[0].revision, 0);
    assert!(unchanged.manifest_entries.is_empty());
    assert!(unchanged.replacement_manifest_entries.is_empty());

    connection
        .execute_batch("DROP TRIGGER reject_replacement_manifest")
        .unwrap();
    store
        .install_transfer_manifests_and_checkpoint(
            task_id,
            &key,
            0,
            &manifest,
            &replacement_manifest,
            &checkpoint,
        )
        .unwrap();
    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(snapshot.manifest_entries, manifest);
    assert_eq!(snapshot.replacement_manifest_entries, replacement_manifest);
}

#[test]
fn checkpoint_compare_and_swap_rejects_stale_revision() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let staging = stored_checkpoint(StoredTransferCheckpointKind::Staging);

    assert_eq!(
        store
            .compare_and_swap_transfer_checkpoint(task_id, &key, 0, &staging)
            .unwrap(),
        1
    );
    let error = store
        .compare_and_swap_transfer_checkpoint(task_id, &key, 0, &staging)
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::StaleTransferRevision {
            task_id: stale_task_id,
            transfer_index: 0,
            expected_revision: 0,
        } if stale_task_id == task_id
    ));

    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(snapshot.journal_entries[0].revision, 1);
    assert_eq!(snapshot.journal_entries[0].checkpoint, staging);
}

#[test]
fn merge_completion_and_cursor_commit_atomically_and_reload_identity_fact() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let source = Path::new("/tmp/source");
    let target = Path::new("/tmp/target");
    let task_id = store
        .insert_recoverable_transfer_task(&recoverable_copy(vec![transfer(source, target)]))
        .unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let manifest = vec![StoredManifestEntry {
        transfer_index: 0,
        relative_path: StoredPath::from_path(Path::new("")),
        identity: identity(10),
    }];
    let initial_merge = StoredTransferCheckpoint::new(
        StoredTransferCheckpointKind::Merging,
        r#"{"state":"merging","fields":{"cursor":0}}"#.to_owned(),
    )
    .unwrap();
    assert_eq!(
        store
            .install_transfer_manifest_and_checkpoint(task_id, &key, 0, &manifest, &initial_merge,)
            .unwrap(),
        1
    );

    let completion = StoredMergeChildCompletion {
        transfer_index: 0,
        child_relative_path: StoredPath::from_path(Path::new("child")),
        completion_json: r#"{"target_identity":{"device":1,"inode":2}}"#.to_owned(),
    };
    let completed_merge = StoredTransferCheckpoint::new(
        StoredTransferCheckpointKind::Merging,
        r#"{"state":"merging","fields":{"cursor":1}}"#.to_owned(),
    )
    .unwrap();
    let connection = store.connection().unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_merge_completion
             BEFORE INSERT ON transfer_merge_completion
             BEGIN SELECT RAISE(ABORT, 'injected merge completion failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .compare_and_swap_transfer_merge_completion(
                task_id,
                &key,
                1,
                &completion,
                &completed_merge,
            )
            .is_err()
    );
    connection
        .execute_batch("DROP TRIGGER reject_merge_completion;")
        .unwrap();
    let unchanged = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(unchanged.journal_entries[0].revision, 1);
    assert!(unchanged.merge_completions.is_empty());

    assert_eq!(
        store
            .compare_and_swap_transfer_merge_completion(
                task_id,
                &key,
                1,
                &completion,
                &completed_merge,
            )
            .unwrap(),
        2
    );
    assert!(matches!(
        store.compare_and_swap_transfer_merge_completion(
            task_id,
            &key,
            1,
            &completion,
            &completed_merge,
        ),
        Err(StoreError::StaleTransferRevision { .. })
    ));

    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(snapshot.merge_completions, vec![completion]);
    assert_eq!(snapshot.journal_entries[0].checkpoint, completed_merge);

    store.delete_transfer_recovery(task_id).unwrap();
    assert!(store
        .read_transfer_recovery(task_id)
        .unwrap()
        .merge_completions
        .is_empty());
}

#[test]
fn manifest_roundtrips_and_is_removed_with_recovery_details() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let entries = vec![StoredManifestEntry {
        transfer_index: 0,
        relative_path: StoredPath::from_path(Path::new("nested/file.txt")),
        identity: identity(42),
    }];
    let checkpoint = stored_checkpoint(StoredTransferCheckpointKind::StageCreationIntent);

    store
        .install_transfer_manifest_and_checkpoint(task_id, &key, 0, &entries, &checkpoint)
        .unwrap();
    assert_eq!(
        store
            .read_transfer_recovery(task_id)
            .unwrap()
            .manifest_entries,
        entries
    );

    store.delete_transfer_recovery(task_id).unwrap();
    assert_eq!(
        store.read_transfer_recovery(task_id).unwrap(),
        StoredTransferRecoverySnapshot {
            journal_entries: Vec::new(),
            manifest_entries: Vec::new(),
            replacement_manifest_entries: Vec::new(),
            merge_completions: Vec::new(),
        }
    );
    assert!(store.read_task(task_id).unwrap().is_some());
}

#[test]
fn terminal_task_state_and_recovery_cleanup_commit_atomically() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();

    assert!(store
        .finalize_recoverable_transfer_task(
            task_id,
            StoredTaskStatus::Completed,
            StoredProgress::with_fraction(1.0),
            None,
        )
        .is_err());
    assert_eq!(
        store.read_task(task_id).unwrap().unwrap().status,
        StoredTaskStatus::Pending
    );

    persist_initial_checkpoint(&store, task_id, &completed_checkpoint());
    store
        .finalize_recoverable_transfer_task(
            task_id,
            StoredTaskStatus::Completed,
            StoredProgress::with_fraction(1.0),
            None,
        )
        .unwrap();

    let task = store.read_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, StoredTaskStatus::Completed);
    assert_eq!(task.progress, StoredProgress::with_fraction(1.0));
    assert!(store
        .read_transfer_recovery(task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[test]
fn failed_checkpoint_can_finalize_task_without_losing_error() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    persist_initial_checkpoint(
        &store,
        task_id,
        &stored_checkpoint(StoredTransferCheckpointKind::Failed),
    );

    store
        .finalize_recoverable_transfer_task(
            task_id,
            StoredTaskStatus::Failed,
            StoredProgress::pending(),
            Some("source changed"),
        )
        .unwrap();

    let task = store.read_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, StoredTaskStatus::Failed);
    assert_eq!(task.error.as_deref(), Some("source changed"));
    assert!(store
        .read_transfer_recovery(task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[test]
fn terminal_finalize_rolls_back_task_state_when_cleanup_fails() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    persist_initial_checkpoint(&store, task_id, &completed_checkpoint());
    let connection = store.connection().unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_recovery_cleanup
             BEFORE DELETE ON transfer_journal
             BEGIN
                 SELECT RAISE(ABORT, 'injected recovery cleanup failure');
             END;",
        )
        .unwrap();
    drop(connection);

    assert!(store
        .finalize_recoverable_transfer_task(
            task_id,
            StoredTaskStatus::Completed,
            StoredProgress::with_fraction(1.0),
            None,
        )
        .is_err());

    assert_eq!(
        store.read_task(task_id).unwrap().unwrap().status,
        StoredTaskStatus::Pending
    );
    assert_eq!(
        store
            .read_transfer_recovery(task_id)
            .unwrap()
            .journal_entries
            .len(),
        1
    );
}

#[test]
fn legacy_transfer_payload_defaults_to_nonrecoverable_semantics() {
    let decoded: StoredOperation = serde_json::from_str(
        r#"{"kind":"copy","transfers":[{"source":{"encoding":"utf8","value":"a"},"target":{"encoding":"utf8","value":"b"}}]}"#,
    )
    .unwrap();

    let StoredOperation::Copy {
        transfers,
        verification,
        recovery_version,
    } = decoded
    else {
        panic!("expected copy operation");
    };
    assert_eq!(
        transfers[0].conflict_strategy,
        StoredTransferConflictStrategy::Fail
    );
    assert_eq!(verification, StoredFileOperationVerification::BasicMetadata);
    assert_eq!(recovery_version, None);
}

#[cfg(unix)]
#[test]
fn recovery_paths_preserve_non_utf8_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let source = PathBuf::from(std::ffi::OsString::from_vec(vec![b's', 0xff]));
    let target = PathBuf::from(std::ffi::OsString::from_vec(vec![b't', 0xfe]));
    let relative = PathBuf::from(std::ffi::OsString::from_vec(vec![b'r', 0xfd]));
    let operation = recoverable_copy(vec![transfer(&source, &target)]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();
    let key = StoredTransferWorkKey::top_level(0);
    let manifest = vec![StoredManifestEntry {
        transfer_index: 0,
        relative_path: StoredPath::from_path(&relative),
        identity: identity(1),
    }];
    store
        .install_transfer_manifest_and_checkpoint(
            task_id,
            &key,
            0,
            &manifest,
            &stored_checkpoint(StoredTransferCheckpointKind::StageCreationIntent),
        )
        .unwrap();

    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert_eq!(snapshot.journal_entries[0].source.to_path_buf(), source);
    assert_eq!(
        snapshot.journal_entries[0].requested_target.to_path_buf(),
        target
    );
    assert_eq!(
        snapshot.manifest_entries[0].relative_path.to_path_buf(),
        relative
    );
}

#[test]
fn deleting_task_cascades_recovery_rows() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![transfer(
        Path::new("/tmp/source"),
        Path::new("/tmp/target"),
    )]);
    let task_id = store.insert_recoverable_transfer_task(&operation).unwrap();

    store.delete_task(task_id).unwrap();

    let connection = store.connection().unwrap();
    let journal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM transfer_journal WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(journal_count, 0);
}

#[test]
fn batch_checkpoint_swap_updates_every_record_in_one_transaction() {
    let directory = tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let operation = recoverable_copy(vec![
        transfer(Path::new("/tmp/source-0"), Path::new("/tmp/target-0")),
        transfer(Path::new("/tmp/source-1"), Path::new("/tmp/target-1")),
    ]);
    let claimed = store
        .insert_claimed_recoverable_transfer_task(&operation)
        .unwrap();
    let task_id = claimed.task_id;
    drop(claimed);

    let swap = |transfer_index: u64| StoredTransferCheckpointSwap {
        task_id,
        key: StoredTransferWorkKey::top_level(transfer_index),
        expected_revision: 0,
        checkpoint: completed_checkpoint(),
    };
    let revisions = store
        .compare_and_swap_transfer_checkpoints(&[swap(0), swap(1)])
        .unwrap();
    assert_eq!(revisions, vec![1, 1]);

    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert!(snapshot.journal_entries.iter().all(|entry| {
        entry.revision == 1 && entry.checkpoint.kind == StoredTransferCheckpointKind::Completed
    }));

    // A stale revision in one swap must fail the whole batch atomically.
    assert!(store
        .compare_and_swap_transfer_checkpoints(&[swap(0), swap(1)])
        .is_err());
    let snapshot = store.read_transfer_recovery(task_id).unwrap();
    assert!(snapshot
        .journal_entries
        .iter()
        .all(|entry| entry.revision == 1));
}
