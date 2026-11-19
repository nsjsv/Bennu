use file_core::FileKind;

use super::*;
use crate::model::{
    FilePropertiesFileDetails, FilePropertiesLookup, FilePropertiesOwner,
    FilePropertiesPermissionWriteOutcome, FilePropertiesPresentation, FilePropertiesSnapshot,
    PermissionBatchOutcome, PermissionBatchPathFailure,
};

#[test]
fn application_multi_selection_opens_the_shared_target_set_loader() {
    let first = PathBuf::from("/workspace/first");
    let second = PathBuf::from("/workspace/second");
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().entries = vec![
        file_core::DirectoryEntry::new(
            first.clone(),
            FileKind::File,
            file_core::EntryMetadata::default(),
            false,
            false,
            false,
        ),
        file_core::DirectoryEntry::new(
            second.clone(),
            FileKind::File,
            file_core::EntryMetadata::default(),
            false,
            false,
            false,
        ),
    ]
    .into();
    browser.tab_mut().selected = Some(first.clone());
    browser.tab_mut().selected_paths =
        std::collections::HashSet::from([first.clone(), second.clone()]);

    let _command = browser.open_selected_file_properties();

    let properties = browser.properties.as_ref().expect("properties opened");
    assert_eq!(properties.targets.paths().len(), 2);
    assert!(properties.targets.paths().contains(&first));
    assert!(properties.targets.paths().contains(&second));
    assert!(matches!(
        properties.load_state,
        FilePropertiesLoadState::Loading
    ));
}

#[test]
fn aggregate_progress_remains_non_terminal_until_recursive_scan_finishes() {
    let targets = FilePropertiesTargetSet::new(vec![
        PathBuf::from("/workspace/first"),
        PathBuf::from("/workspace/second"),
    ])
    .expect("target set");
    let request = FilePropertiesRequest {
        targets: targets.clone(),
        generation: 3,
    };
    let progress = aggregate_snapshot(2);
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.properties = Some(FilePropertiesState::loading(
        request.clone(),
        CancellationToken::new(),
    ));

    let _progress =
        browser.accept_file_properties_aggregate_progress(request.clone(), progress.clone());
    let _toggle = browser.toggle_file_properties_permission(
        FilePropertiesPermissionClass::Owner,
        FilePropertiesPermissionAccess::Write,
    );

    let properties = browser.properties.as_ref().expect("properties remain open");
    assert!(matches!(
        properties.load_state,
        FilePropertiesLoadState::LoadingAggregate(_)
    ));
    assert_eq!(
        properties.permission_update,
        FilePropertiesPermissionUpdate::Idle
    );

    let _completed = browser
        .accept_file_properties(request, Ok(FilePropertiesPresentation::Aggregate(progress)));
    assert!(matches!(
        browser.properties.as_ref().unwrap().load_state,
        FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Aggregate(_))
    ));
}

#[test]
fn properties_directory_contents_rejects_stale_generation_for_same_targets() {
    let path = PathBuf::from("/workspace/project");
    let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
    let stale_request = properties_request(path, 1);

    let _command = browser
        .accept_file_properties_directory_contents(stale_request, Ok(directory_contents(9, 9, 9)));

    let snapshot = loaded_single_snapshot(&browser);
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loading(None))
    ));
    assert_eq!(snapshot.size_bytes, 0);
}

#[test]
fn properties_directory_contents_accepts_current_generation() {
    let path = PathBuf::from("/workspace/project");
    let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
    let request = properties_request(path, 2);

    let _command = browser
        .accept_file_properties_directory_contents(request, Ok(directory_contents(3, 2, 512)));

    let snapshot = loaded_single_snapshot(&browser);
    assert_eq!(snapshot.size_bytes, 512);
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loaded(
            FilePropertiesDirectoryContents {
                file_count: 3,
                directory_count: 2,
                ..
            }
        ))
    ));
}

#[test]
fn current_item_permission_save_updates_snapshot_permissions() {
    let path = PathBuf::from("/workspace/project");
    let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
    browser.properties.as_mut().unwrap().permission_update =
        FilePropertiesPermissionUpdate::SavingCurrentItem {
            permissions: FilePropertiesPermissions::from_mode(0o755),
        };

    let _command = browser.accept_file_properties_permissions(
        properties_request(path, 2),
        Ok(FilePropertiesPermissionWriteOutcome::Single(
            FilePropertiesPermissions::from_mode(0o700),
        )),
    );

    assert_eq!(
        loaded_single_snapshot(&browser).permissions,
        Some(FilePropertiesPermissions::from_mode(0o700))
    );
    assert_eq!(
        browser.properties.as_ref().unwrap().permission_update,
        FilePropertiesPermissionUpdate::Idle
    );
}

#[test]
fn target_set_permission_outcome_reloads_and_preserves_partial_fact() {
    let first = PathBuf::from("/workspace/first");
    let second = PathBuf::from("/workspace/second");
    let targets =
        FilePropertiesTargetSet::new(vec![first.clone(), second.clone()]).expect("target set");
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.properties = Some(FilePropertiesState::loading(
        FilePropertiesRequest {
            targets: targets.clone(),
            generation: 4,
        },
        CancellationToken::new(),
    ));
    browser.properties_load_generation = 4;

    let _command = browser.accept_file_properties_permissions(
        FilePropertiesRequest {
            targets,
            generation: 4,
        },
        Ok(FilePropertiesPermissionWriteOutcome::Batch(
            PermissionBatchOutcome {
                succeeded_paths: vec![first],
                failures: vec![PermissionBatchPathFailure {
                    path: second,
                    error: "identity changed".to_owned(),
                }],
            },
        )),
    );

    let properties = browser.properties.as_ref().expect("properties remain open");
    assert_eq!(properties.load_generation, 5);
    assert!(matches!(
        properties.load_state,
        FilePropertiesLoadState::Loading
    ));
    assert!(matches!(
        properties.permission_update,
        FilePropertiesPermissionUpdate::TargetSetCompleted {
            succeeded_count: 1,
            ref failures,
        } if failures.len() == 1
    ));
}

#[test]
fn enclosed_permission_result_rejects_stale_generation_for_same_targets() {
    let path = PathBuf::from("/workspace/project");
    let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
    browser.properties.as_mut().unwrap().permission_update =
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems {
            permissions: FilePropertiesPermissions::from_mode(0o755),
        };

    let _command = browser.accept_file_properties_enclosed_permissions(
        properties_request(path, 1),
        Ok(FilePropertiesPermissions::from_mode(0o700)),
    );

    assert_eq!(
        loaded_single_snapshot(&browser).permissions,
        Some(FilePropertiesPermissions::from_mode(0o755))
    );
    assert!(matches!(
        browser.properties.as_ref().unwrap().permission_update,
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. }
    ));
}

#[test]
fn permission_actions_are_ignored_while_enclosed_permissions_are_applying() {
    let path = PathBuf::from("/workspace/project");
    let mut browser = browser_with_loaded_directory_properties(path, 2);
    browser.properties.as_mut().unwrap().permission_update =
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems {
            permissions: FilePropertiesPermissions::from_mode(0o755),
        };

    let _toggle = browser.toggle_file_properties_permission(
        FilePropertiesPermissionClass::Owner,
        FilePropertiesPermissionAccess::Write,
    );
    let _enclosed = browser.apply_file_properties_permissions_to_enclosed_items();

    assert!(matches!(
        browser.properties.as_ref().unwrap().permission_update,
        FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. }
    ));
}

#[test]
fn opening_new_properties_cancels_previous_load() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let old_cancel = CancellationToken::new();
    browser.properties = Some(FilePropertiesState::loading(
        properties_request(PathBuf::from("/workspace/old"), 1),
        old_cancel.clone(),
    ));
    browser.properties_load_generation = 1;

    let (_request, _cancel) = browser.next_file_properties_request(
        FilePropertiesTargetSet::single(PathBuf::from("/workspace/new")),
    );

    assert!(old_cancel.is_cancelled());
    assert_eq!(browser.properties_load_generation, 2);
}

#[test]
fn slow_lookups_fill_in_only_for_the_current_request() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_loaded_properties(path.clone(), 2, image_snapshot(path.clone()));
    let rows = vec![bennu_preview::info_overlay::InfoRow {
        label: "Dimensions".to_owned(),
        value: "4 × 3".to_owned(),
    }];

    let _stale =
        browser.accept_file_properties_message(FilePropertiesMessage::DefaultApplicationLoaded(
            properties_request(path.clone(), 1),
            Ok(Some("Old Viewer".to_owned())),
        ));
    let _stale = browser.accept_file_properties_message(FilePropertiesMessage::MediaDetailsLoaded(
        properties_request(path.clone(), 1),
        Ok(rows.clone()),
    ));
    assert_eq!(
        loaded_file_details(&browser).default_application,
        FilePropertiesLookup::Pending
    );
    assert_eq!(
        loaded_file_details(&browser).media_details,
        Some(FilePropertiesLookup::Pending)
    );

    let _current =
        browser.accept_file_properties_message(FilePropertiesMessage::DefaultApplicationLoaded(
            properties_request(path.clone(), 2),
            Ok(Some("Viewer".to_owned())),
        ));
    let _current =
        browser.accept_file_properties_message(FilePropertiesMessage::MediaDetailsLoaded(
            properties_request(path, 2),
            Err("ffprobe failed".to_owned()),
        ));
    assert_eq!(
        loaded_file_details(&browser).default_application,
        FilePropertiesLookup::Ready(Some("Viewer".to_owned()))
    );
    assert_eq!(
        loaded_file_details(&browser).media_details,
        Some(FilePropertiesLookup::Failed("ffprobe failed".to_owned()))
    );
}

#[test]
fn folder_contents_never_turn_a_file_into_a_folder() {
    let path = PathBuf::from("/workspace/photo.png");
    let mut browser = browser_with_loaded_properties(path.clone(), 2, image_snapshot(path.clone()));

    let _command = browser.accept_file_properties_directory_contents(
        properties_request(path, 2),
        Ok(directory_contents(3, 2, 512)),
    );

    let snapshot = loaded_single_snapshot(&browser);
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::File(_)
    ));
    assert_eq!(snapshot.size_bytes, 0);
}

fn browser_with_loaded_directory_properties(path: PathBuf, generation: u64) -> FileBrowser {
    let snapshot = directory_snapshot(path.clone());
    browser_with_loaded_properties(path, generation, snapshot)
}

fn browser_with_loaded_properties(
    path: PathBuf,
    generation: u64,
    snapshot: FilePropertiesSnapshot,
) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.properties = Some(FilePropertiesState {
        targets: FilePropertiesTargetSet::single(path),
        load_state: FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)),
        permission_update: FilePropertiesPermissionUpdate::Idle,
        load_generation: generation,
        load_cancel: CancellationToken::new(),
    });
    browser
}

fn properties_request(path: PathBuf, generation: u64) -> FilePropertiesRequest {
    FilePropertiesRequest {
        targets: FilePropertiesTargetSet::single(path),
        generation,
    }
}

fn loaded_single_snapshot(browser: &FileBrowser) -> &FilePropertiesSnapshot {
    let FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) =
        &browser.properties.as_ref().unwrap().load_state
    else {
        panic!("single properties should remain loaded");
    };
    snapshot
}

fn directory_snapshot(path: PathBuf) -> FilePropertiesSnapshot {
    snapshot(
        path,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loading(None)),
    )
}

/// 刚读完的图片：打开方式和详细信息都还在读。
fn image_snapshot(path: PathBuf) -> FilePropertiesSnapshot {
    snapshot(
        path,
        FilePropertiesKindDetails::File(FilePropertiesFileDetails {
            mime_type: "image/png".to_owned(),
            description: Some("PNG image".to_owned()),
            default_application: FilePropertiesLookup::Pending,
            media_details: Some(FilePropertiesLookup::Pending),
        }),
    )
}

fn snapshot(path: PathBuf, details: FilePropertiesKindDetails) -> FilePropertiesSnapshot {
    FilePropertiesSnapshot {
        name: path.file_name().unwrap().to_os_string(),
        location: path.parent().unwrap().to_path_buf(),
        created: None,
        modified: None,
        accessed: None,
        changed: None,
        size_bytes: 0,
        disk_size_bytes: 0,
        details,
        owner: FilePropertiesOwner {
            user: "alice".to_owned(),
            group: "staff".to_owned(),
        },
        volume: Err("not measured".to_owned()),
        permissions: Some(FilePropertiesPermissions::from_mode(0o755)),
    }
}

fn loaded_file_details(browser: &FileBrowser) -> &FilePropertiesFileDetails {
    let FilePropertiesKindDetails::File(file) = &loaded_single_snapshot(browser).details else {
        panic!("a file should stay a file");
    };
    file
}

fn aggregate_snapshot(target_count: usize) -> FilePropertiesAggregateSnapshot {
    FilePropertiesAggregateSnapshot {
        target_count,
        file_count: target_count,
        directory_count: 0,
        symlink_count: 0,
        other_count: 0,
        total_size_bytes: 0,
        total_disk_size_bytes: 0,
        recursive_contents: directory_contents(0, 0, 0),
        common_parent: Some(PathBuf::from("/workspace")),
        common_kind: Some(FileKind::File),
        common_created: None,
        common_modified: None,
        common_accessed: None,
        common_changed: None,
        permissions: Some(FilePropertiesPermissions::from_mode(0o644)),
        permission_baselines: Vec::new(),
    }
}

fn directory_contents(
    file_count: usize,
    directory_count: usize,
    total_size_bytes: u64,
) -> FilePropertiesDirectoryContents {
    FilePropertiesDirectoryContents {
        file_count,
        directory_count,
        total_size_bytes,
        total_disk_size_bytes: total_size_bytes,
        unreadable_entries: 0,
    }
}
