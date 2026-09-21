use super::*;

#[tokio::test]
async fn interrupted_staging_payload_is_removed_and_top_level_copy_restarts() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source).await.unwrap();
    fs::write(source.join("complete.txt"), b"complete-content")
        .await
        .unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(99, TransferWorkKey::top_level(0), None);
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
    let TransferCheckpoint::Staging(staging) = &record.checkpoint else {
        panic!("expected staging checkpoint");
    };
    let payload = staging.artifact.plan.payload_path();
    fs::create_dir(&payload).await.unwrap();
    fs::write(payload.join("complete.txt"), b"partial")
        .await
        .unwrap();

    let outcome = run_recoverable_transfer(
        journal.record(request),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.final_target, Some(target.clone()));
    assert_eq!(
        fs::read(target.join("complete.txt")).await.unwrap(),
        b"complete-content"
    );
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn replace_move_failure_restores_hidden_objects_after_target_race() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");
    fs::write(&source, b"new-content").await.unwrap();
    fs::write(&target, b"old-content").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Replace,
    );
    let journal = MemoryJournal::new(607, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    for _ in 0..6 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
            break;
        }
    }
    let TransferCheckpoint::CommitIntent(commit) = &record.checkpoint else {
        panic!("commit intent expected");
    };
    let CommitPayload::Artifact { artifact, .. } = &commit.payload else {
        panic!("replace move artifact expected");
    };
    let artifact_root = artifact.plan.root.clone();
    let payload = artifact.plan.payload_path();
    fs::write(&target, b"external-content").await.unwrap();

    let transfer_result =
        run_recoverable_transfer(record, &journal, running_transfer_options()).await;
    assert!(
        matches!(
            transfer_result,
            Err(RecoverableTransferError::SafeRename { ref from, ref to, .. })
                if from == &payload && to == &target
        ),
        "unexpected transfer result: {transfer_result:?}"
    );

    assert_eq!(fs::read(&source).await.unwrap(), b"new-content");
    assert_eq!(fs::read(&target).await.unwrap(), b"external-content");
    let recovered = rename::recovered_name_candidate(&target, 1);
    assert_eq!(fs::read(recovered).await.unwrap(), b"old-content");
    assert!(fs::symlink_metadata(artifact_root).await.is_err());
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::Failed { .. }
    ));
}

#[tokio::test]
async fn source_change_records_failed_checkpoint_after_owned_cleanup() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"original").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(100, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    for _ in 0..2 {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    assert!(matches!(record.checkpoint, TransferCheckpoint::Staging(_)));
    fs::write(&source, b"changed-after-manifest").await.unwrap();

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::SourceChanged { .. })
    ));
    assert!(matches!(
        journal.state.lock().unwrap().checkpoint,
        TransferCheckpoint::Failed { .. }
    ));
    assert!(fs::symlink_metadata(&target).await.is_err());
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn tampered_marker_blocks_failure_cleanup_and_preserves_journal() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"original").await.unwrap();
    let request = transfer_request(
        source,
        target,
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(101, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    for _ in 0..2 {
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap();
    }
    let TransferCheckpoint::Staging(staging) = &record.checkpoint else {
        panic!("expected staging checkpoint");
    };
    fs::write(staging.artifact.plan.owner_path(), b"external-owner")
        .await
        .unwrap();

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert!(matches!(
        journal.state.lock().unwrap().checkpoint,
        TransferCheckpoint::FailureIntent(_)
    ));
    assert!(fs::symlink_metadata(&staging.artifact.plan.root)
        .await
        .is_ok());
}

#[tokio::test]
async fn tampered_marker_blocks_cancellation_cleanup() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");
    fs::write(&source, b"content").await.unwrap();
    let request = transfer_request(
        source,
        target,
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(603, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    for _ in 0..4 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if matches!(record.checkpoint, TransferCheckpoint::Staging(_)) {
            break;
        }
    }
    let TransferCheckpoint::Staging(staging) = &record.checkpoint else {
        panic!("staging checkpoint expected");
    };
    let artifact_plan = staging.artifact.plan.clone();
    fs::write(artifact_plan.owner_path(), b"tampered")
        .await
        .unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, canceled_transfer_options()).await,
        Err(RecoverableTransferError::RecoveryBlocked { .. })
    ));
    assert!(matches!(
        journal.record(request).checkpoint,
        TransferCheckpoint::CancelIntent(_)
    ));
    assert!(artifact_plan.root.exists());
}

#[tokio::test]
async fn commit_intent_rejects_equal_content_from_different_object() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");
    fs::write(&source, b"same-content").await.unwrap();
    let request = transfer_request(
        source,
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(604, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);
    for _ in 0..6 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
            break;
        }
    }
    let TransferCheckpoint::CommitIntent(commit) = &record.checkpoint else {
        panic!("commit intent expected");
    };
    let CommitPayload::Artifact { artifact, .. } = &commit.payload else {
        panic!("copy commit must use an artifact");
    };
    fs::remove_file(artifact.plan.payload_path()).await.unwrap();
    fs::write(&target, b"same-content").await.unwrap();

    assert!(matches!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options()).await,
        Err(RecoverableTransferError::TargetConflict { path }) if path == target
    ));
    assert_eq!(fs::read(&target).await.unwrap(), b"same-content");
}

#[tokio::test]
async fn copy_recovers_after_every_journal_boundary() {
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
            100 + failed_attempt as u64,
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
        let outcome = run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_target, Some(target.clone()));
        assert_eq!(fs::read(&source).await.unwrap(), b"copy-content");
        assert_eq!(fs::read(&target).await.unwrap(), b"copy-content");
        assert_no_transfer_artifacts(directory.path());
    }
}

#[tokio::test]
async fn basic_direct_move_persists_target_visibility_before_content_hash() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"move-content").await.unwrap();
    let source_identity = inspect_file_identity(&source).await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(199, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert!(fs::symlink_metadata(&source).await.is_ok());
    assert!(fs::symlink_metadata(&target).await.is_err());

    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    let TransferCheckpoint::DirectMoveRenamed(renamed) = &record.checkpoint else {
        panic!("durable direct-move rename fact expected");
    };
    assert!(renamed.target_identity.same_object(&source_identity));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"move-content");

    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::TargetCommitted(_)
    ));
}

#[tokio::test]
async fn basic_direct_move_recovers_when_renamed_checkpoint_write_fails() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"move-content").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(200, TransferWorkKey::top_level(0), Some(2));

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::Journal { .. })
    ));
    assert!(matches!(
        journal.record(request.clone()).checkpoint,
        TransferCheckpoint::DirectMoveIntent(_)
    ));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"move-content");

    journal.set_failure(None);
    let outcome = run_recoverable_transfer(
        journal.record(request),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.final_target, Some(target.clone()));
    assert_eq!(fs::read(&target).await.unwrap(), b"move-content");
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn basic_direct_move_recovers_when_target_committed_write_fails() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"move-content").await.unwrap();
    let request = basic_transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(201, TransferWorkKey::top_level(0), Some(3));

    assert!(matches!(
        run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options()
        )
        .await,
        Err(RecoverableTransferError::Journal { .. })
    ));
    assert!(matches!(
        journal.record(request.clone()).checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
    ));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"move-content");

    journal.set_failure(None);
    let outcome = run_recoverable_transfer(
        journal.record(request),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.final_target, Some(target.clone()));
    assert_eq!(fs::read(&target).await.unwrap(), b"move-content");
    assert_no_transfer_artifacts(directory.path());
}

#[tokio::test]
async fn basic_and_strong_move_recover_after_every_journal_boundary() {
    for verification in [
        FileOperationVerification::BasicMetadata,
        FileOperationVerification::Strong,
    ] {
        for failed_attempt in 1.. {
            let directory = tempdir().unwrap();
            let source = directory.path().join("source");
            let target = directory.path().join("target");
            fs::write(&source, b"move-content").await.unwrap();
            let mut request = transfer_request(
                source.clone(),
                target.clone(),
                RecoverableTransferOperation::Move,
                TransferConflictStrategy::Fail,
            );
            request.verification = verification;
            let journal = MemoryJournal::new(
                200 + failed_attempt as u64,
                TransferWorkKey::top_level(0),
                Some(failed_attempt),
            );

            let first_run = run_recoverable_transfer(
                journal.record(request.clone()),
                &journal,
                running_transfer_options(),
            )
            .await;
            let hit_journal_boundary = match first_run {
                Err(RecoverableTransferError::Journal { .. }) => true,
                Ok(_) => false,
                Err(error) => panic!("unexpected transfer error: {error:?}"),
            };
            if hit_journal_boundary {
                run_recoverable_transfer(
                    journal.record(request.clone()),
                    &journal,
                    running_transfer_options(),
                )
                .await
                .unwrap();
            }

            assert!(fs::symlink_metadata(&source).await.is_err());
            assert_eq!(fs::read(&target).await.unwrap(), b"move-content");
            assert_no_transfer_artifacts(directory.path());
            if !hit_journal_boundary {
                break;
            }
        }
    }
}

#[tokio::test]
async fn replace_copy_recovers_old_target_backup_after_every_boundary() {
    for failed_attempt in 1.. {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::write(&source, b"replacement").await.unwrap();
        fs::write(&target, b"old-target").await.unwrap();
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Copy,
            TransferConflictStrategy::Replace,
        );
        let journal = MemoryJournal::new(
            300 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        let first_run = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await;
        let hit_journal_boundary = match first_run {
            Err(RecoverableTransferError::Journal { .. }) => true,
            Ok(_) => false,
            Err(error) => panic!("unexpected transfer error: {error:?}"),
        };
        if hit_journal_boundary {
            run_recoverable_transfer(
                journal.record(request.clone()),
                &journal,
                running_transfer_options(),
            )
            .await
            .unwrap();
        }

        assert_eq!(fs::read(&source).await.unwrap(), b"replacement");
        assert_eq!(fs::read(&target).await.unwrap(), b"replacement");
        assert_no_transfer_artifacts(directory.path());
        if !hit_journal_boundary {
            break;
        }
    }
}

#[tokio::test]
async fn replace_move_restores_hidden_source_or_finishes_after_failures() {
    for failed_attempt in 1.. {
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
            400 + failed_attempt as u64,
            TransferWorkKey::top_level(0),
            Some(failed_attempt),
        );

        let first_run = run_recoverable_transfer(
            journal.record(request.clone()),
            &journal,
            running_transfer_options(),
        )
        .await;
        let hit_journal_boundary = match first_run {
            Err(RecoverableTransferError::Journal { .. }) => true,
            Ok(_) => false,
            Err(error) => panic!("unexpected transfer error: {error:?}"),
        };
        if hit_journal_boundary {
            run_recoverable_transfer(
                journal.record(request.clone()),
                &journal,
                running_transfer_options(),
            )
            .await
            .unwrap();
        }

        assert!(fs::symlink_metadata(&source).await.is_err());
        assert_eq!(fs::read(&target).await.unwrap(), b"replacement");
        assert_no_transfer_artifacts(directory.path());
        if !hit_journal_boundary {
            break;
        }
    }
}

#[tokio::test]
async fn merge_rejects_replaced_target_root_before_child_side_effects() {
    let directory = tempdir().unwrap();
    let (source, target) =
        super::merge_and_control_cases::populate_merge_directories(directory.path()).await;
    let replaced_target = directory.path().join("replaced-target");
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Merge,
    );
    let journal = MemoryJournal::new(599, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    assert_eq!(
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
            .await
            .unwrap(),
        TransferAdvance::Continue
    );
    fs::rename(&target, &replaced_target).await.unwrap();
    fs::create_dir(&target).await.unwrap();

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::TargetConflict { path }) if path == target
    ));
    assert!(fs::symlink_metadata(target.join("new.txt")).await.is_err());
    assert_eq!(
        fs::read(source.join("new.txt")).await.unwrap(),
        b"new-content"
    );
    assert_eq!(
        fs::read(replaced_target.join("skip.txt")).await.unwrap(),
        b"target-collision"
    );
}

#[tokio::test]
async fn merge_recovery_rejects_external_replacement_of_committed_child() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source).await.unwrap();
    fs::create_dir(&target).await.unwrap();
    fs::write(source.join("a.txt"), b"first").await.unwrap();
    fs::write(source.join("b.txt"), b"second").await.unwrap();
    let request = transfer_request(
        source,
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Merge,
    );
    let journal = MemoryJournal::new(601, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    for _ in 0..16 {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if matches!(
            &record.checkpoint,
            TransferCheckpoint::Merging(merge) if merge.next_child == 1
        ) {
            break;
        }
    }
    let TransferCheckpoint::Merging(merge) = &record.checkpoint else {
        panic!("merge checkpoint expected");
    };
    assert_eq!(merge.next_child, 1);

    fs::remove_file(target.join("a.txt")).await.unwrap();
    fs::write(target.join("a.txt"), b"external").await.unwrap();
    let TransferCheckpoint::Merging(merge) = &mut record.checkpoint else {
        unreachable!();
    };
    merge.completed_prefix_verified = false;

    assert!(matches!(
        run_recoverable_transfer(record, &journal, running_transfer_options()).await,
        Err(RecoverableTransferError::TargetConflict { path })
            if path == target.join("a.txt")
    ));
    assert_eq!(fs::read(target.join("a.txt")).await.unwrap(), b"external");
    assert!(fs::symlink_metadata(target.join("b.txt")).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_after_source_retirement_intent_finishes_safe_boundary() {
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
    fs::write(&source, b"retirement-content").await.unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(600, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);
    for _ in 0..12 {
        if matches!(
            record.checkpoint,
            TransferCheckpoint::SourceRetirementIntent(_)
        ) {
            break;
        }
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
    }
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::SourceRetirementIntent(_)
    ));

    let outcome = run_recoverable_transfer(record, &journal, canceled_transfer_options())
        .await
        .unwrap();

    assert_eq!(outcome.final_target, Some(target.clone()));
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(fs::read(&target).await.unwrap(), b"retirement-content");
    assert_no_transfer_artifacts(source_directory.path());
    assert_no_transfer_artifacts(target_directory.path());
}

#[cfg(unix)]
#[tokio::test]
async fn cross_filesystem_move_recovers_target_commit_and_source_retirement() {
    use std::os::unix::fs::MetadataExt;

    for failed_attempt in 1..=10 {
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
        let request = transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Fail,
        );
        let journal = MemoryJournal::new(
            500 + failed_attempt as u64,
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
        run_recoverable_transfer(
            journal.record(request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();

        assert!(fs::symlink_metadata(&source).await.is_err());
        assert_eq!(fs::read(&target).await.unwrap(), b"cross-device");
        assert_no_transfer_artifacts(source_directory.path());
        assert_no_transfer_artifacts(target_directory.path());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn source_retirement_resumes_after_entry_delete_checkpoint_failure() {
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
    fs::create_dir(source.join("nested")).await.unwrap();
    fs::write(source.join("nested/file"), b"retire-me")
        .await
        .unwrap();
    let request = transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(700, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request.clone());
    for _ in 0..24 {
        if matches!(
            &record.checkpoint,
            TransferCheckpoint::SourceRetired(retired)
                if retired.cleanup_intent.as_ref().is_some_and(|intent| {
                    intent.entry.relative_path.as_path() == Path::new("nested/file")
                })
        ) {
            break;
        }
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
    }
    let TransferCheckpoint::SourceRetired(retired) = &record.checkpoint else {
        panic!("source retirement cleanup intent expected");
    };
    let retired_file = retired.artifact.plan.payload_path().join("nested/file");
    let deletion_slot = retired.artifact.plan.backup_path();
    journal.set_failure(Some(journal.attempt_count() + 1));

    let interrupted =
        advance_recoverable_transfer(&mut record, &journal, &running_transfer_options()).await;
    assert!(
        matches!(interrupted, Err(RecoverableTransferError::Journal { .. })),
        "unexpected cleanup outcome: {interrupted:?}"
    );
    assert!(fs::symlink_metadata(&retired_file).await.is_err());
    assert!(fs::symlink_metadata(&deletion_slot).await.is_ok());

    journal.set_failure(None);
    run_recoverable_transfer(
        journal.record(request),
        &journal,
        running_transfer_options(),
    )
    .await
    .unwrap();
    assert!(fs::symlink_metadata(&source).await.is_err());
    assert_eq!(
        fs::read(target.join("nested/file")).await.unwrap(),
        b"retire-me"
    );
    assert_no_transfer_artifacts(source_directory.path());
}

#[cfg(unix)]
#[tokio::test]
async fn source_retirement_never_recursively_deletes_unmanifested_entry() {
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
    fs::create_dir(source.join("nested")).await.unwrap();
    fs::write(source.join("nested/file"), b"manifested")
        .await
        .unwrap();
    let request = transfer_request(
        source,
        target.clone(),
        RecoverableTransferOperation::Move,
        TransferConflictStrategy::Fail,
    );
    let journal = MemoryJournal::new(701, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);
    let retirement_payload = loop {
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        if let TransferCheckpoint::SourceRetired(retired) = &record.checkpoint {
            break retired.artifact.plan.payload_path();
        }
    };
    let unmanifested = retirement_payload.join("nested/late");
    fs::write(&unmanifested, b"must-survive").await.unwrap();

    let error = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RecoverableTransferError::RecoveryBlocked { .. }
    ));
    assert_eq!(fs::read(&unmanifested).await.unwrap(), b"must-survive");
    assert_eq!(
        fs::read(target.join("nested/file")).await.unwrap(),
        b"manifested"
    );
}
