use super::*;

#[cfg(unix)]
#[tokio::test]
async fn source_retirement_rejects_payload_modified_after_deletion_intent() {
    use std::os::unix::fs::MetadataExt;

    for (task_id, verification) in [
        (702, FileOperationVerification::BasicMetadata),
        (704, FileOperationVerification::Strong),
    ] {
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
        fs::create_dir(&source).await.unwrap();
        fs::write(source.join("file"), b"original").await.unwrap();
        let mut request = transfer_request(
            source,
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Fail,
        );
        request.verification = verification;
        let journal = MemoryJournal::new(task_id, TransferWorkKey::top_level(0), None);
        let mut record = journal.record(request);

        let (retired_entry, deletion_slot) = loop {
            assert_eq!(
                advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                    .await
                    .unwrap(),
                TransferAdvance::Continue
            );
            if let TransferCheckpoint::SourceRetired(retired) = &record.checkpoint {
                if retired
                    .cleanup_intent
                    .as_ref()
                    .is_some_and(|intent| intent.entry.relative_path.as_path() == Path::new("file"))
                {
                    break (
                        retired.artifact.plan.payload_path().join("file"),
                        retired.artifact.plan.backup_path(),
                    );
                }
            }
        };
        rename_noreplace(&retired_entry, &deletion_slot).unwrap();
        fs::write(&deletion_slot, b"tampered").await.unwrap();

        let error = run_recoverable_transfer(record, &journal, running_transfer_options())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            RecoverableTransferError::RecoveryBlocked { .. }
        ));
        assert_eq!(fs::read(&retired_entry).await.unwrap(), b"tampered");
        assert!(fs::symlink_metadata(&deletion_slot).await.is_err());
        assert_eq!(fs::read(target.join("file")).await.unwrap(), b"original");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn source_retirement_keeps_replaced_deletion_slot_hidden() {
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
    fs::create_dir(&source).await.unwrap();
    fs::write(source.join("file"), b"original").await.unwrap();
    let request = transfer_request(
        source,
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(703, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    let (retired_entry, deletion_slot) = loop {
        assert!(matches!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        ));
        if let TransferCheckpoint::SourceRetired(retired) = &record.checkpoint {
            if retired
                .cleanup_intent
                .as_ref()
                .is_some_and(|intent| intent.entry.relative_path.as_path() == Path::new("file"))
            {
                break (
                    retired.artifact.plan.payload_path().join("file"),
                    retired.artifact.plan.backup_path(),
                );
            }
        }
    };
    rename_noreplace(&retired_entry, &deletion_slot).unwrap();
    fs::remove_file(&deletion_slot).await.unwrap();
    fs::write(&deletion_slot, b"foreign-object").await.unwrap();

    let error = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RecoverableTransferError::RecoveryBlocked { .. }
    ));
    assert!(!retired_entry.exists());
    assert_eq!(fs::read(&deletion_slot).await.unwrap(), b"foreign-object");
    assert_eq!(fs::read(target.join("file")).await.unwrap(), b"original");
}

#[cfg(unix)]
#[tokio::test]
async fn source_retirement_rejects_replaced_identified_artifact_root() {
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
    fs::write(&source, b"original").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(703, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    let identified_artifact = loop {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if let TransferCheckpoint::SourceRetirementIntent(retirement) = &record.checkpoint {
            if let Some(artifact) = retirement.artifact.as_ref() {
                break artifact.clone();
            }
        }
    };
    let marker_bytes = fs::read(identified_artifact.plan.owner_path())
        .await
        .unwrap();
    fs::remove_file(identified_artifact.plan.owner_path())
        .await
        .unwrap();
    fs::remove_dir(&identified_artifact.plan.root)
        .await
        .unwrap();
    fs::create_dir(&identified_artifact.plan.root)
        .await
        .unwrap();
    fs::write(identified_artifact.plan.owner_path(), marker_bytes)
        .await
        .unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"original");
    assert_eq!(fs::read(&target).await.unwrap(), b"original");
}
