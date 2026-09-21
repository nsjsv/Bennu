use super::*;

async fn commit_intent_after_staging(
    journal: &MemoryJournal,
    request: &RecoverableTransferRequest,
) -> TransferJournalRecord {
    let mut record = journal.record(request.clone());
    loop {
        if matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
            return record;
        }
        assert!(matches!(
            advance_recoverable_transfer(&mut record, journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        ));
        record = journal.record(request.clone());
    }
}

fn commit_artifact(record: &TransferJournalRecord) -> OwnedArtifact {
    let TransferCheckpoint::CommitIntent(CommitTransfer {
        payload: CommitPayload::Artifact { artifact, .. },
        ..
    }) = &record.checkpoint
    else {
        panic!("expected artifact commit intent");
    };
    artifact.clone()
}

#[tokio::test]
async fn canceled_copy_does_not_delete_modified_commit_payload() {
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
    let journal = MemoryJournal::new(91, TransferWorkKey::top_level(0), None);
    let record = commit_intent_after_staging(&journal, &request).await;
    let artifact = commit_artifact(&record);
    let payload = artifact.plan.payload_path();
    fs::write(&payload, b"externally-modified").await.unwrap();

    let error = run_recoverable_transfer(record, &journal, canceled_transfer_options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RecoverableTransferError::RecoveryBlocked { .. }
    ));
    assert_eq!(fs::read(&payload).await.unwrap(), b"externally-modified");
    assert_eq!(fs::read(&source).await.unwrap(), b"source");
    assert!(!target.exists());
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::CancelIntent(_)
    ));
}

#[tokio::test]
async fn canceled_move_does_not_restore_modified_commit_payload_as_source() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"source").await.unwrap();
    fs::write(&target, b"previous").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Replace,
    );
    let journal = MemoryJournal::new(92, TransferWorkKey::top_level(0), None);
    let record = commit_intent_after_staging(&journal, &request).await;
    let artifact = commit_artifact(&record);
    let payload = artifact.plan.payload_path();
    assert!(!source.exists());
    fs::write(&payload, b"externally-modified").await.unwrap();

    let error = run_recoverable_transfer(record, &journal, canceled_transfer_options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RecoverableTransferError::RecoveryBlocked { .. }
    ));
    assert!(!source.exists());
    assert_eq!(fs::read(&payload).await.unwrap(), b"externally-modified");
    assert_eq!(fs::read(&target).await.unwrap(), b"previous");
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::CancelIntent(_)
    ));
}
