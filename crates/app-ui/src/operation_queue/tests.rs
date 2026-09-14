use super::*;
use file_core::TransferCheckpoint;
use file_operation_store::StoredProgress;
use std::time::{Duration, Instant};

fn sample_operation() -> QueuedFileOperation {
    QueuedFileOperation::CreateDirectory {
        parent: PathBuf::from("/tmp"),
    }
}

fn sample_transfer_operation() -> QueuedFileOperation {
    QueuedFileOperation::Copy {
        transfers: vec![QueuedTransfer::new(
            PathBuf::from("/tmp/source"),
            PathBuf::from("/tmp/target"),
        )],
        verification: FileOperationVerification::BasicMetadata,
    }
}

#[test]
fn enqueue_accepts_before_a_contended_store_writer_releases() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    let (writer_ready, wait_for_writer) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let mut connection = rusqlite::Connection::open(database_path).unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        writer_ready.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        drop(transaction);
    });
    wait_for_writer.recv().unwrap();

    let started_at = Instant::now();
    let outcome = queue.enqueue(sample_transfer_operation());
    let acceptance_latency = started_at.elapsed();
    writer.join().unwrap();
    eprintln!("contended enqueue acceptance latency: {acceptance_latency:?}");

    assert!(outcome.error().is_none());
    assert!(
        acceptance_latency < Duration::from_millis(50),
        "enqueue blocked the caller for {acceptance_latency:?}"
    );
}

#[test]
fn recoverable_task_waits_for_journal_ack_before_starting() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);

    let outcome = queue.enqueue(sample_transfer_operation());
    let local_task_id = match outcome {
        FileOperationEnqueueOutcome::Queued { task_id } => task_id,
        _ => panic!("unexpected enqueue outcome"),
    };
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Pending);
    assert!(queue.active_subscription().is_none());

    let request = queue
        .take_next_persistence_request()
        .expect("insert must be the first persistence request");
    let persistence_outcome = execute_file_operation_persistence(request);
    let acceptance = queue.accept_persistence_outcome(persistence_outcome);

    assert!(acceptance.error.is_none());
    let stored_task_id = queue.tasks()[0]
        .stored_id
        .expect("insert assigned stored id");
    assert_eq!(
        acceptance.task_id_remap,
        Some((local_task_id, stored_task_id))
    );
    assert_eq!(queue.tasks()[0].id, local_task_id);
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Running);
    assert_eq!(queue.tasks()[0].status_label(), "Preparing");
    assert!(queue.active_subscription().is_some());

    queue.update_progress(
        queue.tasks()[0].id,
        FileOperationProgressUpdate::Indeterminate,
        Vec::new(),
    );
    assert_eq!(queue.tasks()[0].status_label(), "Running");
}

#[test]
fn direct_move_commit_revisions_are_accepted_once_for_the_active_work_item() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    let first_source = PathBuf::from("/tmp/first-source");
    let second_source = PathBuf::from("/tmp/second-source");
    queue.enqueue(QueuedFileOperation::Move {
        transfers: vec![
            QueuedTransfer::new(first_source.clone(), PathBuf::from("/tmp/first-target")),
            QueuedTransfer::new(second_source.clone(), PathBuf::from("/tmp/second-target")),
        ],
        verification: FileOperationVerification::BasicMetadata,
    });
    let request = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(request));
    let task_id = queue.tasks()[0].id;
    let first_key = file_core::TransferWorkKey::top_level(0);
    let second_key = file_core::TransferWorkKey::top_level(1);
    let nested_key = file_core::TransferWorkKey {
        transfer_index: 1,
        relative_path: PathBuf::from("nested/child"),
    };
    let nested_source = second_source.join("nested/child");

    assert!(queue.accept_durable_direct_move_commit(task_id, &first_key, &first_source, 4));
    assert_eq!(queue.tasks()[0].status_label(), "Running");
    assert!(!queue.accept_durable_direct_move_commit(task_id, &first_key, &first_source, 4));
    assert!(!queue.accept_durable_direct_move_commit(task_id, &first_key, &first_source, 5));
    assert!(!queue.accept_durable_direct_move_commit(task_id, &second_key, &first_source, 4));
    assert!(queue.accept_durable_direct_move_commit(task_id, &nested_key, &nested_source, 9));
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert!(!queue.accept_durable_direct_move_commit(task_id, &second_key, &second_source, 4));
}

#[test]
fn persistence_requests_are_fifo_across_multiple_accepts() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    queue.enqueue(sample_operation());
    queue.enqueue(sample_operation());

    let first = queue
        .take_next_persistence_request()
        .expect("first insert must be available");
    let first_id = first.request_id;
    assert!(queue.take_next_persistence_request().is_none());
    let first_outcome = execute_file_operation_persistence(first);
    queue.accept_persistence_outcome(first_outcome);

    let second = queue
        .take_next_persistence_request()
        .expect("second insert must remain at the head of the FIFO");
    assert!(matches!(
        second.action,
        FileOperationPersistenceAction::Insert { .. }
    ));
    assert_ne!(second.request_id, first_id);
}

#[test]
fn failed_recoverable_insert_removes_transient_task_without_starting_it() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    queue.enqueue(sample_transfer_operation());
    let request = queue
        .take_next_persistence_request()
        .expect("insert must be available");

    std::fs::remove_file(&database_path).unwrap();
    std::fs::create_dir(&database_path).unwrap();
    let acceptance = queue.accept_persistence_outcome(execute_file_operation_persistence(request));

    assert!(acceptance.error.is_some());
    assert!(acceptance.rejected_task_id.is_some());
    assert!(queue.tasks().is_empty());
    assert!(queue.active_subscription().is_none());
}

#[test]
fn canceling_task_stays_canceling_when_insert_fails() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    let task_id = match queue.enqueue(sample_operation()) {
        FileOperationEnqueueOutcome::Queued { task_id }
        | FileOperationEnqueueOutcome::QueuedWithStorageWarning { task_id, .. } => task_id,
        FileOperationEnqueueOutcome::Rejected { error } => panic!("enqueue failed: {error}"),
    };
    queue.cancel(task_id);
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Canceling);
    let request = queue.take_next_persistence_request().unwrap();
    std::fs::remove_file(&database_path).unwrap();
    std::fs::create_dir(&database_path).unwrap();

    let acceptance = queue.accept_persistence_outcome(execute_file_operation_persistence(request));

    assert!(acceptance.error.is_some());
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Canceling);
}

#[test]
fn shutdown_canceling_task_stays_canceling_when_insert_fails() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    queue.enqueue(sample_operation());
    let request = queue.take_next_persistence_request().unwrap();
    queue.begin_application_shutdown();
    std::fs::remove_file(&database_path).unwrap();
    std::fs::create_dir(&database_path).unwrap();

    let acceptance = queue.accept_persistence_outcome(execute_file_operation_persistence(request));

    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Running);
    assert!(queue.tasks()[0].cancel.is_cancelled());
    assert!(queue.active_subscription().is_some());
}

#[test]
fn shutdown_persists_in_flight_recoverable_insert_into_the_final_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store);
    queue.enqueue(sample_transfer_operation());
    let request = queue
        .take_next_persistence_request()
        .expect("insert must be in flight");

    let disposition = queue.begin_application_shutdown();
    assert!(disposition.waiting_for_operation_ids.is_empty());
    assert!(disposition.interrupted_recoverable_tasks.is_empty());

    let acceptance = queue.accept_persistence_outcome(execute_file_operation_persistence(request));
    assert!(matches!(
        acceptance.persisted_shutdown_operation,
        Some(PersistedShutdownFileOperation::Recoverable(
            StoredInterruptedRecoverableTask {
                status: StoredTaskStatus::RecoveryPending,
                ..
            }
        ))
    ));
}
fn completed_checkpoint(path: &std::path::Path) -> TransferCheckpoint {
    TransferCheckpoint::Completed(file_core::CompletedTarget {
        path: path.to_path_buf(),
        identity: file_core::FileIdentity {
            device: 1,
            inode: 2,
            object_kind: file_core::FileObjectKind::RegularFile,
            size: 3,
            modified_seconds: 4,
            modified_nanoseconds: 5,
            changed_seconds: 6,
            changed_nanoseconds: 7,
            symbolic_link_target: None,
        },
        fingerprint: file_core::ObjectFingerprint([8; 32]),
    })
}

fn persist_terminal_checkpoint(
    store: &TaskQueueStore,
    task_id: u64,
    checkpoint: &TransferCheckpoint,
) {
    let kind = match checkpoint {
        TransferCheckpoint::Completed(_) => {
            file_operation_store::StoredTransferCheckpointKind::Completed
        }
        TransferCheckpoint::Failed { .. } => {
            file_operation_store::StoredTransferCheckpointKind::Failed
        }
        _ => panic!("test terminal checkpoint must be completed or failed"),
    };
    let checkpoint = file_operation_store::StoredTransferCheckpoint::new(
        kind,
        serde_json::to_string(checkpoint).unwrap(),
    )
    .unwrap();
    store
        .compare_and_swap_transfer_checkpoint(
            task_id,
            &file_operation_store::StoredTransferWorkKey::top_level(0),
            0,
            &checkpoint,
        )
        .unwrap();
}

#[test]
fn copy_and_move_fail_closed_without_operation_store() {
    let mut queue = FileOperationQueue::new();

    let FileOperationEnqueueOutcome::Rejected { error } =
        queue.enqueue(sample_transfer_operation())
    else {
        panic!("transfer without a store must be rejected");
    };

    assert!(error.contains("storage is unavailable"));
    assert!(queue.tasks().is_empty());
}

#[test]
fn recoverable_transfer_enqueue_atomically_creates_journal() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());

    assert!(queue.enqueue(sample_transfer_operation()).error().is_none());

    let task = &queue.tasks()[0];
    let snapshot = store
        .read_transfer_recovery(task.stored_id.unwrap())
        .unwrap();
    assert_eq!(snapshot.journal_entries.len(), 1);
    assert_eq!(
        snapshot.journal_entries[0].checkpoint.kind,
        file_operation_store::StoredTransferCheckpointKind::AwaitingManifest
    );
    assert!(queue.active_subscription().unwrap().store.is_some());
}

#[test]
fn only_completed_failed_and_canceled_statuses_are_terminal() {
    for (status, expected) in [
        (FileOperationStatus::Pending, false),
        (FileOperationStatus::Running, false),
        (FileOperationStatus::Paused, false),
        (FileOperationStatus::Canceling, false),
        (FileOperationStatus::Failed, true),
        (FileOperationStatus::Completed, true),
        (FileOperationStatus::Canceled, true),
    ] {
        assert_eq!(status.is_terminal(), expected, "{status:?}");
    }
}

#[test]
fn clearing_all_terminal_tasks_keeps_running_and_pending_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    for _ in 0..5 {
        assert!(queue.enqueue(sample_operation()).error().is_none());
    }
    let completed_id = queue.tasks()[0].id;
    let completed_id_stored = queue.tasks()[0].stored_id.unwrap();
    let failed_id = queue.tasks()[1].id;
    let failed_id_stored = queue.tasks()[1].stored_id.unwrap();
    let canceled_id = queue.tasks()[2].id;
    let canceled_id_stored = queue.tasks()[2].stored_id.unwrap();
    let running_id = queue.tasks()[3].id;
    let pending_id = queue.tasks()[4].id;
    let running_id_stored = queue.tasks()[3].stored_id.unwrap();
    let pending_id_stored = queue.tasks()[4].stored_id.unwrap();
    assert_eq!(
        queue.finish(completed_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert!(queue.cancel(canceled_id).is_none());
    assert_eq!(
        queue
            .finish(
                failed_id,
                FileOperationFinish::Failed("create failed".to_owned()),
            )
            .0,
        Some(FileOperationTerminalStatus::Failed)
    );
    assert!(queue.has_terminal_tasks());

    assert!(queue.clear_terminal_tasks().is_none());

    assert!(!queue.has_terminal_tasks());

    let remaining_ids = queue.tasks().iter().map(|task| task.id).collect::<Vec<_>>();
    assert_eq!(remaining_ids, vec![running_id, pending_id]);
    assert!(store.read_task(completed_id_stored).unwrap().is_none());
    assert!(store.read_task(failed_id_stored).unwrap().is_none());
    assert!(store.read_task(canceled_id_stored).unwrap().is_none());
    assert!(store.read_task(running_id_stored).unwrap().is_some());
    assert!(store.read_task(pending_id_stored).unwrap().is_some());
}

#[test]
fn clearing_all_terminal_tasks_keeps_memory_when_persisted_delete_fails() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store);
    assert!(queue.enqueue(sample_operation()).error().is_none());
    assert!(queue.enqueue(sample_operation()).error().is_none());
    let completed_id = queue.tasks()[0].id;
    let running_id = queue.tasks()[1].id;
    assert_eq!(
        queue.finish(completed_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    std::fs::remove_file(&database_path).unwrap();
    std::fs::create_dir(&database_path).unwrap();

    let error = queue
        .clear_terminal_tasks()
        .expect("invalid database path must fail bulk deletion");

    assert!(error.contains("File operation queue storage failed"));
    assert_eq!(
        queue.tasks().iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![completed_id, running_id]
    );
}

#[test]
fn clearing_terminal_task_removes_memory_and_persisted_record() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );

    assert!(queue.clear_terminal_task(task_id).is_none());

    assert!(queue.tasks().is_empty());
    assert!(store.read_task(task_id_stored).unwrap().is_none());
}

#[test]
fn clearing_rejects_nonterminal_tasks() {
    let mut queue = FileOperationQueue::new();
    assert!(queue.enqueue(sample_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;

    assert!(queue.clear_terminal_task(task_id).is_none());

    assert_eq!(queue.tasks().len(), 1);
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Running);
}

#[test]
fn clearing_keeps_terminal_task_when_persisted_delete_fails() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("state.sqlite");
    let store = TaskQueueStore::new(&database_path).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    std::fs::remove_file(&database_path).unwrap();
    std::fs::create_dir(&database_path).unwrap();

    let error = queue
        .clear_terminal_task(task_id)
        .expect("invalid database path must fail deletion");

    assert!(error.contains("File operation queue storage failed"));
    assert_eq!(queue.tasks().len(), 1);
    assert_eq!(queue.tasks()[0].id, task_id);
}

#[test]
fn terminal_completion_releases_recoverable_runner_lease() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_transfer_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        task_id_stored,
        &completed_checkpoint(std::path::Path::new("/tmp/target")),
    );

    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert!(store
        .try_acquire_recoverable_task_runner(task_id_stored)
        .unwrap()
        .is_some());
    assert_eq!(queue.tasks().len(), 1);
}

#[test]
fn terminal_persistence_keeps_recoverable_runner_lease_until_fifo_ack() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store.clone());
    queue.enqueue(sample_transfer_operation());

    let insert = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(insert));
    let ui_task_id = queue.tasks()[0].id;
    let stored_task_id = queue.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        stored_task_id,
        &completed_checkpoint(std::path::Path::new("/tmp/target")),
    );
    assert_eq!(
        queue.finish(ui_task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );

    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_none());
    assert!(queue.tasks()[0].terminal_persistence_pending);
    assert!(queue.clear_terminal_task(ui_task_id).is_none());

    let running_state = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(running_state));
    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_none());

    let terminal_state = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(terminal_state));
    assert!(!queue.tasks()[0].terminal_persistence_pending);
    assert!(store
        .try_acquire_recoverable_task_runner(stored_task_id)
        .unwrap()
        .is_some());
}

#[test]
fn shutdown_snapshots_recoverable_terminal_task_until_terminal_persistence_ack() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store_with_deferred_persistence(store.clone());
    queue.enqueue(sample_transfer_operation());

    let insert = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(insert));
    let task_id = queue.tasks()[0].id;
    let stored_task_id = queue.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        stored_task_id,
        &completed_checkpoint(std::path::Path::new("/tmp/target")),
    );
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded).0,
        Some(FileOperationTerminalStatus::Completed)
    );
    let running_state = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(running_state));

    let disposition = queue.begin_application_shutdown();

    assert_eq!(
        disposition
            .interrupted_recoverable_tasks
            .iter()
            .map(|task| task.task_id)
            .collect::<Vec<_>>(),
        vec![stored_task_id]
    );
    assert_eq!(
        disposition.interrupted_recoverable_tasks[0].status,
        StoredTaskStatus::RecoveryPending
    );

    let terminal_state = queue.take_next_persistence_request().unwrap();
    queue.accept_persistence_outcome(execute_file_operation_persistence(terminal_state));
    assert!(!queue.tasks()[0].terminal_persistence_pending);
}

#[test]
fn second_process_does_not_restore_task_with_active_runner_lease() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut owner = FileOperationQueue::new();
    owner.set_store(store.clone());
    assert!(owner.enqueue(sample_transfer_operation()).error().is_none());
    let task_id = owner.tasks()[0].stored_id.unwrap();

    let mut observer = FileOperationQueue::new();
    assert!(observer.set_store_and_restore(store.clone()).is_none());

    assert!(observer.tasks().is_empty());
    assert_eq!(
        store.read_task(task_id).unwrap().unwrap().status,
        StoredTaskStatus::Running
    );
    assert_eq!(owner.tasks().len(), 1);
}

#[test]
fn restore_coordinator_keeps_restart_task_claims_in_one_process() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let disposition = original.begin_application_shutdown();
    assert_eq!(disposition.journal_read_count, 0);
    original.release_application_shutdown_ownership();
    let coordinator = store
        .try_acquire_recoverable_restore_coordinator()
        .unwrap()
        .unwrap();

    let mut losing_process = FileOperationQueue::new();
    assert!(losing_process
        .set_store_and_restore(store.clone())
        .is_none());
    assert!(losing_process.tasks().is_empty());
    drop(coordinator);

    let mut recovery_owner = FileOperationQueue::new();
    assert!(recovery_owner.set_store_and_restore(store).is_none());
    assert_eq!(recovery_owner.tasks().len(), 2);
}

#[test]
fn restore_reloads_task_state_after_coordinator_acquisition() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let task_id = original.tasks()[0].stored_id.unwrap();
    let stale_snapshot = store.read_tasks().unwrap();
    assert_eq!(stale_snapshot[0].status, StoredTaskStatus::Running);
    store
        .update_task_state(
            task_id,
            StoredTaskStatus::Failed,
            StoredProgress::pending(),
            Some("recovery blocked in owning process"),
        )
        .unwrap();
    drop(original);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store).is_none());
    assert!(restored.tasks().is_empty());
}

#[test]
fn current_recovery_journal_is_requeued_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let task_id = original.tasks()[0].stored_id.unwrap();
    drop(original);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());

    assert_eq!(restored.tasks().len(), 1);
    assert_eq!(restored.tasks()[0].id, task_id);
    assert_eq!(restored.tasks()[0].status, FileOperationStatus::Running);
}

#[test]
fn paused_and_canceling_recovery_states_preserve_controls() {
    for (stored_status, expected_status, cancellation_expected) in [
        (StoredTaskStatus::Paused, FileOperationStatus::Paused, false),
        (
            StoredTaskStatus::Canceling,
            FileOperationStatus::Canceling,
            true,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut original = FileOperationQueue::new();
        original.set_store(store.clone());
        assert!(original
            .enqueue(sample_transfer_operation())
            .error()
            .is_none());
        let task_id = original.tasks()[0].stored_id.unwrap();
        store.update_status(task_id, stored_status).unwrap();
        drop(original);

        let mut restored = FileOperationQueue::new();
        assert!(restored.set_store_and_restore(store.clone()).is_none());

        assert_eq!(restored.tasks()[0].status, expected_status);
        assert_eq!(
            restored
                .active_subscription()
                .unwrap()
                .controls
                .cancellation_token()
                .is_cancelled(),
            cancellation_expected
        );
    }
}

#[test]
fn recovery_pending_transfer_with_unfinished_checkpoint_is_requeued_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let task_id = original.tasks()[0].stored_id.unwrap();
    store
        .update_task_state(
            task_id,
            StoredTaskStatus::RecoveryPending,
            StoredProgress::pending(),
            Some("transfer journal failed"),
        )
        .unwrap();
    drop(original);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());

    assert_eq!(restored.tasks().len(), 1);
    assert_eq!(restored.tasks()[0].status, FileOperationStatus::Running);
}

#[test]
fn pending_recoverable_cancel_uses_recovery_runner() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    queue.enqueue(sample_operation());
    queue.enqueue(sample_transfer_operation());
    let transfer_id = queue.tasks()[1].id;
    let transfer_id_stored = queue.tasks()[1].stored_id.unwrap();
    assert_eq!(queue.tasks()[1].status, FileOperationStatus::Pending);

    assert!(queue.cancel(transfer_id).is_none());

    assert_eq!(queue.tasks()[1].status, FileOperationStatus::Canceling);
    assert!(queue.tasks()[1].cancel.is_cancelled());
    assert_eq!(
        store.read_task(transfer_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::Canceling
    );
    assert!(!store
        .read_transfer_recovery(transfer_id_stored)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[tokio::test]
async fn shutdown_preserves_nonterminal_recoverable_task_until_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    queue.enqueue(sample_transfer_operation());
    let task_id = queue.tasks()[0].id;
    let stored_id = queue.tasks()[0].stored_id.unwrap();
    let mut controls = queue.active_subscription().unwrap().controls;

    let disposition = queue.begin_application_shutdown();

    assert_eq!(
        disposition.waiting_for_operation_ids,
        BTreeSet::from([task_id])
    );
    assert_eq!(disposition.stopping_signal_count, 1);
    assert_eq!(disposition.journal_read_count, 0);
    assert_eq!(disposition.interrupted_recoverable_tasks.len(), 1);
    assert_eq!(
        disposition.interrupted_recoverable_tasks[0].status,
        StoredTaskStatus::RecoveryPending
    );
    assert!(disposition.transient_task_ids.is_empty());
    assert!(matches!(
        controls.wait_until_running().await,
        Err(file_core::FileError::ApplicationStopping)
    ));
    assert_eq!(queue.tasks().len(), 1);
    assert!(store
        .try_acquire_recoverable_task_runner(stored_id)
        .unwrap()
        .is_none());

    queue.release_application_shutdown_ownership();
    assert!(store
        .try_acquire_recoverable_task_runner(stored_id)
        .unwrap()
        .is_some());
    assert!(!store
        .read_transfer_recovery(stored_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[test]
fn recovery_interruption_persists_distinct_restart_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_transfer_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();

    assert_eq!(
        queue.finish(
            task_id,
            FileOperationFinish::RecoveryInterrupted("checkpoint write failed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert_eq!(
        store.read_task(task_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::RecoveryPending
    );
    drop(queue);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());
    assert_eq!(restored.tasks()[0].status, FileOperationStatus::Running);
}

#[test]
fn late_cancel_does_not_mask_recorded_recoverable_failure() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("late-cancel-state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_transfer_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        task_id_stored,
        &TransferCheckpoint::Failed {
            final_target: None,
            diagnostic: "source changed".to_owned(),
        },
    );
    assert!(queue.cancel(task_id).is_none());

    assert_eq!(
        queue.finish(
            task_id,
            FileOperationFinish::Failed("source changed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert_eq!(
        store.read_task(task_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::Failed
    );
}

#[test]
fn cancellation_checkpoint_interruption_stays_recoverable() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("cancel-state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    assert!(queue.enqueue(sample_transfer_operation()).error().is_none());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();
    assert!(queue.cancel(task_id).is_none());
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Canceling);

    assert_eq!(
        queue.finish(
            task_id,
            FileOperationFinish::RecoveryInterrupted("cancel checkpoint failed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert_eq!(
        store.read_task(task_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::RecoveryPending
    );
    assert!(!store
        .read_transfer_recovery(task_id_stored)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[test]
fn terminal_recoverable_failure_discards_terminal_recovery_details() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    queue.enqueue(sample_transfer_operation());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        task_id_stored,
        &TransferCheckpoint::Failed {
            final_target: None,
            diagnostic: "source changed".to_owned(),
        },
    );

    assert_eq!(
        queue.finish(
            task_id,
            FileOperationFinish::Failed("source changed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert!(store
        .read_transfer_recovery(task_id_stored)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[test]
fn blocked_recovery_preserves_details_without_automatic_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    queue.enqueue(sample_transfer_operation());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();

    assert_eq!(
        queue.finish(
            task_id,
            FileOperationFinish::RecoveryBlocked("owner marker changed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert_eq!(
        store.read_task(task_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::Failed
    );
    assert!(!store
        .read_transfer_recovery(task_id_stored)
        .unwrap()
        .journal_entries
        .is_empty());

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());
    assert!(restored.tasks().is_empty());
}

#[test]
fn terminal_failed_transfer_with_unfinished_checkpoint_is_not_requeued() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let task_id = original.tasks()[0].stored_id.unwrap();
    store
        .update_task_state(
            task_id,
            StoredTaskStatus::Failed,
            StoredProgress::pending(),
            Some("source changed"),
        )
        .unwrap();
    drop(original);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());

    assert!(restored.tasks().is_empty());
}

#[test]
fn failed_transfer_with_terminal_checkpoint_is_not_requeued() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original = FileOperationQueue::new();
    original.set_store(store.clone());
    assert!(original
        .enqueue(sample_transfer_operation())
        .error()
        .is_none());
    let task_id = original.tasks()[0].stored_id.unwrap();
    persist_terminal_checkpoint(
        &store,
        task_id,
        &completed_checkpoint(std::path::Path::new("/tmp/target")),
    );
    store
        .update_task_state(
            task_id,
            StoredTaskStatus::Failed,
            StoredProgress::pending(),
            Some("late task state write"),
        )
        .unwrap();
    drop(original);

    let mut restored = FileOperationQueue::new();
    assert!(restored.set_store_and_restore(store.clone()).is_none());

    assert!(restored.tasks().is_empty());
}

#[test]
fn local_and_persisted_task_ids_do_not_collide() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(sample_operation());
    let local_id = queue.tasks()[0].id;

    let root = std::env::temp_dir().join(format!(
        "app-ui-operation-queue-test-{}-{}",
        std::process::id(),
        local_id
    ));
    let store = TaskQueueStore::new(root.join("state.sqlite")).unwrap();
    queue.set_store(store);
    queue.enqueue(sample_operation());

    let second_local_id = queue.tasks()[1].id;
    let persisted_id = queue.tasks()[1].stored_id.unwrap();
    assert!(local_id > i64::MAX as u64);
    assert!(second_local_id > i64::MAX as u64);
    assert!(persisted_id <= i64::MAX as u64);
    assert_ne!(local_id, persisted_id);
    assert_ne!(second_local_id, persisted_id);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn finished_tasks_stay_until_queue_is_cleared() {
    let root = std::env::temp_dir().join(format!(
        "app-ui-operation-queue-finish-test-{}",
        std::process::id()
    ));
    let store = TaskQueueStore::new(root.join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    queue.enqueue(sample_operation());
    let task_id = queue.tasks()[0].id;
    let task_id_stored = queue.tasks()[0].stored_id.unwrap();

    let (terminal_status, error) = queue.finish(task_id, FileOperationFinish::Succeeded);

    assert_eq!(
        terminal_status,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert!(error.is_none());
    assert_eq!(queue.tasks().len(), 1);
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Completed);
    assert_eq!(queue.tasks()[0].progress.fraction(), Some(1.0));
    assert!(!queue.has_active_task());
    assert_eq!(queue.unread_count(), 1);
    assert_eq!(
        store.read_task(task_id_stored).unwrap().unwrap().status,
        StoredTaskStatus::Completed
    );

    queue.open_panel();
    assert_eq!(queue.unread_count(), 0);

    let disposition = queue.begin_application_shutdown();
    assert!(disposition.waiting_for_operation_ids.is_empty());
    store
        .commit_application_shutdown(file_operation_store::StoredApplicationShutdown {
            browser_session: file_operation_store::StoredBrowserSessionShutdown::Skip,
            user_preferences: None,
            interrupted_recoverable_tasks: disposition.interrupted_recoverable_tasks,
            transient_task_ids: disposition.transient_task_ids,
        })
        .unwrap();
    queue.release_application_shutdown_ownership();
    assert!(store.read_tasks().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_warning_keeps_completed_status_and_marks_the_task_for_attention() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(sample_operation());
    let task_id = queue.tasks()[0].id;

    let (terminal_status, error) = queue.finish(
        task_id,
        FileOperationFinish::SucceededWithWarning("undo tracking failed".to_owned()),
    );

    assert_eq!(
        terminal_status,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert!(error.is_none());
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Completed);
    assert_eq!(
        queue.tasks()[0].completion_warning.as_deref(),
        Some("undo tracking failed")
    );
    assert!(queue.has_unread_warning_task());
}

#[test]
fn pending_and_terminal_tasks_reject_completion_messages() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(sample_operation());
    queue.enqueue(sample_operation());
    let running_task_id = queue.tasks()[0].id;
    let pending_task_id = queue.tasks()[1].id;

    assert_eq!(
        queue.finish(pending_task_id, FileOperationFinish::Succeeded),
        (None, None)
    );
    assert_eq!(
        queue.tasks()[1].status,
        FileOperationStatus::Pending,
        "pending task must not accept another task's completion"
    );
    assert_eq!(
        queue
            .finish(running_task_id, FileOperationFinish::Succeeded)
            .0,
        Some(FileOperationTerminalStatus::Completed)
    );
    assert_eq!(
        queue.finish(running_task_id, FileOperationFinish::Succeeded),
        (None, None)
    );
}

#[test]
fn late_cancel_request_keeps_successful_completion_semantics() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(sample_operation());
    let task_id = queue.tasks()[0].id;
    queue.cancel(task_id);

    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded),
        (Some(FileOperationTerminalStatus::Completed), None)
    );
    assert_eq!(queue.tasks()[0].status, FileOperationStatus::Completed);
}

#[test]
fn failed_operation_records_once_but_cancel_completion_does_not() {
    RECORDED_FILE_OPERATION_FAILURES.with(|count| count.set(0));
    let mut failed_queue = FileOperationQueue::new();
    failed_queue.enqueue(sample_operation());
    let failed_task_id = failed_queue.tasks()[0].id;
    let mut canceled_queue = FileOperationQueue::new();
    canceled_queue.enqueue(sample_operation());
    let canceled_task_id = canceled_queue.tasks()[0].id;
    canceled_queue.cancel(canceled_task_id);

    assert_eq!(
        failed_queue.finish(
            failed_task_id,
            FileOperationFinish::Failed("create failed".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Failed), None)
    );
    assert_eq!(
        failed_queue.finish(
            failed_task_id,
            FileOperationFinish::Failed("duplicate failure".to_owned()),
        ),
        (None, None)
    );
    assert_eq!(
        canceled_queue.finish(
            canceled_task_id,
            FileOperationFinish::Failed("cancelled".to_owned()),
        ),
        (Some(FileOperationTerminalStatus::Canceled), None)
    );

    RECORDED_FILE_OPERATION_FAILURES.with(|count| assert_eq!(count.get(), 1));
    assert_eq!(failed_queue.tasks()[0].status, FileOperationStatus::Failed);
    assert_eq!(
        failed_queue.tasks()[0].error.as_deref(),
        Some("create failed")
    );
    assert_eq!(
        canceled_queue.tasks()[0].status,
        FileOperationStatus::Canceled
    );
    assert_eq!(canceled_queue.tasks()[0].error, None);
}
