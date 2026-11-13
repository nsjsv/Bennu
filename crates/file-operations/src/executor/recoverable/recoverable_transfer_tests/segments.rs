//! 分段：按文件数和字节数切段；一个文件出错不拦其它段。

use super::*;

fn record_with_files(index: u64, files: &[u64]) -> TransferJournalRecord {
    let identity = |object_kind, size| file_core::FileIdentity {
        device: 1,
        inode: index + 1,
        object_kind,
        size,
        modified_seconds: 0,
        modified_nanoseconds: 0,
        changed_seconds: 0,
        changed_nanoseconds: 0,
        symbolic_link_target: None,
    };
    let root = PathBuf::from(format!("/source-{index}"));
    let mut entries = vec![file_core::SourceManifestEntry {
        relative_path: PathBuf::new(),
        identity: identity(file_core::FileObjectKind::Directory, 0),
    }];
    entries.extend(
        files
            .iter()
            .enumerate()
            .map(|(file, size)| file_core::SourceManifestEntry {
                relative_path: PathBuf::from(format!("file-{file}")),
                identity: identity(file_core::FileObjectKind::RegularFile, *size),
            }),
    );
    TransferJournalRecord {
        task_id: 1,
        key: file_core::TransferWorkKey::top_level(index),
        request: file_core::RecoverableTransferRequest {
            source: root.clone(),
            requested_target: PathBuf::from(format!("/target-{index}")),
            operation: RecoverableTransferOperation::Copy,
            conflict_strategy: TransferConflictStrategy::Fail,
            verification: FileOperationVerification::BasicMetadata,
        },
        checkpoint: file_core::TransferCheckpoint::AwaitingManifest,
        revision: 1,
        manifest: Some(file_core::SourceManifest { root, entries }),
        replacement_manifest: None,
    }
}

fn segment_sizes(records: Vec<TransferJournalRecord>) -> Vec<usize> {
    transfer_segments(records)
        .iter()
        .map(|segment| segment.len())
        .collect()
}

#[test]
fn segments_close_at_the_file_or_byte_limit_and_keep_oversized_records_whole() {
    let single_files = (0..300)
        .map(|index| record_with_files(index, &[1]))
        .collect();
    assert_eq!(segment_sizes(single_files), vec![256, 44]);

    let half_limit = TRANSFER_SEGMENT_BYTES / 2;
    let large_files = (0..5)
        .map(|index| record_with_files(index, &[half_limit]))
        .collect();
    assert_eq!(segment_sizes(large_files), vec![2, 2, 1]);

    let mut mixed = vec![record_with_files(0, &[1; 10])];
    mixed.push(record_with_files(1, &[1; 1_000]));
    mixed.push(record_with_files(2, &[TRANSFER_SEGMENT_BYTES * 3]));
    mixed.push(record_with_files(3, &[1]));
    assert_eq!(segment_sizes(mixed), vec![1, 1, 1, 1]);

    let mut without_manifest = record_with_files(0, &[]);
    without_manifest.manifest = None;
    assert_eq!(segment_sizes(vec![without_manifest]), vec![1]);
}

/// 一个文件出错（读不了），其它文件照常做完，后面的段也照样跑；任务最后按出错的那个失败。
#[tokio::test]
async fn one_failing_file_does_not_stop_the_later_segments() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let sources = directory.path().join("sources");
    let targets = directory.path().join("targets");
    tokio::fs::create_dir(&sources).await.unwrap();
    tokio::fs::create_dir(&targets).await.unwrap();
    let locked = sources.join("locked");
    tokio::fs::create_dir_all(locked.join("inner"))
        .await
        .unwrap();
    std::fs::set_permissions(locked.join("inner"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut transfers = vec![QueuedTransfer::new(locked.clone(), targets.join("locked"))];
    for index in 0..(TRANSFER_SEGMENT_FILES + 4) {
        let name = format!("file-{index:03}");
        tokio::fs::write(sources.join(&name), name.as_bytes())
            .await
            .unwrap();
        transfers.push(QueuedTransfer::new(
            sources.join(&name),
            targets.join(&name),
        ));
    }
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable copy should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, messages) = tokio::sync::mpsc::channel(64);
    let drain = tokio::spawn(drained_events(messages));

    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Copy,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);
    drain.await.unwrap();
    std::fs::set_permissions(locked.join("inner"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let FileOperationCompletion::Failed {
        partial_outcome:
            FileOperationOutcome::Copy {
                transfers: completed,
                ..
            },
        ..
    } = completion
    else {
        panic!("the unreadable source should fail the task");
    };
    assert_eq!(completed.len(), transfers.len() - 1);
    for transfer in &transfers[1..] {
        assert_eq!(
            tokio::fs::read(&transfer.target).await.unwrap(),
            tokio::fs::read(&transfer.source).await.unwrap()
        );
    }
    assert!(tokio::fs::symlink_metadata(targets.join("locked"))
        .await
        .is_err());
}
