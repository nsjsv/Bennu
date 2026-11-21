use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::Path;

use tempfile::tempdir;

use super::*;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("test runtime")
        .block_on(future)
}

#[test]
fn aggregate_properties_combine_top_level_kinds_and_recursive_contents() {
    let temp = tempdir().expect("create temp dir");
    let directory = temp.path().join("directory");
    let child_directory = directory.join("child");
    let nested = child_directory.join("nested.txt");
    let file = temp.path().join("file.txt");
    fs::create_dir(&directory).expect("create directory");
    fs::create_dir(&child_directory).expect("create child directory");
    fs::write(&nested, b"nested").expect("write nested");
    fs::write(&file, b"file").expect("write file");

    let snapshot = block_on(read_aggregate_file_properties(
        &[directory, file],
        CancellationToken::new(),
        |_| {},
    ))
    .expect("aggregate properties");

    assert_eq!(snapshot.target_count, 2);
    assert_eq!(snapshot.file_count, 1);
    assert_eq!(snapshot.directory_count, 1);
    assert_eq!(snapshot.recursive_contents.file_count, 1);
    assert_eq!(snapshot.recursive_contents.directory_count, 1);
    assert!(snapshot.total_size_bytes >= 10);
    assert_eq!(snapshot.common_parent, Some(temp.path().to_path_buf()));
    assert_eq!(snapshot.common_kind, None);
}

#[test]
fn aggregate_properties_honor_cancellation_before_initial_stat() {
    let temp = tempdir().expect("create temp dir");
    let target = temp.path().join("file.txt");
    fs::write(&target, b"file").expect("write file");
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = block_on(read_aggregate_file_properties(
        &[target],
        cancellation,
        |_| {},
    ))
    .expect_err("cancelled aggregate must stop");

    assert_eq!(error, "operation cancelled");
}

#[test]
fn aggregate_symlink_makes_permissions_read_only() {
    let temp = tempdir().expect("create temp dir");
    let target = temp.path().join("target.txt");
    let link = temp.path().join("target-link");
    fs::write(&target, b"target").expect("write target");
    symlink(&target, &link).expect("create symlink");

    let snapshot = block_on(read_aggregate_file_properties(
        &[target, link],
        CancellationToken::new(),
        |_| {},
    ))
    .expect("aggregate properties");

    assert_eq!(snapshot.permissions, None);
    assert!(snapshot.permission_baselines.is_empty());
}

#[test]
fn target_set_permissions_use_descriptor_even_without_read_permission() {
    let temp = tempdir().expect("create temp dir");
    let file = temp.path().join("file.txt");
    fs::write(&file, b"content").expect("write file");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).expect("remove permissions");
    let baseline = permission_baseline(&file);

    let outcome = write_file_properties_target_set_permissions(
        vec![baseline],
        FilePropertiesPermissions::from_mode(0o600),
    );

    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.succeeded_paths, vec![file.clone()]);
    assert_mode(&file, 0o600);
}

#[test]
fn target_set_permissions_reject_mode_change_before_commit() {
    let temp = tempdir().expect("create temp dir");
    let file = temp.path().join("file.txt");
    fs::write(&file, b"content").expect("write file");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("set baseline mode");
    let baseline = permission_baseline(&file);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).expect("change mode");

    let outcome = write_file_properties_target_set_permissions(
        vec![baseline],
        FilePropertiesPermissions::from_mode(0o700),
    );

    assert!(outcome.succeeded_paths.is_empty());
    assert_eq!(outcome.failures.len(), 1);
    assert!(outcome.failures[0].error.contains("permissions changed"));
    assert_mode(&file, 0o640);
}

#[test]
fn target_set_permissions_reject_path_replacement_before_commit() {
    let temp = tempdir().expect("create temp dir");
    let file = temp.path().join("file.txt");
    let retired = temp.path().join("retired.txt");
    fs::write(&file, b"original").expect("write original");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("set baseline mode");
    let baseline = permission_baseline(&file);
    fs::rename(&file, &retired).expect("retire original");
    fs::write(&file, b"replacement").expect("write replacement");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("set replacement mode");

    let outcome = write_file_properties_target_set_permissions(
        vec![baseline],
        FilePropertiesPermissions::from_mode(0o700),
    );

    assert!(outcome.succeeded_paths.is_empty());
    assert_eq!(outcome.failures.len(), 1);
    assert!(outcome.failures[0].error.contains("identity changed"));
    assert_mode(&file, 0o600);
    assert_mode(&retired, 0o600);
}

fn permission_baseline(path: &Path) -> FilePropertiesPermissionBaseline {
    let metadata = fs::symlink_metadata(path).expect("metadata");
    FilePropertiesPermissionBaseline {
        path: path.to_path_buf(),
        identity: metadata_properties_identity(&metadata).expect("identity"),
        permissions: metadata_properties_permissions(&metadata, false).expect("permissions"),
    }
}

#[test]
fn recursive_permissions_apply_to_root_directories_and_files() {
    let temp = tempdir().expect("create temp dir");
    let root = temp.path().join("root");
    let child_dir = root.join("child");
    let file = child_dir.join("file.txt");
    fs::create_dir(&root).expect("create root");
    fs::create_dir(&child_dir).expect("create child");
    fs::write(&file, "content").expect("write file");

    write_file_properties_permissions_to_enclosed_items(
        root.clone(),
        FilePropertiesPermissions::from_mode(0o755),
    )
    .expect("apply recursive permissions");

    assert_mode(&root, 0o755);
    assert_mode(&child_dir, 0o755);
    assert_mode(&file, 0o755);
}

#[test]
fn recursive_permissions_skip_symlinks_and_do_not_follow_targets() {
    let temp = tempdir().expect("create temp dir");
    let root = temp.path().join("root");
    let target = temp.path().join("target.txt");
    let link = root.join("target-link");
    fs::create_dir(&root).expect("create root");
    fs::write(&target, "target").expect("write target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("set target mode");
    symlink(&target, &link).expect("create symlink");

    write_file_properties_permissions_to_enclosed_items(
        root.clone(),
        FilePropertiesPermissions::from_mode(0o644),
    )
    .expect("apply recursive permissions");

    assert_mode(&root, 0o644);
    assert_mode(&target, 0o600);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("restore root permissions");
    assert!(fs::symlink_metadata(&link)
        .expect("link metadata")
        .file_type()
        .is_symlink());
}

#[test]
fn recursive_permissions_use_postorder_when_directory_execute_is_removed() {
    let temp = tempdir().expect("create temp dir");
    let root = temp.path().join("root");
    let child_dir = root.join("child");
    let file = child_dir.join("file.txt");
    fs::create_dir(&root).expect("create root");
    fs::create_dir(&child_dir).expect("create child");
    fs::write(&file, "content").expect("write file");

    write_file_properties_permissions_to_enclosed_items(
        root.clone(),
        FilePropertiesPermissions::from_mode(0o600),
    )
    .expect("apply recursive permissions");

    assert_mode(&root, 0o600);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("restore root permissions");
    assert_mode(&child_dir, 0o600);
    fs::set_permissions(&child_dir, fs::Permissions::from_mode(0o700))
        .expect("restore child permissions");
    assert_mode(&file, 0o600);
}

#[test]
fn recursive_permissions_reject_non_directories() {
    let temp = tempdir().expect("create temp dir");
    let file = temp.path().join("file.txt");
    fs::write(&file, "content").expect("write file");

    let error = write_file_properties_permissions_to_enclosed_items(
        file.clone(),
        FilePropertiesPermissions::from_mode(0o644),
    )
    .expect_err("file recursive permissions should fail");

    assert!(error.contains("item is not a folder"));
    assert!(error.contains(file.to_string_lossy().as_ref()));
}

fn assert_mode(path: &Path, expected_mode: u32) {
    let mode = fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode, expected_mode, "unexpected mode for {:?}", path);
}

#[test]
fn a_single_folder_counts_everything_inside_not_only_its_first_level() {
    let temp = tempdir().expect("create temp dir");
    let nested = temp.path().join("child").join("grandchild");
    fs::create_dir_all(&nested).expect("create nested directories");
    fs::write(nested.join("deep.txt"), b"deep").expect("write deep file");
    let request = FilePropertiesRequest {
        targets: crate::model::FilePropertiesTargetSet::single(temp.path().to_path_buf()),
        generation: 1,
    };
    let (mut output, _receiver) = iced::futures::channel::mpsc::channel(64);

    let contents = block_on(load_directory_properties_contents(
        request,
        &CancellationToken::new(),
        &mut output,
    ))
    .expect("directory contents");

    assert_eq!((contents.file_count, contents.directory_count), (1, 2));
    assert!(contents.total_size_bytes >= 4);
}

#[test]
fn a_file_is_read_with_its_type_owner_and_disk_and_the_slow_parts_queued() {
    let temp = tempdir().expect("create temp dir");
    let path = temp.path().join("notes.txt");
    fs::write(&path, b"hello").expect("write file");

    let (snapshot, lookups) = read_file_properties(path.clone()).expect("file properties");

    let FilePropertiesKindDetails::File(file) = &snapshot.details else {
        panic!("a regular file is a file");
    };
    assert_eq!(file.mime_type, "text/plain");
    assert_eq!(file.default_application, FilePropertiesLookup::Pending);
    assert_eq!(file.media_details, None);
    assert_eq!(
        lookups.default_application_for.as_deref(),
        Some("text/plain")
    );
    assert_eq!(lookups.media, None);
    assert!(!lookups.directory_contents);
    let metadata = fs::metadata(&path).expect("stat file");
    assert_eq!(snapshot.owner.user, file_core::user_name(metadata.uid()));
    assert_eq!(snapshot.owner.group, file_core::group_name(metadata.gid()));
    let volume = snapshot.volume.expect("the temp dir is on a mounted disk");
    assert!(temp
        .path()
        .canonicalize()
        .unwrap()
        .starts_with(&volume.mount_point));
}

#[test]
fn media_files_queue_their_details_and_folders_their_contents() {
    let temp = tempdir().expect("create temp dir");
    let picture = temp.path().join("photo.png");
    fs::write(&picture, b"not really a png").expect("write picture");

    let (snapshot, lookups) = read_file_properties(picture).expect("picture properties");
    let FilePropertiesKindDetails::File(file) = &snapshot.details else {
        panic!("a regular file is a file");
    };
    assert_eq!(file.media_details, Some(FilePropertiesLookup::Pending));
    assert!(matches!(
        lookups.media,
        Some(file_core::MediaByName::Image(_))
    ));

    let (snapshot, lookups) =
        read_file_properties(temp.path().to_path_buf()).expect("folder properties");
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loading(None))
    ));
    assert!(lookups.directory_contents);
    assert_eq!(lookups.default_application_for, None);
}

#[test]
fn a_link_shows_where_it_points_without_following_it() {
    let temp = tempdir().expect("create temp dir");
    let link = temp.path().join("link");
    symlink("/nonexistent/target", &link).expect("create symlink");

    let (snapshot, lookups) = read_file_properties(link).expect("link properties");

    assert!(matches!(
        &snapshot.details,
        FilePropertiesKindDetails::Symlink(Ok(target)) if target == Path::new("/nonexistent/target")
    ));
    assert_eq!(snapshot.permissions, None);
    assert!(snapshot.volume.is_ok());
    assert_eq!(lookups.default_application_for, None);
}

/// 重读同一个条目：依据没变的慢项沿用、不再读，变了的再读；按了刷新都再读，但先摆着上次的。
#[test]
fn a_reread_keeps_what_did_not_change_and_reads_the_rest_again() {
    let temp = tempdir().expect("create temp dir");
    let picture = temp.path().join("photo.png");
    fs::write(&picture, b"not really a png").expect("write picture");
    let read = || read_file_properties(picture.clone()).expect("picture properties");
    let viewer = FilePropertiesLookup::Ready(Some("Viewer".to_owned()));
    let (mut previous, _) = read();
    let FilePropertiesKindDetails::File(file) = &mut previous.details else {
        panic!("a regular file is a file");
    };
    file.default_application = viewer.clone();
    file.media_details = Some(FilePropertiesLookup::Ready(Vec::new()));
    let reread = |reread| {
        let (mut snapshot, mut lookups) = read();
        keep_previous_slow_properties(&mut snapshot, &mut lookups, &previous, reread);
        let FilePropertiesKindDetails::File(file) = snapshot.details else {
            panic!("a regular file is a file");
        };
        (file, lookups)
    };

    // 只是权限、时间这类变了：类型、大小、修改时间都没变，两项沿用、不再读。
    let (file, lookups) = reread(FilePropertiesReread::Item);
    assert_eq!(file.default_application, viewer);
    assert_eq!(
        file.media_details,
        Some(FilePropertiesLookup::Ready(Vec::new()))
    );
    assert_eq!(lookups.default_application_for, None);
    assert_eq!(lookups.media, None);

    // 按了刷新：先摆着上次的，照样再读。
    let (file, lookups) = reread(FilePropertiesReread::All);
    assert_eq!(file.default_application, viewer);
    assert!(lookups.default_application_for.is_some());
    assert!(lookups.media.is_some());

    // 内容变了：详细信息再读，打开方式只看类型，不用再问。
    fs::write(&picture, b"not really a png, only longer").expect("rewrite picture");
    let (file, lookups) = reread(FilePropertiesReread::Item);
    assert_eq!(file.default_application, viewer);
    assert_eq!(lookups.default_application_for, None);
    assert!(lookups.media.is_some());
}

/// 文件夹：目标本身变了不重量，大小还是上次量的；里面的东西变了重量，量完之前显示上次的。
#[test]
fn a_reread_folder_is_measured_again_only_when_its_contents_changed() {
    let temp = tempdir().expect("create temp dir");
    let (mut previous, _) =
        read_file_properties(temp.path().to_path_buf()).expect("folder properties");
    previous.details = FilePropertiesKindDetails::Directory(
        FilePropertiesDirectoryContentsState::Loaded(FilePropertiesDirectoryContents {
            file_count: 3,
            directory_count: 1,
            total_size_bytes: 100,
            total_disk_size_bytes: 8192,
            unreadable_entries: 0,
        }),
    );
    let reread = |reread| {
        let (mut snapshot, mut lookups) =
            read_file_properties(temp.path().to_path_buf()).expect("folder properties");
        keep_previous_slow_properties(&mut snapshot, &mut lookups, &previous, reread);
        (snapshot, lookups)
    };

    let (snapshot, lookups) = reread(FilePropertiesReread::Item);
    assert!(!lookups.directory_contents);
    assert_eq!(snapshot.size_bytes, 100);
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loaded(
            FilePropertiesDirectoryContents { file_count: 3, .. }
        ))
    ));

    let (snapshot, lookups) = reread(FilePropertiesReread::Contents);
    assert!(lookups.directory_contents);
    assert_eq!(snapshot.disk_size_bytes, 8192);
    assert!(matches!(
        snapshot.details,
        FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Refreshing(
            FilePropertiesDirectoryContents { file_count: 3, .. }
        ))
    ));
}
