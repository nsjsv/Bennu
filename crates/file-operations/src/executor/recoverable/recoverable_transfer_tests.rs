use std::sync::Arc;

use super::*;
use crate::test_persistence::{assert_persisted, attach_store};
use crate::{FileOperationEnqueueOutcome, FileOperationFinish, FileOperationQueue};

use file_core::{run_recoverable_transfer, DurableDirectMoveCommit};

mod cancellation_terminalization;
mod cross_filesystem_recovery;
mod direct_moves;
mod driver_adoption;
mod failure_settlement;
mod segments;
mod start_latency_harness;

fn task_queue_transfer_journal(
    store: TaskQueueStore,
    controls: FileOperationControls,
) -> TaskQueueTransferJournal {
    TaskQueueTransferJournal { store, controls }
}

fn commits_checkpoint(
    mutations: &[TransferJournalMutation],
    matches: impl Fn(&file_core::TransferCheckpoint) -> bool,
) -> bool {
    mutations
        .iter()
        .any(|mutation| matches(mutation.transition.checkpoint()))
}

/// 发送端都已丢弃后,按到达顺序取出通道里剩下的全部事件。
async fn drained_events(
    mut events: tokio::sync::mpsc::Receiver<FileOperationEvent>,
) -> Vec<FileOperationEvent> {
    let mut drained = Vec::new();
    while let Some(event) = events.recv().await {
        drained.push(event);
    }
    drained
}

/// 提交成功后假装进程就此退出：日志停在这一步，下一步的文件操作还没发生。
#[derive(Clone)]
struct StopAfterCheckpointJournal {
    inner: TaskQueueTransferJournal,
    stops_after: fn(&file_core::TransferCheckpoint) -> bool,
}

impl TransferJournal for StopAfterCheckpointJournal {
    fn commit(&self, mutations: Vec<TransferJournalMutation>) -> TransferJournalFuture<'_> {
        let stops = commits_checkpoint(&mutations, self.stops_after);
        Box::pin(async move {
            let revisions = self.inner.commit(mutations).await?;
            if stops {
                return Err(TransferJournalError::Storage(
                    "stopped right after the checkpoint commit".to_owned(),
                ));
            }
            Ok(revisions)
        })
    }
}

/// 把任务的记录推进到 `stops_after` 那一步落盘为止。
async fn run_until_checkpoint(
    store: &TaskQueueStore,
    stored_task_id: u64,
    controls: &FileOperationControls,
    stops_after: fn(&file_core::TransferCheckpoint) -> bool,
) {
    let records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    let journal = StopAfterCheckpointJournal {
        inner: task_queue_transfer_journal(store.clone(), controls.clone()),
        stops_after,
    };
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    assert!(matches!(
        file_core::run_recoverable_segment(
            records,
            &journal,
            &FileTransferOptions::new(controls.clone()),
            &events,
        )
        .await,
        Err(RecoverableTransferError::Journal { .. })
    ));
}

fn is_direct_move_renamed(checkpoint: &file_core::TransferCheckpoint) -> bool {
    matches!(
        checkpoint,
        file_core::TransferCheckpoint::DirectMoveRenamed(_)
    )
}

#[test]
fn application_stopping_has_recoverable_interruption_completion() {
    assert!(matches!(
        recoverable_transfer_failure(
            RecoverableTransferError::FileOperation(file_core::FileError::ApplicationStopping),
            FileOperationOutcome::NoHistory,
        ),
        FileOperationCompletion::RecoveryInterrupted(_, FileOperationOutcome::NoHistory)
    ));
}

#[test]
fn recoverable_cancellation_keeps_the_completed_part() {
    let completed = SettledTransfers {
        transfers: vec![CompletedTransfer {
            source: PathBuf::from("source"),
            target: PathBuf::from("target"),
        }],
        ..SettledTransfers::default()
    };
    assert!(matches!(
        recoverable_transfer_failure(
            RecoverableTransferError::FileOperation(file_core::FileError::Cancelled),
            settled_transfer_outcome(
                QueuedTransferMode::Copy,
                FileOperationHistoryEligibility::Replayable,
                completed,
            ),
        ),
        FileOperationCompletion::Canceled(FileOperationOutcome::Copy { transfers, .. })
            if transfers.len() == 1
    ));
}

#[tokio::test]
async fn recoverable_copy_uses_one_manifest_byte_denominator_for_the_batch() {
    let directory = tempfile::tempdir().unwrap();
    let first_source = directory.path().join("first-source");
    let second_source = directory.path().join("second-source");
    let first_target = directory.path().join("first-target");
    let second_target = directory.path().join("second-target");
    tokio::fs::write(&first_source, vec![1_u8; 10])
        .await
        .unwrap();
    tokio::fs::write(&second_source, vec![2_u8; 990])
        .await
        .unwrap();
    let transfers = vec![
        QueuedTransfer::new(first_source, first_target),
        QueuedTransfer::new(second_source, second_target),
    ];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable copy should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, messages) = tokio::sync::mpsc::channel(32);

    let completion = run_queued_transfers(
        transfers,
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);

    assert!(matches!(
        completion,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Copy { .. })
    ));
    let mut byte_updates = Vec::new();
    for event in drained_events(messages).await {
        if let FileOperationEvent::Progressed {
            task_id: id,
            update:
                FileOperationProgressUpdate::Transfer {
                    completed_items,
                    total_items,
                    bytes: Some((completed_bytes, total_bytes)),
                    ..
                },
            ..
        } = event
        {
            assert_eq!(id, task_id);
            byte_updates.push((completed_bytes, total_bytes, completed_items, total_items));
        }
    }

    assert_eq!(byte_updates.last(), Some(&(1_000, 1_000, 2, 2)));
    assert!(byte_updates
        .iter()
        .all(|(_, total_bytes, _, total_items)| *total_bytes == 1_000 && *total_items == 2));
    assert!(byte_updates
        .windows(2)
        .all(|pair| pair[0].0 <= pair[1].0 && pair[0].2 <= pair[1].2));
}

#[tokio::test]
async fn mismatched_verification_blocks_recovery_without_filesystem_effects() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"content").await.unwrap();
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable copy should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let (output, _messages) = tokio::sync::mpsc::channel(8);

    let completion = run_queued_transfers(
        transfers,
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::Strong,
    )
    .await;
    let FileOperationCompletion::RecoveryBlocked { error, .. } = completion else {
        panic!("verification mismatch must block recovery");
    };
    assert!(error.contains("does not match"));
    assert!(tokio::fs::symlink_metadata(&target).await.is_err());

    assert_eq!(
        queue.finish(task_id, FileOperationFinish::RecoveryBlocked(error)),
        Some(crate::FileOperationTerminalStatus::Failed)
    );
    assert_persisted(&mut queue);
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        bennu_state::StoredTaskStatus::Failed
    );
    assert!(!store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[tokio::test]
async fn sqlite_replace_runner_persists_backup_manifest_until_terminal_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"replacement").await.unwrap();
    tokio::fs::create_dir(&target).await.unwrap();
    tokio::fs::write(target.join("old.txt"), b"old-target")
        .await
        .unwrap();
    let transfers = vec![QueuedTransfer {
        source,
        target: target.clone(),
        conflict_strategy: TransferConflictStrategy::Replace,
    }];
    let store = TaskQueueStore::new(directory.path().join("replace-state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::Strong,
        })
    else {
        panic!("recoverable replace copy should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let (output, _messages) = tokio::sync::mpsc::channel(32);

    let completion = run_queued_transfers(
        transfers,
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::Strong,
    )
    .await;
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    assert_eq!(tokio::fs::read(&target).await.unwrap(), b"replacement");
    let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
    assert!(!snapshot.replacement_manifest_entries.is_empty());

    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Succeeded),
        Some(crate::FileOperationTerminalStatus::Completed)
    );
    assert_persisted(&mut queue);
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[tokio::test]
async fn sqlite_move_runner_preserves_committed_item_and_history_after_restart() {
    // migrate durable journal test

    let directory = tempfile::tempdir().unwrap();
    let first_source = directory.path().join("first-move-source");
    let second_source = directory.path().join("second-move-source");
    let first_target = directory.path().join("first-move-target");
    let second_target = directory.path().join("second-move-target");
    tokio::fs::write(&first_source, b"first-move")
        .await
        .unwrap();
    tokio::fs::write(&second_source, b"second-move")
        .await
        .unwrap();
    let transfers = vec![
        QueuedTransfer::new(first_source.clone(), first_target.clone()),
        QueuedTransfer::new(second_source.clone(), second_target.clone()),
    ];
    let store = TaskQueueStore::new(directory.path().join("move-state.sqlite")).unwrap();
    let mut original_queue = FileOperationQueue::new();
    attach_store(&mut original_queue, store.clone());
    assert!(matches!(
        original_queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        }),
        FileOperationEnqueueOutcome::Queued { .. }
    ));
    let task_id = original_queue.tasks()[0].id;
    assert_persisted(&mut original_queue);
    let running = original_queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let mut records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
    let (output, _messages) = tokio::sync::mpsc::channel(64);
    run_recoverable_transfer(
        records.remove(0),
        &journal,
        FileTransferOptions::new(running.controls.clone()),
    )
    .await
    .unwrap();
    assert!(tokio::fs::symlink_metadata(&first_source).await.is_err());
    drop(original_queue);

    let mut restored_queue = FileOperationQueue::new();
    assert!(restored_queue
        .set_store_and_restore(store.clone())
        .is_none());
    let restored_running = restored_queue.active_subscription().unwrap();
    let stored_task_id = restored_running.stored_id.unwrap();
    let completion = run_queued_transfers(
        transfers,
        restored_running.controls,
        restored_running.stored_id.unwrap(),
        task_id,
        &output,
        restored_running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    let FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
        transfers,
        history_eligibility: FileOperationHistoryEligibility::Replayable,
        ..
    }) = completion
    else {
        panic!("expected resumed move completion");
    };
    assert_eq!(transfers.len(), 2);
    assert_eq!(tokio::fs::read(&first_target).await.unwrap(), b"first-move");
    assert_eq!(
        tokio::fs::read(&second_target).await.unwrap(),
        b"second-move"
    );
    assert!(tokio::fs::symlink_metadata(&second_source).await.is_err());
    assert_eq!(
        restored_queue.finish(stored_task_id, FileOperationFinish::Succeeded),
        Some(crate::FileOperationTerminalStatus::Completed)
    );
    assert_persisted(&mut restored_queue);
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[tokio::test]
async fn sqlite_runner_skips_committed_batch_item_and_finishes_remaining_item() {
    // migrate committed batch test

    let directory = tempfile::tempdir().unwrap();
    let first_source = directory.path().join("first-source");
    let second_source = directory.path().join("second-source");
    let first_target = directory.path().join("first-target");
    let second_target = directory.path().join("second-target");
    tokio::fs::write(&first_source, b"first-original")
        .await
        .unwrap();
    tokio::fs::write(&second_source, b"second-original")
        .await
        .unwrap();
    let transfers = vec![
        QueuedTransfer::new(first_source.clone(), first_target.clone()),
        QueuedTransfer::new(second_source.clone(), second_target.clone()),
    ];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut original_queue = FileOperationQueue::new();
    attach_store(&mut original_queue, store.clone());
    assert!(matches!(
        original_queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        }),
        FileOperationEnqueueOutcome::Queued { .. }
    ));
    let task_id = original_queue.tasks()[0].id;
    assert_persisted(&mut original_queue);
    let running = original_queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let mut records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
    let (output, _messages) = tokio::sync::mpsc::channel(64);
    run_recoverable_transfer(
        records.remove(0),
        &journal,
        FileTransferOptions::new(running.controls.clone()),
    )
    .await
    .unwrap();
    tokio::fs::write(&first_source, b"first-changed-after-commit")
        .await
        .unwrap();
    drop(original_queue);

    let mut restored_queue = FileOperationQueue::new();
    assert!(restored_queue
        .set_store_and_restore(store.clone())
        .is_none());
    let restored_running = restored_queue.active_subscription().unwrap();
    let stored_task_id = restored_running.stored_id.unwrap();
    let completion = run_queued_transfers(
        transfers,
        restored_running.controls,
        restored_running.stored_id.unwrap(),
        task_id,
        &output,
        restored_running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    let FileOperationCompletion::Succeeded(FileOperationOutcome::Copy { transfers, .. }) =
        completion
    else {
        panic!("expected resumed copy completion");
    };
    assert_eq!(transfers.len(), 2);
    assert_eq!(
        tokio::fs::read(&first_target).await.unwrap(),
        b"first-original"
    );
    assert_eq!(
        tokio::fs::read(&second_target).await.unwrap(),
        b"second-original"
    );
    assert_eq!(
        restored_queue.finish(stored_task_id, FileOperationFinish::Succeeded),
        Some(crate::FileOperationTerminalStatus::Completed)
    );
    assert_persisted(&mut restored_queue);
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        bennu_state::StoredTaskStatus::Completed
    );
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}
