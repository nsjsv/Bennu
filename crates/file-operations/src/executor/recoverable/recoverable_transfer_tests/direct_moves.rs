//! 移动：改名先报给界面、落盘后再报“已提交”，重启后补报，重复提交被拒。

use super::*;

#[derive(Clone)]
struct ReplayDirectMoveRenamedJournal {
    inner: TaskQueueTransferJournal,
}

impl TransferJournal for ReplayDirectMoveRenamedJournal {
    fn commit(&self, mutations: Vec<TransferJournalMutation>) -> TransferJournalFuture<'_> {
        if commits_checkpoint(&mutations, |checkpoint| {
            matches!(
                checkpoint,
                file_core::TransferCheckpoint::DirectMoveRenamed(_)
            )
        }) {
            return Box::pin(async move {
                let revisions = self.inner.commit(mutations.clone()).await?;
                assert_eq!(
                    self.inner.commit(mutations).await,
                    Err(TransferJournalError::StaleRevision)
                );
                Ok(revisions)
            });
        }
        self.inner.commit(mutations)
    }
}

#[tokio::test]
async fn sqlite_basic_move_emits_durable_target_commit_before_runner_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"move-content").await.unwrap();
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
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
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    let messages = drained_events(messages).await;
    assert!(messages.iter().any(|message| matches!(
        message,
        FileOperationEvent::DirectMovesCommitted {
            task_id: committed_task_id,
            commits,
        } if *committed_task_id == task_id
            && matches!(
                commits.as_slice(),
                [DurableDirectMoveCommit {
                    work_key,
                    source: committed_source,
                    target: committed_target,
                    checkpoint_revision,
                }] if work_key == &file_core::TransferWorkKey::top_level(0)
                    && committed_source == &source
                    && committed_target == &target
                    && *checkpoint_revision > 0
            )
    )));
}

#[tokio::test]
async fn sqlite_basic_move_batch_makes_every_target_durable_before_item_completion() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        tokio::fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(source, target));
    }
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, messages) = tokio::sync::mpsc::channel(64);

    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);

    let FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
        transfers: completed,
        ..
    }) = completion
    else {
        panic!("expected move completion");
    };
    assert_eq!(
        completed
            .iter()
            .map(|transfer| (&transfer.source, &transfer.target))
            .collect::<Vec<_>>(),
        transfers
            .iter()
            .map(|transfer| (&transfer.source, &transfer.target))
            .collect::<Vec<_>>()
    );

    let messages = drained_events(messages).await;
    let direct_commit_batches = messages
        .iter()
        .enumerate()
        .filter_map(|(position, message)| match message {
            FileOperationEvent::DirectMovesCommitted {
                task_id: committed_task_id,
                commits,
            } => Some((position, *committed_task_id, commits.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    // rename 事实必须先于 durable 提交消息到达 UI：列表刷新不等 journal 落盘。
    let renamed_moves = messages
        .iter()
        .enumerate()
        .filter_map(|(position, message)| match message {
            FileOperationEvent::MovesRenamed { moves, .. } => Some((position, moves.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(renamed_moves.len(), 1);
    assert_eq!(renamed_moves[0].1, transfers.len());
    let first_item_completion = messages
        .iter()
        .position(|message| {
            matches!(
                message,
                FileOperationEvent::Progressed {
                    update: FileOperationProgressUpdate::Transfer { completed_items, .. },
                    ..
                } if *completed_items > 0
            )
        })
        .expect("batch should publish item completion progress");
    assert_eq!(direct_commit_batches.len(), 1);
    assert_eq!(direct_commit_batches[0].1, task_id);
    assert_eq!(direct_commit_batches[0].2, transfers.len());
    assert!(renamed_moves[0].0 < direct_commit_batches[0].0);
    assert!(direct_commit_batches[0].0 < first_item_completion);
    let direct_commit_batch = messages
        .iter()
        .find_map(|message| match message {
            FileOperationEvent::DirectMovesCommitted {
                task_id: committed_task_id,
                commits,
            } => Some((*committed_task_id, commits)),
            _ => None,
        })
        .expect("visibility segment should publish one commit batch");
    assert_eq!(direct_commit_batch.0, task_id);
    assert_eq!(direct_commit_batch.1.len(), transfers.len());
    for (index, (transfer, commit)) in transfers
        .iter()
        .zip(direct_commit_batch.1.iter())
        .enumerate()
    {
        assert_eq!(
            commit.work_key,
            file_core::TransferWorkKey::top_level(index as u64)
        );
        assert_eq!(commit.source, transfer.source);
        assert_eq!(commit.target, transfer.target);
        assert!(commit.checkpoint_revision > 0);
    }
}

/// 通道没有余量、界面又取得慢时，"已重命名"也要等到送达，并且仍在 durable 提交之前。
#[tokio::test]
async fn renamed_moves_reach_a_slow_ui_through_a_full_channel() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        tokio::fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(source, target));
    }
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, mut messages) = tokio::sync::mpsc::channel(1);

    let run = {
        let transfers = transfers.clone();
        async move {
            let completion = run_queued_transfers(
                transfers,
                running.controls,
                running.stored_id.unwrap(),
                task_id,
                &output,
                running.store,
                QueuedTransferMode::Move,
                FileOperationVerification::BasicMetadata,
            )
            .await;
            drop(output);
            completion
        }
    };
    let slow_ui = async move {
        let mut received = Vec::new();
        while let Some(message) = messages.recv().await {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            received.push(message);
        }
        received
    };
    let (completion, received) = tokio::join!(run, slow_ui);

    assert!(matches!(
        completion,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Move { .. })
    ));
    let renamed = received
        .iter()
        .position(|message| {
            matches!(
                message,
                FileOperationEvent::MovesRenamed { moves, .. } if moves.len() == transfers.len()
            )
        })
        .expect("the renamed moves must reach the UI");
    let committed = received
        .iter()
        .position(|message| matches!(message, FileOperationEvent::DirectMovesCommitted { .. }))
        .expect("the durable commit must reach the UI");
    assert!(renamed < committed);
}

#[tokio::test]
async fn direct_move_segment_finishes_siblings_of_a_conflicting_item() {
    let directory = tempfile::tempdir().unwrap();
    let first_source = directory.path().join("first-source");
    let second_source = directory.path().join("second-source");
    let third_source = directory.path().join("third-source");
    let first_target = directory.path().join("first-target");
    let second_target = directory.path().join("second-target");
    let third_target = directory.path().join("third-target");
    tokio::fs::write(&first_source, b"first").await.unwrap();
    tokio::fs::write(&second_source, b"second").await.unwrap();
    tokio::fs::write(&third_source, b"third").await.unwrap();
    tokio::fs::write(&second_target, b"occupied").await.unwrap();
    let transfers = vec![
        QueuedTransfer::new(first_source.clone(), first_target.clone()),
        QueuedTransfer::new(second_source.clone(), second_target.clone()),
        QueuedTransfer::new(third_source.clone(), third_target.clone()),
    ];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, _messages) = tokio::sync::mpsc::channel(64);

    let completion = run_queued_transfers(
        transfers,
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    let FileOperationCompletion::Failed {
        partial_outcome: FileOperationOutcome::Move { transfers, .. },
        ..
    } = completion
    else {
        panic!("a target conflict should fail the task");
    };
    // 同一段里的其它记录照常做完；冲突的那条原样不动。
    assert_eq!(
        transfers,
        vec![
            CompletedTransfer {
                source: first_source.clone(),
                target: first_target.clone(),
            },
            CompletedTransfer {
                source: third_source.clone(),
                target: third_target.clone(),
            },
        ]
    );
    assert_eq!(tokio::fs::read(&first_target).await.unwrap(), b"first");
    assert!(tokio::fs::symlink_metadata(&first_source).await.is_err());
    assert_eq!(tokio::fs::read(&second_source).await.unwrap(), b"second");
    assert_eq!(tokio::fs::read(&second_target).await.unwrap(), b"occupied");
    assert_eq!(tokio::fs::read(&third_target).await.unwrap(), b"third");
    assert!(tokio::fs::symlink_metadata(&third_source).await.is_err());
}

#[tokio::test]
async fn direct_move_segment_finishes_other_visible_items_after_one_is_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        tokio::fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(source, target));
    }
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    run_until_checkpoint(
        &store,
        stored_task_id,
        &running.controls,
        is_direct_move_renamed,
    )
    .await;

    let displaced_target = directory.path().join("displaced-target");
    tokio::fs::rename(&transfers[1].target, &displaced_target)
        .await
        .unwrap();
    tokio::fs::write(&transfers[1].target, b"external")
        .await
        .unwrap();

    let (output, _messages) = tokio::sync::mpsc::channel(64);
    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        stored_task_id,
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    let FileOperationCompletion::RecoveryBlocked {
        partial_outcome:
            FileOperationOutcome::Move {
                transfers: completed,
                ..
            },
        ..
    } = completion
    else {
        panic!("a replaced visible target must block recovery");
    };
    assert_eq!(
        completed,
        vec![
            CompletedTransfer {
                source: transfers[0].source.clone(),
                target: transfers[0].target.clone(),
            },
            CompletedTransfer {
                source: transfers[2].source.clone(),
                target: transfers[2].target.clone(),
            },
        ]
    );
    assert_eq!(
        tokio::fs::read(&transfers[1].target).await.unwrap(),
        b"external"
    );
    assert_eq!(
        tokio::fs::read(displaced_target).await.unwrap(),
        b"content-1"
    );
    let restored = load_recoverable_transfer_records(store, stored_task_id)
        .await
        .unwrap();
    assert!(matches!(
        restored[0].checkpoint,
        file_core::TransferCheckpoint::Completed(_)
    ));
    assert!(matches!(
        restored[1].checkpoint,
        file_core::TransferCheckpoint::DirectMoveRenamed(_)
    ));
    assert!(matches!(
        restored[2].checkpoint,
        file_core::TransferCheckpoint::Completed(_)
    ));
}

#[tokio::test]
async fn sqlite_merge_move_emits_the_nested_durable_target_commit() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let source_child = source.join("child");
    let target_child = target.join("child");
    tokio::fs::create_dir(&source).await.unwrap();
    tokio::fs::create_dir(&target).await.unwrap();
    tokio::fs::write(&source_child, b"nested-move")
        .await
        .unwrap();
    let transfers = vec![QueuedTransfer {
        source: source.clone(),
        target: target.clone(),
        conflict_strategy: TransferConflictStrategy::Merge,
    }];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable merge move should enqueue");
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
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    let messages = drained_events(messages).await;
    assert!(messages.iter().any(|message| matches!(
        message,
        FileOperationEvent::DirectMovesCommitted {
            task_id: committed_task_id,
            commits,
        } if *committed_task_id == task_id
            && matches!(
                commits.as_slice(),
                [DurableDirectMoveCommit {
                    work_key,
                    source: committed_source,
                    target: committed_target,
                    checkpoint_revision,
                }] if work_key.transfer_index == 0
                    && work_key.relative_path.as_path() == std::path::Path::new("child")
                    && committed_source == &source_child
                    && committed_target == &target_child
                    && *checkpoint_revision > 0
            )
    )));
    assert!(tokio::fs::symlink_metadata(&source).await.is_err());
    assert_eq!(
        tokio::fs::read(&target_child).await.unwrap(),
        b"nested-move"
    );
}

#[tokio::test]
async fn restored_direct_move_renamed_replays_its_durable_commit() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"direct-move").await.unwrap();
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    run_until_checkpoint(
        &store,
        stored_task_id,
        &running.controls,
        is_direct_move_renamed,
    )
    .await;
    let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
    assert_eq!(
        snapshot.journal_entries[0].checkpoint.kind,
        bennu_state::StoredTransferCheckpointKind::DirectMoveRenamed
    );

    let (output, messages) = tokio::sync::mpsc::channel(32);
    let completion = run_queued_transfers(
        transfers,
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    let commits = drained_events(messages)
        .await
        .into_iter()
        .filter(|message| {
            matches!(
                message,
                FileOperationEvent::DirectMovesCommitted {
                    task_id: committed_task_id,
                    commits,
                } if *committed_task_id == task_id
                    && matches!(
                        commits.as_slice(),
                        [DurableDirectMoveCommit {
                            source: committed_source,
                            target: committed_target,
                            ..
                        }] if committed_source == &source && committed_target == &target
                    )
            )
        })
        .count();
    assert_eq!(commits, 1);
}

#[tokio::test]
async fn duplicate_direct_move_cas_is_rejected_and_original_completes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"direct-move").await.unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { .. } = queue.enqueue(QueuedFileOperation::Move {
        transfers: vec![QueuedTransfer::new(source, target.clone())],
        verification: FileOperationVerification::BasicMetadata,
    }) else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let record = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap()
        .remove(0);
    let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
    let replaying_journal = ReplayDirectMoveRenamedJournal { inner: journal };

    run_recoverable_transfer(
        record,
        &replaying_journal,
        FileTransferOptions::new(running.controls.clone()),
    )
    .await
    .unwrap();

    let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
    assert_eq!(
        snapshot.journal_entries[0].checkpoint.kind,
        bennu_state::StoredTransferCheckpointKind::Completed
    );
    assert_eq!(tokio::fs::read(target).await.unwrap(), b"direct-move");
}
