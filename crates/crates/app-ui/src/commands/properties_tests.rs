use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use tempfile::tempdir;

use super::*;

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

    let snapshot =
        read_aggregate_file_properties(&[directory, file], CancellationToken::new(), |_| {})
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

    let error = read_aggregate_file_properties(&[target], cancellation, |_| {})
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

    let snapshot =
        read_aggregate_file_properties(&[target, link], CancellationToken::new(), |_| {})
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
