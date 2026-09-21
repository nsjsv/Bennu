use super::*;

#[tokio::test]
async fn identical_source_and_target_records_skip_for_non_keep_both_strategy() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("same");
    fs::write(&path, b"content").await.unwrap();
    let request = transfer_request(
        path.clone(),
        path.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(68, TransferWorkKey::top_level(0), None);

    let outcome = run_recoverable_transfer(
        journal.record(request.clone()),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();
    assert!(outcome.final_target.is_none());
    assert_eq!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Skipped
    );
    assert_eq!(fs::read(&path).await.unwrap(), b"content");
}

#[tokio::test]
async fn fail_conflict_preserves_source_and_existing_target() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&target, b"existing").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(69, TransferWorkKey::top_level(0), None);

    let error = run_recoverable_transfer(
        journal.record(request.clone()),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, RecoverableTransferError::TargetConflict { path } if path == target));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Failed { .. }
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert_eq!(fs::read(&target).await.unwrap(), b"existing");
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn skip_conflict_records_terminal_skip_without_side_effects() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&target, b"existing").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Skip,
    );
    let journal = MemoryJournal::new(70, TransferWorkKey::top_level(0), None);

    let outcome = run_recoverable_transfer(
        journal.record(request.clone()),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();
    assert!(outcome.final_target.is_none());
    assert_eq!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Skipped
    );
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert_eq!(fs::read(&target).await.unwrap(), b"existing");
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn keep_both_reselects_persisted_candidate_when_concurrently_occupied() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"replacement").await.unwrap();
    fs::write(&target, b"existing").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::KeepBoth,
    );
    let journal = MemoryJournal::new(71, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());

    for _ in 0..3 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        record = journal.record(request.clone());
    }
    let TransferCheckpoint::CommitIntent(commit) = &record.checkpoint else {
        panic!("expected commit intent");
    };
    let occupied_candidate = commit.prepared.resolved_target.clone();
    assert_ne!(occupied_candidate, target);
    fs::write(&occupied_candidate, b"concurrent").await.unwrap();

    let outcome = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap();
    let final_target = outcome.final_target.unwrap();
    assert_ne!(final_target, target);
    assert_ne!(final_target, occupied_candidate);
    assert_eq!(fs::read(&target).await.unwrap(), b"existing");
    assert_eq!(fs::read(&occupied_candidate).await.unwrap(), b"concurrent");
    assert_eq!(fs::read(&final_target).await.unwrap(), b"replacement");
    assert_no_transfer_artifacts(directory.path());
}
