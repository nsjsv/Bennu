use super::*;

#[tokio::test]
async fn direct_move_batch_defers_fingerprint_and_resumes_to_completion() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(203, TransferWorkKey::top_level(0), None);

    let intent = run_recoverable_transfer_to_direct_move_intent(
        journal.record(request.clone()),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();
    let DirectMoveIntentBoundary::Intent(record) = intent else {
        panic!("basic same-filesystem move should prepare a direct move intent");
    };
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert!(matches!(
        journal.record(request.clone()).checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(fs::symlink_metadata(&target).await.is_err());

    let batch = run_direct_move_batch_to_durable_renamed(
        vec![record],
        &journal,
        &running_transfer_options(),
        &mut |_| {},
    )
    .await
    .unwrap();
    let [DirectMoveBatchRecord::Renamed(record)] = batch.as_slice() else {
        panic!("batch should rename the single record");
    };
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    assert!(matches!(
        journal.record(request.clone()).checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"source");

    let outcome = run_recoverable_transfer(record.clone(), &journal, running_transfer_options())
        .await
        .unwrap();
    assert_eq!(outcome.final_target, Some(target));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Completed(_)
    ));
}

#[tokio::test]
async fn direct_move_intent_boundary_returns_non_direct_before_copy_side_effects() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(204, TransferWorkKey::top_level(0), None);

    let intent = run_recoverable_transfer_to_direct_move_intent(
        journal.record(request),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();
    let DirectMoveIntentBoundary::NotApplicable(record) = intent else {
        panic!("copy must remain on the sequential completion path");
    };
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::StageCreationIntent(_)
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(fs::symlink_metadata(&target).await.is_err());
}

#[tokio::test]
async fn direct_move_intent_boundary_uses_existing_cancellation_cleanup() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(209, TransferWorkKey::top_level(0), None);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert!(matches!(
        run_recoverable_transfer_to_direct_move_intent(
            journal.record(request.clone()),
            &journal,
            FileTransferOptions::running(cancellation),
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
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(fs::symlink_metadata(&target).await.is_err());
}

#[tokio::test]
async fn direct_move_intent_rejects_an_unrelated_target_after_source_disappears() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let moved_source = directory.path().join("moved-source");
    let external = directory.path().join("external");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&external, b"external").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(205, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    rename_noreplace(&source, &target).unwrap();
    fs::rename(&target, &moved_source).await.unwrap();
    fs::rename(&external, &target).await.unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert_eq!(fs::read(&moved_source).await.unwrap(), b"source");
    assert_eq!(fs::read(&target).await.unwrap(), b"external");
}

#[tokio::test]
async fn direct_move_renamed_rejects_target_identity_replacement() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let moved_source = directory.path().join("moved-source");
    let external = directory.path().join("external");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&external, b"external").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(206, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());

    for _ in 0..2 {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    fs::rename(&target, &moved_source).await.unwrap();
    fs::rename(&external, &target).await.unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    assert_eq!(fs::read(&moved_source).await.unwrap(), b"source");
    assert_eq!(fs::read(&target).await.unwrap(), b"external");
}

#[tokio::test]
async fn basic_direct_move_keep_both_reselects_an_occupied_candidate() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&target, b"existing").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::KeepBoth,
    );
    let journal = MemoryJournal::new(207, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    let TransferCheckpoint::DirectMoveIntent(prepared) = &record.checkpoint else {
        panic!("basic KeepBoth move should persist a direct move intent");
    };
    let occupied_candidate = prepared.resolved_target.clone();
    fs::write(&occupied_candidate, b"concurrent").await.unwrap();

    let outcome = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap();
    let final_target = outcome.final_target.unwrap();
    assert_ne!(final_target, target);
    assert_ne!(final_target, occupied_candidate);
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"existing");
    assert_eq!(fs::read(&occupied_candidate).await.unwrap(), b"concurrent");
    assert_eq!(fs::read(&final_target).await.unwrap(), b"source");
}

#[cfg(unix)]
#[tokio::test]
async fn basic_direct_move_cross_device_falls_back_before_any_side_effect() {
    use std::os::unix::fs::MetadataExt;

    let source_directory = tempdir().unwrap();
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
    fs::write(&source, b"cross-device").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(208, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::StageCreationIntent(_)
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"cross-device");
    assert!(fs::symlink_metadata(&target).await.is_err());

    let outcome = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap();
    assert_eq!(outcome.final_target, Some(target.clone()));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"cross-device");
}

#[tokio::test]
async fn cancellation_after_direct_move_rename_before_checkpoint_finishes_forward() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(210, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    rename_noreplace(&source, &target).unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let outcome =
        run_recoverable_transfer(record, &journal, FileTransferOptions::running(cancellation))
            .await
            .unwrap();
    assert_eq!(outcome.final_target, Some(target.clone()));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"source");
}

#[tokio::test]
async fn direct_move_intent_blocks_when_source_is_recreated_after_rename() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"original").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::KeepBoth,
    );
    let journal = MemoryJournal::new(211, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    rename_noreplace(&source, &target).unwrap();
    fs::write(&source, b"recreated").await.unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"recreated");
    assert_eq!(fs::read(&target).await.unwrap(), b"original");
}

#[tokio::test]
async fn direct_move_renamed_ignores_a_recreated_source_path() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"original").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(212, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    for _ in 0..2 {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    fs::write(&source, b"recreated").await.unwrap();

    let outcome = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap();
    assert_eq!(outcome.final_target, Some(target.clone()));
    assert_eq!(fs::read(&source).await.unwrap(), b"recreated");
    assert_eq!(fs::read(&target).await.unwrap(), b"original");
}

#[tokio::test]
async fn cancellation_after_direct_move_renamed_finishes_forward() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(209, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    for _ in 0..2 {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let outcome =
        run_recoverable_transfer(record, &journal, FileTransferOptions::running(cancellation))
            .await
            .unwrap();
    assert_eq!(outcome.final_target, Some(target.clone()));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"source");
}
