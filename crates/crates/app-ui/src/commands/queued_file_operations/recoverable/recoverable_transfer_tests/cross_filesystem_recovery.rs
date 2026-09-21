use super::*;

#[derive(Clone)]
struct InterruptSourceRetiredJournal {
    inner: TaskQueueTransferJournal,
    interrupted: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl TransferJournal for InterruptSourceRetiredJournal {
    fn commit(
        &self,
        mutation: TransferJournalMutation,
    ) -> Pin<Box<dyn Future<Output = Result<u64, TransferJournalError>> + Send + '_>> {
        if matches!(
            &mutation,
            TransferJournalMutation::CompareAndSwapCheckpoint {
                checkpoint: file_core::TransferCheckpoint::SourceRetired(_),
                ..
            }
        ) && !self
            .interrupted
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Box::pin(async {
                Err(TransferJournalError::Storage(
                    "injected source-retired checkpoint failure".to_owned(),
                ))
            });
        }
        self.inner.commit(mutation)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sqlite_cross_filesystem_move_resumes_from_identified_retirement_artifact() {
    use std::os::unix::fs::MetadataExt;

    let source_directory = tempfile::tempdir().unwrap();
    let Ok(target_directory) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    if std::fs::metadata(source_directory.path()).unwrap().dev()
        == std::fs::metadata(target_directory.path()).unwrap().dev()
    {
        return;
    }
    let source = source_directory.path().join("source");
    let target = target_directory.path().join("target");
    tokio::fs::write(&source, b"cross-device").await.unwrap();
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    let store = TaskQueueStore::new(source_directory.path().join("state.sqlite")).unwrap();
    let mut original_queue = FileOperationQueue::new();
    original_queue.set_store(store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        original_queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::Strong,
        })
    else {
        panic!("recoverable cross-filesystem move should enqueue");
    };
    let running = original_queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let record = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap()
        .remove(0);
    let (journal, _direct_move_commit_receiver) = task_queue_transfer_journal_channel(
        store.clone(),
        running.controls.clone(),
        std::slice::from_ref(&record),
    );
    let interrupted_journal = InterruptSourceRetiredJournal {
        inner: journal,
        interrupted: Default::default(),
    };

    let error = run_recoverable_transfer(
        record,
        &interrupted_journal,
        FileTransferOptions::new(running.controls),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, RecoverableTransferError::Journal { .. }));
    assert_eq!(
        original_queue.finish(
            task_id,
            FileOperationFinish::RecoveryInterrupted(error.to_string()),
        ),
        (
            Some(crate::operation_queue::FileOperationTerminalStatus::Failed),
            None
        )
    );
    let persisted = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap()
        .remove(0);
    assert!(matches!(
        persisted.checkpoint,
        file_core::TransferCheckpoint::SourceRetirementIntent(retirement)
            if retirement.artifact.is_some()
    ));
    assert!(tokio::fs::symlink_metadata(&source).await.is_err());
    drop(original_queue);

    let mut restored_queue = FileOperationQueue::new();
    assert!(restored_queue
        .set_store_and_restore(store.clone())
        .is_none());
    let restored_running = restored_queue.active_subscription().unwrap();
    let (mut output, _messages) = iced::futures::channel::mpsc::channel(32);
    let completion = run_queued_transfers(
        transfers,
        restored_running.controls,
        restored_running.stored_id.unwrap(),
        task_id,
        &mut output,
        restored_running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::Strong,
    )
    .await;
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    assert_eq!(tokio::fs::read(&target).await.unwrap(), b"cross-device");
    assert_eq!(
        restored_queue.finish(stored_task_id, FileOperationFinish::Succeeded),
        (
            Some(crate::operation_queue::FileOperationTerminalStatus::Completed),
            None
        )
    );
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}
