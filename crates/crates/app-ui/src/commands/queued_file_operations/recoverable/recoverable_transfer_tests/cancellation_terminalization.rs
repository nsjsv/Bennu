use super::*;

async fn assert_all_records_canceled(store: &TaskQueueStore, stored_task_id: u64, count: usize) {
    let records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    assert_eq!(records.len(), count);
    assert!(records.iter().all(|record| matches!(
        record.checkpoint,
        file_core::TransferCheckpoint::Canceled { .. }
    )));
}

fn assert_canceled_task_finalizes(
    queue: &mut FileOperationQueue,
    store: &TaskQueueStore,
    task_id: u64,
    stored_task_id: u64,
) {
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Canceled),
        (
            Some(crate::operation_queue::FileOperationTerminalStatus::Canceled),
            None
        )
    );
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        file_operation_store::StoredTaskStatus::Canceled
    );
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

#[tokio::test]
async fn cancellation_before_manifest_preparation_terminalizes_every_record() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("copy-source-{index}"));
        let target = directory.path().join(format!("copy-target-{index}"));
        tokio::fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(source, target));
    }
    let store = TaskQueueStore::new(directory.path().join("copy-state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable copy should enqueue");
    };
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    assert!(queue.cancel(task_id).is_none());
    let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);

    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &mut output,
        running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    assert!(matches!(&completion, FileOperationCompletion::Canceled(moved) if moved.is_empty()));
    assert_all_records_canceled(&store, stored_task_id, transfers.len()).await;
    for transfer in &transfers {
        assert!(tokio::fs::symlink_metadata(&transfer.source).await.is_ok());
        assert!(tokio::fs::symlink_metadata(&transfer.target).await.is_err());
    }
    assert_canceled_task_finalizes(&mut queue, &store, task_id, stored_task_id);
}

#[tokio::test]
async fn cancellation_after_direct_move_intents_terminalizes_every_record() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfers = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("move-source-{index}"));
        let target = directory.path().join(format!("move-target-{index}"));
        tokio::fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(source, target));
    }
    let store = TaskQueueStore::new(directory.path().join("move-state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    queue.set_store(store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let controls = running.controls.clone();
    let journal = task_queue_transfer_journal(store.clone(), controls.clone());
    let records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    for record in records {
        assert!(matches!(
            run_recoverable_transfer_to_direct_move_intent(
                record,
                &journal,
                FileTransferOptions::new(controls.clone()),
            )
            .await
            .unwrap(),
            DirectMoveIntentBoundary::Intent(_)
        ));
    }
    assert!(
        load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap()
            .iter()
            .all(|record| matches!(
                record.checkpoint,
                file_core::TransferCheckpoint::DirectMoveIntent(_)
            ))
    );
    assert!(queue.cancel(task_id).is_none());
    let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);

    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &mut output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    assert!(matches!(&completion, FileOperationCompletion::Canceled(moved) if moved.is_empty()));
    assert_all_records_canceled(&store, stored_task_id, transfers.len()).await;
    for transfer in &transfers {
        assert!(tokio::fs::symlink_metadata(&transfer.source).await.is_ok());
        assert!(tokio::fs::symlink_metadata(&transfer.target).await.is_err());
    }
    assert_canceled_task_finalizes(&mut queue, &store, task_id, stored_task_id);
}
