use super::*;

#[tokio::test]
async fn scan_directory_reads_regular_entries() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), b"hello").unwrap();
    fs::create_dir(dir.path().join("a-dir")).unwrap();

    let scan = scan_directory(dir.path(), ScanOptions::default())
        .await
        .unwrap();

    assert_eq!(scan.path, dir.path());
    assert_eq!(names(&scan.entries), vec!["a-dir", "b.txt"]);
    assert_eq!(scan.entries[0].kind, FileKind::Directory);
    assert_eq!(scan.entries[1].kind, FileKind::File);
}

#[tokio::test]
async fn scan_directory_populates_time_metadata_from_filesystem() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("report.txt");
    fs::write(&file_path, b"hello").unwrap();

    let scan = scan_directory(dir.path(), ScanOptions::default())
        .await
        .unwrap();
    let entry = scan
        .entries
        .iter()
        .find(|entry| entry.path == file_path)
        .expect("scanned entry");
    let metadata = fs::symlink_metadata(&file_path).unwrap();

    assert_eq!(entry.metadata.modified, metadata.modified().ok());
    assert_eq!(entry.metadata.accessed, metadata.accessed().ok());
    assert_eq!(entry.metadata.created, metadata.created().ok());
}

#[cfg(unix)]
#[tokio::test]
async fn scan_directory_populates_unix_owner_group_and_permissions() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("report.txt");
    fs::write(&file_path, b"hello").unwrap();

    let scan = scan_directory(dir.path(), ScanOptions::default())
        .await
        .unwrap();
    let entry = scan
        .entries
        .iter()
        .find(|entry| entry.path == file_path)
        .expect("scanned entry");
    let metadata = fs::symlink_metadata(&file_path).unwrap();

    assert!(entry
        .metadata
        .owner_name
        .as_deref()
        .is_some_and(|owner| !owner.is_empty()));
    assert!(entry
        .metadata
        .group_name
        .as_deref()
        .is_some_and(|group| !group.is_empty()));
    assert_eq!(
        entry.metadata.permissions_mode,
        Some(metadata.mode() & 0o7777)
    );
}

#[tokio::test]
async fn discover_directory_returns_authoritative_basic_entries() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), b"second").unwrap();
    fs::create_dir(dir.path().join("a-dir")).unwrap();

    let mut hinted_names = Vec::new();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |batch| {
            hinted_names.extend(
                batch
                    .entries
                    .iter()
                    .map(|entry| entry.name().to_string_lossy().into_owned()),
            );
        },
    )
    .await
    .unwrap();

    hinted_names.sort();
    let ordered_names = discovery
        .order
        .iter()
        .map(|index| {
            discovery.entries[*index]
                .name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(hinted_names, vec!["a-dir", "b.txt"]);
    assert_eq!(ordered_names, vec!["a-dir", "b.txt"]);
    assert!(discovery.entries.iter().all(|entry| {
        matches!(entry.filesystem_metadata(), DirectoryMetadataState::Pending)
            && matches!(entry.identity_names(), DirectoryMetadataState::Pending)
    }));
    let display_entry = discovery.entries[0].display_entry();
    assert_eq!(
        display_entry.metadata.filesystem_availability,
        DirectoryMetadataAvailability::Pending
    );
    assert_eq!(
        display_entry.metadata.identity_names_availability,
        DirectoryMetadataAvailability::Pending
    );
}

#[tokio::test]
async fn resolve_directory_metadata_deduplicates_filesystem_targets() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("report.txt");
    fs::write(&file_path, b"hello").unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let resolution = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 7,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: vec![0, 0],
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(resolution.request_generation, 7);
    assert_eq!(resolution.resolved_indices, vec![0]);
    assert_eq!(resolution.requested_targets, 2);
    assert_eq!(resolution.filesystem_calls, 1);
    assert!(resolution.warnings.is_empty());
    let entry = &discovery.entries[0];
    match entry.filesystem_metadata() {
        DirectoryMetadataState::Complete(metadata) => assert_eq!(metadata.len, 5),
        state => panic!("unexpected filesystem metadata state: {state:?}"),
    }
    assert!(matches!(
        entry.identity_names(),
        DirectoryMetadataState::Pending
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_directory_identity_names_caches_unique_ids() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    let resolution = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 8,
            requirement: DirectoryMetadataRequirement::IdentityNames,
            targets: vec![0, 1, 0],
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(resolution.resolved_indices, vec![0, 1]);
    assert_eq!(resolution.filesystem_calls, 2);
    assert_eq!(resolution.user_name_lookups, 1);
    assert_eq!(resolution.group_name_lookups, 1);
    assert_eq!(resolution.identity_worker_runs, 1);
    for entry in discovery.entries.iter() {
        match entry.identity_names() {
            DirectoryMetadataState::Complete(names) => {
                assert!(names
                    .owner_name
                    .as_deref()
                    .is_some_and(|name| !name.is_empty()));
                assert!(names
                    .group_name
                    .as_deref()
                    .is_some_and(|name| !name.is_empty()));
            }
            state => panic!("unexpected identity-name state: {state:?}"),
        }
    }
}

#[tokio::test]
async fn concurrent_metadata_requests_keep_request_counters_disjoint() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("one.txt"), b"one").unwrap();
    fs::write(dir.path().join("two.txt"), b"two").unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let first = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 20,
            requirement: DirectoryMetadataRequirement::IdentityNames,
            targets: vec![0],
        },
        tokio_util::sync::CancellationToken::new(),
    );
    let second = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 21,
            requirement: DirectoryMetadataRequirement::IdentityNames,
            targets: vec![1],
        },
        tokio_util::sync::CancellationToken::new(),
    );

    let (first, second) = tokio::join!(first, second);
    let resolutions = [first.unwrap(), second.unwrap()];

    assert_eq!(
        resolutions
            .iter()
            .map(|resolution| resolution.filesystem_calls)
            .sum::<usize>(),
        2
    );
    assert!(resolutions
        .iter()
        .all(|resolution| resolution.filesystem_calls == 1));
    assert_eq!(
        resolutions
            .iter()
            .map(|resolution| resolution.user_name_lookups)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        resolutions
            .iter()
            .map(|resolution| resolution.group_name_lookups)
            .sum::<usize>(),
        1
    );
    assert!(resolutions
        .iter()
        .all(|resolution| resolution.identity_worker_runs <= 1));
}

#[tokio::test]
async fn size_sort_waits_for_filesystem_metadata_then_orders_by_size() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a-large.txt"), vec![0_u8; 10]).unwrap();
    fs::write(dir.path().join("z-small.txt"), b"x").unwrap();
    let options = ScanOptions {
        sort_field: SortField::Size,
        ..ScanOptions::default()
    };
    let discovery = discover_directory_with_progress(
        dir.path(),
        options.clone(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    assert!(!discovered_sort_is_ready(&discovery.entries, &options));
    resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 9,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: vec![0, 1],
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(discovered_sort_is_ready(&discovery.entries, &options));
    let order = sort_discovered_entry_indices(&discovery.entries, &options);
    let ordered_names = order
        .iter()
        .map(|index| {
            discovery.entries[*index]
                .name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(ordered_names, vec!["z-small.txt", "a-large.txt"]);
}

#[tokio::test]
async fn cancelled_metadata_request_leaves_cells_pending() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("report.txt"), b"hello").unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();

    let error = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 10,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: vec![0],
        },
        cancellation,
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(matches!(
        discovery.entries[0].filesystem_metadata(),
        DirectoryMetadataState::Pending
    ));
}

#[tokio::test]
async fn metadata_failure_keeps_discovered_row_and_records_unavailable_warning() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("removed-after-discovery.txt");
    fs::write(&file_path, b"hello").unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    fs::remove_file(&file_path).unwrap();

    let resolution = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 11,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: vec![0],
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(discovery.entries.len(), 1);
    assert_eq!(resolution.resolved_indices, vec![0]);
    assert_eq!(resolution.warnings.len(), 1);
    assert!(matches!(
        discovery.entries[0].filesystem_metadata(),
        DirectoryMetadataState::Unavailable(_)
    ));
    assert_eq!(
        discovery.entries[0]
            .display_entry()
            .metadata
            .filesystem_availability,
        DirectoryMetadataAvailability::Unavailable
    );
}

#[tokio::test]
async fn discovery_applies_hidden_filter_before_authoritative_collection() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("visible.txt"), []).unwrap();
    fs::write(dir.path().join(".hidden.txt"), []).unwrap();

    let hidden_excluded = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let hidden_included = discover_directory_with_progress(
        dir.path(),
        ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        },
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(hidden_excluded.entries.len(), 1);
    assert_eq!(hidden_included.entries.len(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn discovery_preserves_non_utf8_name_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let dir = tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'n', b'o', b'n', 0xff]);
    fs::write(dir.path().join(&name), b"bytes").unwrap();

    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(discovery.entries[0].name().as_bytes(), name.as_bytes());
}

#[cfg(unix)]
#[tokio::test]
async fn staged_symlink_broken_fact_resolves_on_filesystem_demand() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("target"), b"target").unwrap();
    std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("broken")).unwrap();
    let discovery = discover_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let broken_index = discovery
        .entries
        .iter()
        .position(|entry| entry.name() == "broken")
        .unwrap();
    assert_eq!(discovery.entries[broken_index].kind(), FileKind::Symlink);
    assert!(
        !discovery.entries[broken_index]
            .display_entry()
            .is_broken_symlink
    );

    resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 12,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: vec![broken_index],
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(
        discovery.entries[broken_index]
            .display_entry()
            .is_broken_symlink
    );
}

#[tokio::test]
async fn scan_directory_with_progress_reports_batches_and_final_sorted_scan() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), b"hello").unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    let mut batch_names = Vec::new();
    let scan = scan_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |batch| batch_names.extend(names(&batch.entries)),
    )
    .await
    .unwrap();

    batch_names.sort();
    assert_eq!(batch_names, vec!["a.txt", "b.txt"]);
    assert_eq!(names(&scan.entries), vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn scan_directory_with_progress_emits_multiple_batches_matching_final_scan() {
    let dir = tempdir().unwrap();
    for index in 0..260 {
        fs::write(dir.path().join(format!("file-{index:03}.txt")), b"hello").unwrap();
    }

    let mut batch_names = Vec::new();
    let mut batch_count = 0usize;
    let scan = scan_directory_with_progress(
        dir.path(),
        ScanOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        |batch| {
            batch_count += 1;
            batch_names.extend(names(&batch.entries));
        },
    )
    .await
    .unwrap();
    let baseline = scan_directory(dir.path(), ScanOptions::default())
        .await
        .unwrap();

    batch_names.sort();
    assert!(batch_count > 1);
    assert_eq!(batch_names, names(&baseline.entries));
    assert_eq!(names(&scan.entries), names(&baseline.entries));
}

#[tokio::test]
async fn scan_directory_with_progress_respects_cancelled_token() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();

    let error =
        scan_directory_with_progress(dir.path(), ScanOptions::default(), cancellation, |_| {
            panic!("cancelled scan must not emit batches")
        })
        .await
        .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
}

#[cfg(unix)]
#[tokio::test]
async fn scan_directory_preserves_non_utf8_names() {
    let dir = tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'n', b'o', b'n', 0xff]);
    fs::write(dir.path().join(&name), b"bytes").unwrap();

    let scan = scan_directory(
        dir.path(),
        ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        },
    )
    .await
    .unwrap();

    assert!(scan
        .entries
        .iter()
        .any(|entry| entry.name() == OsStr::new(&name)));
}

#[cfg(unix)]
#[tokio::test]
async fn scan_directory_marks_symlinks_and_broken_symlinks() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("target"), b"target").unwrap();
    std::os::unix::fs::symlink(dir.path().join("target"), dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("broken")).unwrap();

    let scan = scan_directory(
        dir.path(),
        ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        },
    )
    .await
    .unwrap();

    let link = scan
        .entries
        .iter()
        .find(|entry| entry.name() == "link")
        .unwrap();
    let broken = scan
        .entries
        .iter()
        .find(|entry| entry.name() == "broken")
        .unwrap();

    assert_eq!(link.kind, FileKind::Symlink);
    assert!(link.is_symlink);
    assert!(!link.is_broken_symlink);
    assert_eq!(broken.kind, FileKind::Symlink);
    assert!(broken.is_broken_symlink);
}

#[tokio::test]
async fn scan_missing_directory_returns_structured_error() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing");

    let error = scan_directory(&missing, ScanOptions::default())
        .await
        .unwrap_err();

    match error {
        FileError::ReadDirectory { path, source } => {
            assert_eq!(path, missing);
            assert_eq!(source.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn scan_unreadable_directory_reports_error_when_os_denies_access() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    let original_permissions = fs::metadata(&locked).unwrap().permissions();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let result = scan_directory(&locked, ScanOptions::default()).await;

    fs::set_permissions(&locked, original_permissions).unwrap();

    if let Err(FileError::ReadDirectory { source, .. }) = result {
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
    }
}
