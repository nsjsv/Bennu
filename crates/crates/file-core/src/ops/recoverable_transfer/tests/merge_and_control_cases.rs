use super::*;

pub(super) async fn populate_merge_directories(root: &Path) -> (PathBuf, PathBuf) {
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir(&source).await.unwrap();
    fs::create_dir(&target).await.unwrap();
    fs::write(source.join("new.txt"), b"new-content")
        .await
        .unwrap();
    fs::write(source.join("skip.txt"), b"source-collision")
        .await
        .unwrap();
    fs::write(target.join("skip.txt"), b"target-collision")
        .await
        .unwrap();
    fs::create_dir(source.join("nested")).await.unwrap();
    fs::create_dir(target.join("nested")).await.unwrap();
    fs::write(source.join("nested/new.txt"), b"nested-new")
        .await
        .unwrap();
    fs::write(source.join("nested/skip.txt"), b"nested-source-collision")
        .await
        .unwrap();
    fs::write(target.join("nested/skip.txt"), b"nested-target-collision")
        .await
        .unwrap();
    (source, target)
}

async fn assert_merge_target_has_only_complete_children(target: &Path) {
    let new_path = target.join("new.txt");
    if fs::symlink_metadata(&new_path).await.is_ok() {
        assert_eq!(fs::read(new_path).await.unwrap(), b"new-content");
    }
    let nested_new_path = target.join("nested/new.txt");
    if fs::symlink_metadata(&nested_new_path).await.is_ok() {
        assert_eq!(fs::read(nested_new_path).await.unwrap(), b"nested-new");
    }
    assert_eq!(
        fs::read(target.join("skip.txt")).await.unwrap(),
        b"target-collision"
    );
    assert_eq!(
        fs::read(target.join("nested/skip.txt")).await.unwrap(),
        b"nested-target-collision"
    );
}

#[tokio::test]
async fn merge_restart_after_skipped_conflict_continues_next_child() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source).await.unwrap();
    fs::create_dir(&target).await.unwrap();
    fs::write(source.join("a-conflict"), b"source-conflict")
        .await
        .unwrap();
    fs::write(target.join("a-conflict"), b"target-conflict")
        .await
        .unwrap();
    fs::write(source.join("b-copy"), b"copy-me").await.unwrap();
    let request = transfer_request(
        source,
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Merge,
    );
    let journal = MemoryJournal::new(606, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);
    for _ in 0..8 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if matches!(
            &record.checkpoint,
            TransferCheckpoint::Merging(MergeTransfer { next_child: 1, .. })
        ) {
            break;
        }
    }
    let serialized = serde_json::to_vec(&record).unwrap();
    let decoded: TransferJournalRecord = serde_json::from_slice(&serialized).unwrap();
    let recovered = journal.record(decoded.request);

    run_recoverable_transfer(recovered, &journal, running_transfer_options())
        .await
        .unwrap();

    assert_eq!(
        fs::read(target.join("a-conflict")).await.unwrap(),
        b"target-conflict"
    );
    assert_eq!(fs::read(target.join("b-copy")).await.unwrap(), b"copy-me");
}

#[tokio::test]
async fn move_merge_does_not_adopt_source_entry_added_after_manifest() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source).await.unwrap();
    fs::create_dir(&target).await.unwrap();
    fs::write(source.join("original"), b"original")
        .await
        .unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Merge,
    );
    let journal = MemoryJournal::new(605, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);
    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    fs::write(source.join("late"), b"late").await.unwrap();

    run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap();

    assert_eq!(
        fs::read(target.join("original")).await.unwrap(),
        b"original"
    );
    assert!(fs::symlink_metadata(target.join("late")).await.is_err());
    assert_eq!(fs::read(source.join("late")).await.unwrap(), b"late");
}

#[tokio::test]
async fn copy_merge_recovers_every_child_journal_boundary() {
    for failed_attempt in 1..=64 {
        let directory = tempdir().unwrap();
        let (source, target) = populate_merge_directories(directory.path()).await;
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Copy,
            TransferConflictStrategy::Merge,
        );
        let journal = MemoryJournal::new(
            800 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        let first = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await;
        if first.is_ok() {
            assert!(journal.attempt_count() < failed_attempt);
            break;
        }
        assert!(matches!(
            first,
            Err(RecoverableTransferError::Journal { .. })
        ));
        assert_merge_target_has_only_complete_children(&target).await;
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();

        assert_eq!(
            fs::read(source.join("new.txt")).await.unwrap(),
            b"new-content"
        );
        assert_eq!(
            fs::read(target.join("new.txt")).await.unwrap(),
            b"new-content"
        );
        assert_eq!(
            fs::read(target.join("nested/new.txt")).await.unwrap(),
            b"nested-new"
        );
        assert_merge_target_has_only_complete_children(&target).await;
        assert_no_transfer_artifacts(&target);
    }
}

#[tokio::test]
async fn move_merge_recovers_every_child_journal_boundary() {
    for failed_attempt in 1..=64 {
        let directory = tempdir().unwrap();
        let (source, target) = populate_merge_directories(directory.path()).await;
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Merge,
        );
        let journal = MemoryJournal::new(
            900 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        let first = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await;
        if first.is_ok() {
            assert!(journal.attempt_count() < failed_attempt);
            break;
        }
        assert!(matches!(
            first,
            Err(RecoverableTransferError::Journal { .. })
        ));
        assert_merge_target_has_only_complete_children(&target).await;
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();

        assert!(fs::symlink_metadata(source.join("new.txt")).await.is_err());
        assert_eq!(
            fs::read(target.join("new.txt")).await.unwrap(),
            b"new-content"
        );
        assert!(fs::symlink_metadata(source.join("nested/new.txt"))
            .await
            .is_err());
        assert_eq!(
            fs::read(target.join("nested/new.txt")).await.unwrap(),
            b"nested-new"
        );
        assert_eq!(
            fs::read(source.join("skip.txt")).await.unwrap(),
            b"source-collision"
        );
        assert_eq!(
            fs::read(source.join("nested/skip.txt")).await.unwrap(),
            b"nested-source-collision"
        );
        assert_merge_target_has_only_complete_children(&target).await;
        assert_no_transfer_artifacts(&target);
    }
}

#[tokio::test]
async fn copy_cancellation_cleans_every_persisted_checkpoint() {
    for failed_attempt in 1..=5 {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::write(&source, b"copy-content").await.unwrap();
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Copy,
            TransferConflictStrategy::Fail,
        );
        let journal = MemoryJournal::new(
            700 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        assert!(matches!(
            run_recoverable_transfer(
                journal.record(request.clone()),
                &journal,
                running_transfer_options()
            )
            .await,
            Err(RecoverableTransferError::Journal { .. })
        ));
        let cancellation_outcome = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            canceled_transfer_options(),
        )
        .await;
        assert!(matches!(
            cancellation_outcome,
            Ok(_)
                | Err(RecoverableTransferError::FileOperation(
                    crate::FileError::Cancelled
                ))
        ));

        assert!(matches!(
            (&cancellation_outcome, journal.record(request).checkpoint),
            (
                Err(RecoverableTransferError::FileOperation(
                    crate::FileError::Cancelled
                )),
                TransferCheckpoint::Canceled { .. }
            ) | (Ok(_), TransferCheckpoint::Completed(_))
        ));
        assert_eq!(fs::read(&source).await.unwrap(), b"copy-content");
        if fs::symlink_metadata(&target).await.is_ok() {
            assert_eq!(fs::read(&target).await.unwrap(), b"copy-content");
        }
        assert_no_transfer_artifacts(directory.path());
    }
}

#[tokio::test]
async fn replace_move_cancellation_preserves_complete_old_or_new_data() {
    for failed_attempt in 1..=5 {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::write(&source, b"replacement").await.unwrap();
        fs::write(&target, b"old-target").await.unwrap();
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Replace,
        );
        let journal = MemoryJournal::new(
            800 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        assert!(matches!(
            run_recoverable_transfer(
                journal.record(request.clone()),
                &journal,
                running_transfer_options()
            )
            .await,
            Err(RecoverableTransferError::Journal { .. })
        ));
        let cancellation_outcome = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            canceled_transfer_options(),
        )
        .await;
        assert!(matches!(
            cancellation_outcome,
            Ok(_)
                | Err(RecoverableTransferError::FileOperation(
                    crate::FileError::Cancelled
                ))
        ));

        let target_content = fs::read(&target).await.unwrap();
        assert!(target_content == b"old-target" || target_content == b"replacement");
        if target_content == b"old-target" {
            assert_eq!(fs::read(&source).await.unwrap(), b"replacement");
        }
        let checkpoint = journal.record(request).checkpoint;
        assert!(matches!(
            (&cancellation_outcome, checkpoint),
            (
                Err(RecoverableTransferError::FileOperation(
                    crate::FileError::Cancelled
                )),
                TransferCheckpoint::Canceled { .. }
            ) | (Ok(_), TransferCheckpoint::Completed(_))
        ));
        assert_no_transfer_artifacts(directory.path());
    }
}

#[tokio::test]
async fn cancel_intent_recovers_when_final_checkpoint_write_fails() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"copy-content").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(900, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    journal.set_failure(Some(journal.attempt_count() + 2));

    assert!(matches!(
        run_recoverable_transfer(record, &journal, canceled_transfer_options()).await,
        Err(RecoverableTransferError::Journal { .. })
    ));
    assert!(matches!(
        journal.record(request.clone()).checkpoint,
        TransferCheckpoint::CancelIntent(_)
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"copy-content");
    assert_no_transfer_artifacts(directory.path());

    journal.set_failure(None);
    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::FileOperation(
            crate::FileError::Cancelled
        ))
    ));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Canceled { .. }
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"copy-content");
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn initial_journal_failure_has_no_filesystem_side_effects() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(600, TransferWorkKey::top_level(0), Some(1));

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::Journal { .. })
    ));

    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(fs::symlink_metadata(&target).await.is_err());
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn cancellation_before_prepare_has_no_side_effects() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(700, TransferWorkKey::top_level(0), None);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            FileTransferOptions::running(cancellation),
        )
        .await,
        Err(RecoverableTransferError::FileOperation(
            crate::FileError::Cancelled
        ))
    ));

    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(fs::symlink_metadata(&target).await.is_err());
}
