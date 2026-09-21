use super::*;

#[cfg(unix)]
mod unix {
    use std::ffi::{CString, OsString};
    use std::io::Write;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{symlink, FileTypeExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    use super::*;
    use tokio_util::sync::CancellationToken;

    async fn copy(source: &Path, target: &Path) -> Result<(), FileError> {
        copy_path(source, target, CancellationToken::new(), None).await
    }

    #[tokio::test]
    async fn copy_preserves_relative_and_absolute_symbolic_link_targets() {
        let directory = tempdir().unwrap();
        let source_file = directory.path().join("source.txt");
        let relative_link = directory.path().join("relative-link");
        let relative_copy = directory.path().join("relative-copy");
        let absolute_link = directory.path().join("absolute-link");
        let absolute_copy = directory.path().join("absolute-copy");
        fs::write(&source_file, b"payload").unwrap();
        symlink("source.txt", &relative_link).unwrap();
        symlink(&source_file, &absolute_link).unwrap();

        copy(&relative_link, &relative_copy).await.unwrap();
        copy(&absolute_link, &absolute_copy).await.unwrap();

        assert!(fs::symlink_metadata(&relative_copy)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&relative_copy).unwrap(),
            Path::new("source.txt")
        );
        assert!(fs::symlink_metadata(&absolute_copy)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&absolute_copy).unwrap(), source_file);
    }

    #[tokio::test]
    async fn copy_preserves_broken_non_utf8_symbolic_link_target() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source-link");
        let target = directory.path().join("target-link");
        let link_target = OsString::from_vec(vec![b'm', b'i', b's', b's', b'i', b'n', b'g', 0xff]);
        symlink(&link_target, &source).unwrap();

        copy(&source, &target).await.unwrap();

        assert!(fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&target).unwrap().as_os_str(), link_target);
    }

    #[tokio::test]
    async fn directory_copy_does_not_traverse_symbolic_link_to_directory() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let outside = directory.path().join("outside");
        let target = directory.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("outside.txt"), b"outside").unwrap();
        symlink(&outside, source.join("directory-link")).unwrap();

        copy(&source, &target).await.unwrap();

        let copied_link = target.join("directory-link");
        assert!(fs::symlink_metadata(&copied_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&copied_link).unwrap(), outside);
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn regular_file_copy_preserves_mode_times_and_extended_attributes() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        fs::write(&source, b"payload").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        xattr::set(&source, "user.file-manager-transfer", b"file-value").unwrap();
        let access_time = filetime::FileTime::from_unix_time(1_650_000_001, 123_456_789);
        let modification_time = filetime::FileTime::from_unix_time(1_650_000_002, 987_654_321);
        filetime::set_file_times(&source, access_time, modification_time).unwrap();

        copy(&source, &target).await.unwrap();

        let target_metadata = fs::metadata(&target).unwrap();
        assert_eq!(target_metadata.permissions().mode() & 0o7777, 0o640);
        assert_eq!(
            filetime::FileTime::from_last_access_time(&target_metadata),
            access_time
        );
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&target_metadata),
            modification_time
        );
        assert_eq!(
            xattr::get(&target, "user.file-manager-transfer").unwrap(),
            Some(b"file-value".to_vec())
        );
    }

    #[tokio::test]
    async fn directory_copy_preserves_metadata_after_creating_descendants() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let nested = source.join("nested");
        let target = directory.path().join("target");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("child.txt"), b"payload").unwrap();
        xattr::set(&source, "user.file-manager-transfer", b"root-value").unwrap();
        xattr::set(&nested, "user.file-manager-transfer", b"nested-value").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o701)).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o500)).unwrap();
        let root_access_time = filetime::FileTime::from_unix_time(1_640_000_001, 0);
        let root_modification_time = filetime::FileTime::from_unix_time(1_640_000_002, 0);
        let nested_access_time = filetime::FileTime::from_unix_time(1_640_000_003, 0);
        let nested_modification_time = filetime::FileTime::from_unix_time(1_640_000_004, 0);
        filetime::set_file_times(&source, root_access_time, root_modification_time).unwrap();
        filetime::set_file_times(&nested, nested_access_time, nested_modification_time).unwrap();

        copy(&source, &target).await.unwrap();

        let target_metadata = fs::metadata(&target).unwrap();
        let nested_target_metadata = fs::metadata(target.join("nested")).unwrap();
        assert_eq!(target_metadata.permissions().mode() & 0o7777, 0o701);
        assert_eq!(nested_target_metadata.permissions().mode() & 0o7777, 0o500);
        assert_eq!(
            filetime::FileTime::from_last_access_time(&target_metadata),
            root_access_time
        );
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&target_metadata),
            root_modification_time
        );
        assert_eq!(
            filetime::FileTime::from_last_access_time(&nested_target_metadata),
            nested_access_time
        );
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&nested_target_metadata),
            nested_modification_time
        );
        assert_eq!(
            xattr::get(&target, "user.file-manager-transfer").unwrap(),
            Some(b"root-value".to_vec())
        );
        assert_eq!(
            xattr::get(target.join("nested"), "user.file-manager-transfer").unwrap(),
            Some(b"nested-value".to_vec())
        );
    }

    #[tokio::test]
    async fn symbolic_link_copy_preserves_link_times() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source-link");
        let target = directory.path().join("target-link");
        symlink("missing", &source).unwrap();
        let access_time = filetime::FileTime::from_unix_time(1_630_000_001, 0);
        let modification_time = filetime::FileTime::from_unix_time(1_630_000_002, 0);
        filetime::set_symlink_file_times(&source, access_time, modification_time).unwrap();

        copy(&source, &target).await.unwrap();

        let target_metadata = fs::symlink_metadata(&target).unwrap();
        assert_eq!(
            filetime::FileTime::from_last_access_time(&target_metadata),
            access_time
        );
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&target_metadata),
            modification_time
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn regular_file_copy_preserves_posix_access_acl() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        fs::write(&source, b"payload").unwrap();
        let mut source_acl = exacl::from_mode(0o640);
        source_acl.push(exacl::AclEntry::allow_user(
            "424242",
            exacl::Perm::READ,
            None,
        ));
        exacl::setfacl(&[source.as_path()], &source_acl, None).unwrap();
        let expected_acl = exacl::getfacl(&source, None).unwrap();

        copy(&source, &target).await.unwrap();

        assert_eq!(exacl::getfacl(&target, None).unwrap(), expected_acl);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn directory_copy_preserves_posix_access_and_default_acl() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::create_dir(&source).unwrap();
        let mut source_acl = exacl::from_mode(0o750);
        source_acl.push(exacl::AclEntry::allow_group(
            "424242",
            exacl::Perm::READ | exacl::Perm::EXECUTE,
            None,
        ));
        let mut default_acl = source_acl.clone();
        for entry in &mut default_acl {
            entry.flags |= exacl::Flag::DEFAULT;
        }
        source_acl.append(&mut default_acl);
        exacl::setfacl(&[source.as_path()], &source_acl, None).unwrap();
        let expected_acl = exacl::getfacl(&source, None).unwrap();

        copy(&source, &target).await.unwrap();

        assert_eq!(exacl::getfacl(&target, None).unwrap(), expected_acl);
    }

    #[tokio::test]
    async fn copy_and_move_symbolic_links_do_not_report_byte_progress() {
        let directory = tempdir().unwrap();
        let copy_source = directory.path().join("copy-source-link");
        let copy_target = directory.path().join("copy-target-link");
        let move_source = directory.path().join("move-source-link");
        let move_target = directory.path().join("move-target-link");
        symlink("missing-copy", &copy_source).unwrap();
        symlink("missing-move", &move_source).unwrap();
        let (copy_progress, mut copied_updates) = tokio::sync::mpsc::unbounded_channel();
        let (move_progress, mut moved_updates) = tokio::sync::mpsc::unbounded_channel();

        copy_path(
            &copy_source,
            &copy_target,
            CancellationToken::new(),
            Some(copy_progress),
        )
        .await
        .unwrap();
        move_path(
            &move_source,
            &move_target,
            CancellationToken::new(),
            Some(move_progress),
        )
        .await
        .unwrap();

        assert!(copied_updates.try_recv().is_err());
        assert!(moved_updates.try_recv().is_err());
        assert_eq!(
            fs::read_link(&copy_target).unwrap(),
            Path::new("missing-copy")
        );
        assert_eq!(
            fs::read_link(&move_target).unwrap(),
            Path::new("missing-move")
        );
        assert!(fs::symlink_metadata(move_source).is_err());
    }

    #[tokio::test]
    async fn directory_move_keeps_existing_completion_progress_total() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        fs::create_dir(&source).unwrap();
        let expected_total = fs::symlink_metadata(&source).unwrap().len();
        let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();

        move_path(&source, &target, CancellationToken::new(), Some(progress))
            .await
            .unwrap();

        let completion = updates.try_recv().unwrap();
        assert_eq!(completion.bytes_done, expected_total);
        assert_eq!(completion.bytes_total, expected_total);
        assert!(updates.try_recv().is_err());
    }

    #[tokio::test]
    async fn move_rejects_fifo_without_removing_source_or_creating_target() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source-fifo");
        let target = directory.path().join("target");
        create_fifo(&source);

        let error = move_path(&source, &target, CancellationToken::new(), None)
            .await
            .unwrap_err();

        assert!(matches!(error, FileError::InvalidInput { path, .. } if path == source));
        assert!(fs::symlink_metadata(&source).unwrap().file_type().is_fifo());
        assert!(fs::symlink_metadata(target).is_err());
    }

    #[tokio::test]
    async fn directory_merge_does_not_follow_symbolic_link_target() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let real_target = directory.path().join("real-target");
        let target_link = directory.path().join("target-link");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&real_target).unwrap();
        fs::write(source.join("source.txt"), b"source").unwrap();
        fs::write(real_target.join("existing.txt"), b"existing").unwrap();
        symlink(&real_target, &target_link).unwrap();

        let copied_target = copy_path_with_options(
            &source,
            &target_link,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::Merge),
        )
        .await
        .unwrap();

        assert!(copied_target.is_none());
        assert!(fs::symlink_metadata(&target_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(real_target.join("existing.txt")).unwrap(),
            b"existing"
        );
        assert!(!real_target.join("source.txt").exists());
    }

    #[tokio::test]
    async fn replace_removes_target_link_without_touching_linked_directory() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let real_target = directory.path().join("real-target");
        let target_link = directory.path().join("target-link");
        fs::write(&source, b"replacement").unwrap();
        fs::create_dir(&real_target).unwrap();
        fs::write(real_target.join("existing.txt"), b"existing").unwrap();
        symlink(&real_target, &target_link).unwrap();

        copy_path_with_options(
            &source,
            &target_link,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::Replace),
        )
        .await
        .unwrap();

        assert!(fs::symlink_metadata(&target_link)
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(fs::read(&target_link).unwrap(), b"replacement");
        assert_eq!(
            fs::read(real_target.join("existing.txt")).unwrap(),
            b"existing"
        );
    }

    #[tokio::test]
    async fn unix_socket_is_rejected_before_creating_target() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.socket");
        let target = directory.path().join("target");
        let _listener = UnixListener::bind(&source).unwrap();

        let error = copy(&source, &target).await.unwrap_err();

        match error {
            FileError::InvalidInput { path, message } => {
                assert_eq!(path, source);
                assert!(message.contains("socket"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(fs::symlink_metadata(target).is_err());
    }

    #[tokio::test]
    async fn fifo_is_rejected_before_creating_target() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source-fifo");
        let target = directory.path().join("target");
        create_fifo(&source);
        let mut fifo_guard = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)
            .unwrap();

        let source_for_copy = source.clone();
        let target_for_copy = target.clone();
        let copy_task = tokio::spawn(async move { copy(&source_for_copy, &target_for_copy).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        fifo_guard.write_all(b"payload").unwrap();
        drop(fifo_guard);
        let error = tokio::time::timeout(Duration::from_secs(2), copy_task)
            .await
            .expect("FIFO copy must not block")
            .unwrap()
            .unwrap_err();

        match error {
            FileError::InvalidInput { path, message } => {
                assert_eq!(path, source);
                assert!(message.contains("FIFO"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(fs::symlink_metadata(target).is_err());
    }

    #[tokio::test]
    async fn new_directory_target_is_removed_when_child_fifo_is_rejected() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let source_fifo = source.join("child-fifo");
        let target = directory.path().join("target");
        fs::create_dir(&source).unwrap();
        create_fifo(&source_fifo);
        let mut fifo_guard = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source_fifo)
            .unwrap();

        let source_for_copy = source.clone();
        let target_for_copy = target.clone();
        let copy_task = tokio::spawn(async move { copy(&source_for_copy, &target_for_copy).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        fifo_guard.write_all(b"payload").unwrap();
        drop(fifo_guard);
        let error = tokio::time::timeout(Duration::from_secs(2), copy_task)
            .await
            .expect("directory copy must not block on FIFO")
            .unwrap()
            .unwrap_err();

        assert!(matches!(error, FileError::InvalidInput { path, .. } if path == source_fifo));
        assert!(fs::symlink_metadata(target).is_err());
    }

    #[tokio::test]
    async fn broken_symbolic_link_occupies_transfer_target_and_keep_both_uses_suffix() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target-link");
        let alternate = directory.path().join("target-link 2");
        fs::write(&source, b"payload").unwrap();
        symlink("missing", &target).unwrap();

        assert!(!is_transfer_target_available(&target).await.unwrap());
        copy_path_with_options(
            &source,
            &target,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
        )
        .await
        .unwrap();

        assert_eq!(fs::read_link(&target).unwrap(), Path::new("missing"));
        assert_eq!(fs::read(&alternate).unwrap(), b"payload");
    }

    #[tokio::test]
    async fn symbolic_link_to_directory_is_not_a_mergeable_conflict() {
        let directory = tempdir().unwrap();
        let source_directory = directory.path().join("source-directory");
        let source_link = directory.path().join("source-link");
        let target = directory.path().join("target");
        fs::create_dir(&source_directory).unwrap();
        fs::create_dir(&target).unwrap();
        symlink(&source_directory, &source_link).unwrap();

        let conflicts =
            check_transfer_conflicts(vec![TransferConflictCheck::new(source_link, target)]).await;

        assert_eq!(conflicts.len(), 1);
        assert!(!conflicts[0].source_metadata.is_directory);
        assert!(!conflicts[0].can_merge());
    }

    fn create_fifo(path: &Path) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let status = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(
            status,
            0,
            "could not create FIFO: {}",
            io::Error::last_os_error()
        );
    }
}
