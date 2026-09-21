use super::*;

#[tokio::test]
async fn missing_fingerprint_fields_do_not_decode_as_basic_verification() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"content").await.unwrap();
    let request = transfer_request(
        source,
        target,
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(1_600, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
        .await
        .unwrap();
    let mut missing_preflight = serde_json::to_value(&record.checkpoint).unwrap();
    missing_preflight
        .get_mut("fields")
        .and_then(serde_json::Value::as_object_mut)
        .unwrap()
        .remove("source_fingerprint");
    assert!(serde_json::from_value::<TransferCheckpoint>(missing_preflight).is_err());

    while !matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    let mut missing_commit = serde_json::to_value(&record.checkpoint).unwrap();
    missing_commit
        .get_mut("fields")
        .and_then(serde_json::Value::as_object_mut)
        .unwrap()
        .remove("fingerprint");
    assert!(serde_json::from_value::<TransferCheckpoint>(missing_commit).is_err());
}

#[tokio::test]
async fn copy_cannot_execute_move_direct_from_corrupted_checkpoint() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"content").await.unwrap();
    let source_identity = inspect_file_identity(&source).await.unwrap();
    let source_fingerprint = fingerprint_object(&source).await.unwrap();
    let manifest = build_source_manifest(&source).await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let key = TransferWorkKey::top_level(0);
    let journal = MemoryJournal::new(1_601, key.clone(), None);
    let corrupted = TransferCheckpoint::CommitIntent(CommitTransfer {
        prepared: PreparedTransfer {
            source_identity: source_identity.clone(),
            resolved_target: target.clone(),
            expected_target_identity: None,
            expected_target_fingerprint: None,
            source_fingerprint: Some(source_fingerprint),
            execution: TransferExecutionKind::MoveDirect,
            staging_plan: None,
        },
        payload: CommitPayload::DirectSource {
            identity: source_identity,
        },
        fingerprint: TransferFingerprint::Blake3(source_fingerprint),
        backup_identity: None,
    });
    journal
        .commit(TransferJournalMutation::InstallManifestAndCheckpoint {
            task_id: 1_601,
            key,
            expected_revision: 0,
            manifest,
            replacement_manifest: None,
            checkpoint: corrupted.clone(),
        })
        .await
        .unwrap();

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert_eq!(journal.record(request).checkpoint, corrupted);
    assert_eq!(fs::read(&source).await.unwrap(), b"content");
    assert!(fs::symlink_metadata(&target).await.is_err());
}

#[tokio::test]
async fn semantic_checkpoint_corruption_blocks_without_cleanup_mutation() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"content").await.unwrap();
    let source_identity = inspect_file_identity(&source).await.unwrap();
    let source_fingerprint = fingerprint_object(&source).await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let key = TransferWorkKey::top_level(0);
    let journal = MemoryJournal::new(1_600, key.clone(), None);
    let corrupted = TransferCheckpoint::StageCreationIntent(PreparedTransfer {
        source_identity,
        resolved_target: target.clone(),
        expected_target_identity: None,
        expected_target_fingerprint: None,
        source_fingerprint: Some(source_fingerprint),
        execution: TransferExecutionKind::CopyToStage,
        staging_plan: None,
    });
    journal
        .commit(TransferJournalMutation::CompareAndSwapCheckpoint {
            task_id: 1_600,
            key,
            expected_revision: 0,
            checkpoint: corrupted.clone(),
        })
        .await
        .unwrap();

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert_eq!(journal.record(request).checkpoint, corrupted);
    assert_eq!(fs::read(&source).await.unwrap(), b"content");
    assert!(fs::symlink_metadata(&target).await.is_err());
}
