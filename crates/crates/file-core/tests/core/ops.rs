use super::*;

#[tokio::test]
async fn create_directory_and_rename_path_update_filesystem() {
    let dir = tempdir().unwrap();
    let folder = dir.path().join("folder");

    let created = create_directory(&folder).await.unwrap();
    assert!(created.is_dir());

    let renamed = rename_path(&folder, "renamed").await.unwrap();
    assert_eq!(renamed, dir.path().join("renamed"));
    assert!(renamed.is_dir());
    assert!(!folder.exists());
}

#[tokio::test]
async fn batch_rename_paths_updates_all_items() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let alpha = dir.path().join("alpha.txt");
    let beta = dir.path().join("beta.txt");
    fs::write(&first, b"one").unwrap();
    fs::write(&second, b"two").unwrap();

    let completed = batch_rename_paths(vec![
        BatchRenameItem {
            from: first.clone(),
            to: alpha.clone(),
        },
        BatchRenameItem {
            from: second.clone(),
            to: beta.clone(),
        },
    ])
    .await
    .unwrap();

    assert_eq!(completed.len(), 2);
    assert_eq!(fs::read(&alpha).unwrap(), b"one");
    assert_eq!(fs::read(&beta).unwrap(), b"two");
    assert!(!first.exists());
    assert!(!second.exists());
}

#[tokio::test]
async fn batch_rename_paths_rejects_duplicate_targets() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let target = dir.path().join("target.txt");
    fs::write(&first, b"one").unwrap();
    fs::write(&second, b"two").unwrap();

    let error = batch_rename_paths(vec![
        BatchRenameItem {
            from: first.clone(),
            to: target.clone(),
        },
        BatchRenameItem {
            from: second.clone(),
            to: target.clone(),
        },
    ])
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, target);
            assert_eq!(message, "target appears more than once");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(fs::read(&first).unwrap(), b"one");
    assert_eq!(fs::read(&second).unwrap(), b"two");
}

#[tokio::test]
async fn batch_rename_paths_rejects_existing_unrelated_target() {
    let dir = tempdir().unwrap();
    let source_path = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    fs::write(&source_path, b"source").unwrap();
    fs::write(&target, b"target").unwrap();

    let error = batch_rename_paths(vec![BatchRenameItem {
        from: source_path.clone(),
        to: target.clone(),
    }])
    .await
    .unwrap_err();

    match error {
        FileError::Rename { from, to, source } => {
            assert_eq!(from, source_path);
            assert_eq!(to, target);
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn batch_rename_paths_allows_swapping_names() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    fs::write(&first, b"one").unwrap();
    fs::write(&second, b"two").unwrap();

    batch_rename_paths(vec![
        BatchRenameItem {
            from: first.clone(),
            to: second.clone(),
        },
        BatchRenameItem {
            from: second.clone(),
            to: first.clone(),
        },
    ])
    .await
    .unwrap();

    assert_eq!(fs::read(&first).unwrap(), b"two");
    assert_eq!(fs::read(&second).unwrap(), b"one");
}

#[tokio::test]
async fn create_empty_file_writes_zero_length_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("empty.txt");

    let created = create_empty_file(&file).await.unwrap();

    assert_eq!(created, file);
    assert!(created.is_file());
    assert_eq!(fs::metadata(&created).unwrap().len(), 0);
}

#[tokio::test]
async fn create_file_with_contents_writes_exact_bytes() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("clipboard.bin");

    let created = create_file_with_contents(&file, b"clipboard bytes")
        .await
        .unwrap();

    assert_eq!(created, file);
    assert_eq!(fs::read(&created).unwrap(), b"clipboard bytes");
}

#[tokio::test]
async fn delete_path_permanently_removes_files_and_directories() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("remove.txt");
    let folder = dir.path().join("folder");
    fs::write(&file, b"delete").unwrap();
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("child.txt"), b"delete").unwrap();

    delete_path_permanently(&file).await.unwrap();
    delete_path_permanently(&folder).await.unwrap();

    assert!(!file.exists());
    assert!(!folder.exists());
}

#[tokio::test]
async fn delete_path_permanently_missing_path_returns_structured_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing.txt");

    let error = delete_path_permanently(&path).await.unwrap_err();

    match error {
        FileError::Delete {
            path: error_path,
            source,
        } => {
            assert_eq!(error_path, path);
            assert_eq!(source.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn create_empty_file_existing_path_returns_structured_error() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    fs::write(&file, b"taken").unwrap();

    let error = create_empty_file(&file).await.unwrap_err();

    match error {
        FileError::CreateFile { path, source } => {
            assert_eq!(path, file);
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn copy_and_move_file_operations_update_filesystem() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let copied = dir.path().join("copied.txt");
    let moved = dir.path().join("moved.txt");
    fs::write(&source, b"copy me").unwrap();

    copy_path(
        &source,
        &copied,
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(&copied).unwrap(), b"copy me");

    move_path(
        &copied,
        &moved,
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert!(!copied.exists());
    assert_eq!(fs::read(&moved).unwrap(), b"copy me");
}

#[tokio::test]
async fn strong_copy_verification_accepts_hash_matched_target() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let copied = dir.path().join("copied.bin");
    let mut contents = vec![0x5a; 1024 * 1024 + 17];
    contents[1024 * 1024] = 0xa5;
    fs::write(&source, &contents).unwrap();

    copy_path_with_options(
        &source,
        &copied,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_verification(FileOperationVerification::Strong),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(&copied).unwrap(), contents);
}

#[tokio::test]
async fn copy_conflict_replace_overwrites_existing_file() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();

    copy_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Replace),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[tokio::test]
async fn copy_conflict_skip_preserves_existing_file() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();

    copy_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Skip),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"old");
}

#[tokio::test]
async fn copy_conflict_keep_both_writes_alternate_path() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    let alternate = dir.path().join("target 2.txt");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();

    copy_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"old");
    assert_eq!(fs::read(&alternate).unwrap(), b"new");
}

#[tokio::test]
async fn copy_conflict_merge_directory_adds_missing_children() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("same.txt"), b"new").unwrap();
    fs::write(source.join("new.txt"), b"added").unwrap();
    fs::write(target.join("same.txt"), b"old").unwrap();

    copy_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Merge),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(target.join("same.txt")).unwrap(), b"old");
    assert_eq!(fs::read(target.join("new.txt")).unwrap(), b"added");
}

#[tokio::test]
async fn move_conflict_keep_both_moves_to_alternate_path() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    let alternate = dir.path().join("target 2.txt");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();

    move_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
    )
    .await
    .unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read(&target).unwrap(), b"old");
    assert_eq!(fs::read(&alternate).unwrap(), b"new");
}

#[tokio::test]
async fn move_conflict_replace_collapses_single_child_directory_into_target() {
    let dir = tempdir().unwrap();
    let outer = dir.path().join("nest");
    let inner = outer.join("nest");
    let source = inner.join("nest");
    let payload = source.join("payload.txt");
    let nested_dir = source.join("child");
    let nested_file = nested_dir.join("nested.txt");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(&payload, b"payload").unwrap();
    fs::write(&nested_file, b"nested").unwrap();

    let moved_target = move_path_with_options(
        &source,
        &inner,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Replace),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(moved_target, inner);
    assert!(!source.exists());
    assert_eq!(fs::read(inner.join("payload.txt")).unwrap(), b"payload");
    assert_eq!(
        fs::read(inner.join("child").join("nested.txt")).unwrap(),
        b"nested"
    );
}

#[tokio::test]
async fn copy_conflict_replace_rejects_target_directory_containing_source_path() {
    let dir = tempdir().unwrap();
    let outer = dir.path().join("nest");
    let inner = outer.join("nest");
    let source = inner.join("nest");
    let payload = source.join("payload.txt");
    fs::create_dir_all(&source).unwrap();
    fs::write(&payload, b"payload").unwrap();

    let error = copy_path_with_options(
        &source,
        &inner,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Replace),
    )
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, inner);
            assert_eq!(
                message,
                "cannot replace a target directory that contains the source path"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(inner.is_dir());
    assert_eq!(fs::read(&payload).unwrap(), b"payload");
}

#[tokio::test]
async fn move_conflict_replace_rejects_target_directory_containing_source_path() {
    let dir = tempdir().unwrap();
    let outer = dir.path().join("nest");
    let inner = outer.join("nest");
    let source = inner.join("nest");
    let payload = source.join("payload.txt");
    let hidden = inner.join(".hidden");
    fs::create_dir_all(&source).unwrap();
    fs::write(&payload, b"payload").unwrap();
    fs::write(&hidden, b"keep").unwrap();

    let error = move_path_with_options(
        &source,
        &inner,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Replace),
    )
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, inner);
            assert_eq!(
                message,
                "cannot replace a target directory that contains the source path"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(inner.is_dir());
    assert_eq!(fs::read(&payload).unwrap(), b"payload");
    assert_eq!(fs::read(&hidden).unwrap(), b"keep");
}

#[tokio::test]
async fn move_conflict_replace_rejects_nested_child_with_different_name() {
    let dir = tempdir().unwrap();
    let outer = dir.path().join("nest");
    let inner = outer.join("nest");
    let source = inner.join("child");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload.txt"), b"payload").unwrap();

    let error = move_path_with_options(
        &source,
        &inner,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Replace),
    )
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, inner);
            assert_eq!(
                message,
                "cannot replace a target directory that contains the source path"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(inner.is_dir());
    assert_eq!(fs::read(source.join("payload.txt")).unwrap(), b"payload");
}

#[tokio::test]
async fn transfer_conflict_check_ignores_missing_target() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    fs::write(&source, b"source").unwrap();

    let conflicts =
        check_transfer_conflicts(vec![TransferConflictCheck::new(source, target)]).await;

    assert!(conflicts.is_empty());
}

#[tokio::test]
async fn transfer_conflict_check_detects_file_conflict() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    fs::write(&source, b"new bytes").unwrap();
    fs::write(&target, b"old").unwrap();

    let conflicts = check_transfer_conflicts(vec![TransferConflictCheck::new(
        source.clone(),
        target.clone(),
    )])
    .await;

    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0];
    assert_eq!(conflict.source, source);
    assert_eq!(conflict.target, target);
    assert!(!conflict.can_merge());
    assert!(!conflict.source_metadata.is_directory);
    assert!(!conflict.target_metadata.is_directory);
    assert_eq!(conflict.source_metadata.len, 9);
    assert_eq!(conflict.target_metadata.len, 3);
}

#[tokio::test]
async fn transfer_conflict_check_marks_directories_mergeable() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();

    let conflicts =
        check_transfer_conflicts(vec![TransferConflictCheck::new(source, target)]).await;

    assert_eq!(conflicts.len(), 1);
    assert!(conflicts[0].can_merge());
    assert!(conflicts[0].source_metadata.is_directory);
    assert!(conflicts[0].target_metadata.is_directory);
}

#[tokio::test]
async fn transfer_target_availability_and_candidate_number_before_extension() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("target.txt");
    let numbered_2 = dir.path().join("target 2.txt");
    let numbered_3 = dir.path().join("target 3.txt");
    fs::write(&target, b"old").unwrap();
    fs::write(&numbered_2, b"old copy").unwrap();

    assert!(!is_transfer_target_available(&target).await.unwrap());
    assert!(is_transfer_target_available(&numbered_3).await.unwrap());
    assert_eq!(
        available_transfer_target_path(&target).await.unwrap(),
        numbered_3
    );
}

#[tokio::test]
async fn move_conflict_merge_directory_moves_missing_children() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(source.join("same.txt"), b"new").unwrap();
    fs::write(source.join("new.txt"), b"added").unwrap();
    fs::write(target.join("same.txt"), b"old").unwrap();

    move_path_with_options(
        &source,
        &target,
        FileTransferOptions::running(tokio_util::sync::CancellationToken::new())
            .with_conflict_strategy(TransferConflictStrategy::Merge),
    )
    .await
    .unwrap();

    assert_eq!(fs::read(target.join("same.txt")).unwrap(), b"old");
    assert_eq!(fs::read(target.join("new.txt")).unwrap(), b"added");
    assert!(source.join("same.txt").exists());
    assert!(!source.join("new.txt").exists());
}

#[tokio::test]
async fn copy_directory_recursively_copies_children() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let nested = source.join("nested");
    let empty = source.join("empty");
    let copied = dir.path().join("copied");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::create_dir(&empty).unwrap();
    fs::write(source.join("root.txt"), b"root").unwrap();
    fs::write(nested.join("child.txt"), b"child").unwrap();

    copy_path(
        &source,
        &copied,
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();

    assert!(copied.join("empty").is_dir());
    assert_eq!(fs::read(copied.join("root.txt")).unwrap(), b"root");
    assert_eq!(
        fs::read(copied.join("nested").join("child.txt")).unwrap(),
        b"child"
    );
}

#[tokio::test]
async fn copy_directory_rejects_target_inside_source() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let copied = source.join("copied");
    fs::create_dir(&source).unwrap();

    let error = copy_path(
        &source,
        &copied,
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap_err();

    match error {
        FileError::InvalidInput { path, message } => {
            assert_eq!(path, copied);
            assert_eq!(message, "cannot copy a directory into itself");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!source.join("copied").exists());
}

#[tokio::test]
async fn copy_operation_honors_pre_cancelled_token() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let copied = dir.path().join("copied.txt");
    fs::write(&source, b"copy me").unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();

    let error = copy_path(&source, &copied, token, None).await.unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(!copied.exists());
}

#[tokio::test]
async fn copy_operation_waits_while_paused_and_resumes() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let copied = dir.path().join("copied.txt");
    fs::write(&source, b"pause me").unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    let (run_state_sender, run_state_receiver) =
        tokio::sync::watch::channel(FileOperationRunState::Paused);
    let controls = FileOperationControls::new(token, run_state_receiver);

    let copy = tokio::spawn(copy_path_with_options(
        source.clone(),
        copied.clone(),
        FileTransferOptions::new(controls),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert!(!copied.exists());

    run_state_sender
        .send(FileOperationRunState::Running)
        .unwrap();
    copy.await.unwrap().unwrap();

    assert_eq!(fs::read(copied).unwrap(), b"pause me");
}

#[tokio::test]
async fn application_stopping_takes_precedence_over_user_cancellation() {
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let (_run_state_sender, run_state_receiver) =
        tokio::sync::watch::channel(FileOperationRunState::ApplicationStopping);
    let mut controls = FileOperationControls::new(cancel, run_state_receiver);

    let error = controls.wait_until_running().await.unwrap_err();

    assert!(matches!(error, FileError::ApplicationStopping));
}
