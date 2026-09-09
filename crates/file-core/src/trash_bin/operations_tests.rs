use std::os::unix::fs::PermissionsExt;

use tempfile::tempdir;

use super::super::catalog::discover_trash_locations_from_mountinfo;
use super::super::scan::scan_trash_with_catalog;
use super::*;

fn create_location(root: &Path) {
    fs::create_dir_all(root.join("files")).unwrap();
    fs::create_dir_all(root.join("info")).unwrap();
    for path in [root.to_path_buf(), root.join("files"), root.join("info")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn write_entry(root: &Path, name: &str, original_path: &str, payload: &[u8]) {
    fs::write(root.join("files").join(name), payload).unwrap();
    fs::write(
        root.join("info").join(format!("{name}.trashinfo")),
        format!("[Trash Info]\nPath={original_path}\nDeletionDate=2026-08-02T12:00:00\n"),
    )
    .unwrap();
}

fn scan_home_entries(fixture: &Path) -> Vec<super::super::model::TrashEntry> {
    let data_home = fixture.join("data");
    let catalog =
        discover_trash_locations_from_mountinfo(&data_home, effective_user_id(), b"").unwrap();
    scan_trash_with_catalog(ScanOptions::default(), CancellationToken::new(), catalog)
        .unwrap()
        .entries
}

fn scan_single_entry(fixture: &Path) -> TrashRestoreEntry {
    let data_home = fixture.join("data");
    let root = data_home.join("Trash");
    create_location(&root);
    let original = fixture.join("original.txt");
    write_entry(&root, "item", &original.display().to_string(), b"payload");
    scan_home_entries(fixture).remove(0).restore_entry()
}

#[cfg(unix)]
#[test]
fn trash_source_canonicalizes_the_parent_without_following_the_leaf() {
    use std::os::unix::fs::symlink;

    let fixture = tempdir().unwrap();
    let actual_parent = fixture.path().join("actual");
    fs::create_dir(&actual_parent).unwrap();
    let parent_link = fixture.path().join("parent-link");
    symlink(&actual_parent, &parent_link).unwrap();
    let external_target = fixture.path().join("external-target");
    fs::write(&external_target, b"target").unwrap();
    let leaf = actual_parent.join("leaf-link");
    symlink(&external_target, &leaf).unwrap();

    assert_eq!(
        canonical_trash_source_path(&parent_link.join("leaf-link")).unwrap(),
        leaf
    );
}

#[test]
fn home_trash_mount_classification_resolves_symlinked_existing_parent() {
    use std::os::unix::fs::symlink;

    let fixture = tempdir().unwrap();
    let physical = fixture.path().join("physical");
    fs::create_dir(&physical).unwrap();
    let linked = fixture.path().join("linked");
    symlink(&physical, &linked).unwrap();

    assert_eq!(
        canonicalize_path_or_existing_parent(&linked.join("data/Trash")).unwrap(),
        physical.join("data/Trash")
    );
}

#[test]
fn deepest_mount_point_distinguishes_a_same_device_bind_from_home() {
    let mount_points = vec![PathBuf::from("/"), PathBuf::from("/srv/home-bind")];

    assert_eq!(
        deepest_mount_point(&mount_points, Path::new("/srv/home-bind/user/item")),
        Some(PathBuf::from("/srv/home-bind"))
    );
    assert_eq!(
        deepest_mount_point(&mount_points, Path::new("/home/user/.local/share")),
        Some(PathBuf::from("/"))
    );
}

#[tokio::test]
async fn restore_and_delete_reject_historical_entries_without_side_effects() {
    let fixture = tempdir().unwrap();
    let payload = fixture.path().join("payload");
    let info = fixture.path().join("info");
    fs::write(&payload, b"payload").unwrap();
    fs::write(&info, b"info").unwrap();
    let historical = TrashRestoreEntry::from_historical_paths(
        payload.clone(),
        info.clone(),
        fixture.path().join("original"),
    );

    assert!(delete_trash_entry(historical.clone()).await.is_err());
    assert!(
        restore_trash_entry(historical, TransferConflictStrategy::Fail)
            .await
            .is_err()
    );
    assert!(payload.exists());
    assert!(info.exists());
}

#[tokio::test]
async fn payload_replacement_is_rejected_before_permanent_delete() {
    let fixture = tempdir().unwrap();
    let entry = scan_single_entry(fixture.path());
    fs::remove_file(&entry.trash_path).unwrap();
    fs::write(&entry.trash_path, b"replacement").unwrap();

    let error = delete_trash_entry(entry.clone()).await.unwrap_err();

    assert!(error.to_string().contains("identity changed"));
    assert_eq!(fs::read(&entry.trash_path).unwrap(), b"replacement");
    assert!(entry.info_path.exists());
}

#[tokio::test]
async fn verified_entry_can_be_deleted_with_its_info_file() {
    let fixture = tempdir().unwrap();
    let entry = scan_single_entry(fixture.path());

    delete_trash_entry(entry.clone()).await.unwrap();

    assert!(!entry.trash_path.exists());
    assert!(!entry.info_path.exists());
}

#[tokio::test]
async fn shared_and_private_same_name_entries_restore_and_delete_exact_objects() {
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let volume = fixture.path().join("volume");
    fs::create_dir(&volume).unwrap();
    let shared = volume.join(".Trash");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    let shared_user = shared.join(effective_user_id().to_string());
    let private = volume.join(format!(".Trash-{}", effective_user_id()));
    create_location(&shared_user);
    create_location(&private);
    write_entry(&shared_user, "same", "restored/shared/same", b"shared");
    write_entry(&private, "same", "restored/private/same", b"private");
    let mountinfo = format!("1 0 8:1 / {} rw - ext4 /dev/test rw\n", volume.display());
    let catalog = discover_trash_locations_from_mountinfo(
        &data_home,
        effective_user_id(),
        mountinfo.as_bytes(),
    )
    .unwrap();
    let mut entries =
        scan_trash_with_catalog(ScanOptions::default(), CancellationToken::new(), catalog)
            .unwrap()
            .entries;
    assert_eq!(entries.len(), 2);
    let ordered_paths = entries
        .iter()
        .map(|entry| entry.trash_path.clone())
        .collect::<Vec<_>>();
    let mut expected_paths = ordered_paths.clone();
    expected_paths.sort_unstable();
    assert_eq!(ordered_paths, expected_paths);
    let shared_entry = entries
        .iter()
        .position(|entry| entry.original_path == volume.join("restored/shared/same"))
        .map(|index| entries.remove(index))
        .unwrap();
    let private_entry = entries.remove(0);

    let restored =
        restore_trash_entry(shared_entry.restore_entry(), TransferConflictStrategy::Fail)
            .await
            .unwrap();
    assert_eq!(restored, volume.join("restored/shared/same"));
    assert_eq!(fs::read(&restored).unwrap(), b"shared");
    assert_eq!(fs::read(&private_entry.trash_path).unwrap(), b"private");

    delete_trash_entry(private_entry.restore_entry())
        .await
        .unwrap();
    assert!(!private.join("files/same").exists());
    assert!(!private.join("info/same.trashinfo").exists());
}

#[tokio::test]
async fn replaced_trash_root_is_rejected_without_touching_either_tree() {
    let fixture = tempdir().unwrap();
    let entry = scan_single_entry(fixture.path());
    let root = fixture.path().join("data/Trash");
    let displaced = fixture.path().join("displaced-trash");
    fs::rename(&root, &displaced).unwrap();
    create_location(&root);

    let error = delete_trash_entry(entry).await.unwrap_err();

    assert!(error.to_string().contains("identity changed"));
    assert_eq!(fs::read(displaced.join("files/item")).unwrap(), b"payload");
    assert!(root.join("files").read_dir().unwrap().next().is_none());
}

#[tokio::test]
async fn empty_trash_continues_after_identity_failure_and_reports_partial_completion() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("data/Trash");
    create_location(&root);
    write_entry(
        &root,
        "a-valid",
        &fixture.path().join("valid.txt").display().to_string(),
        b"valid",
    );
    write_entry(
        &root,
        "b-replaced",
        &fixture.path().join("replaced.txt").display().to_string(),
        b"original",
    );
    let entries = scan_home_entries(fixture.path());
    fs::remove_file(root.join("files/b-replaced")).unwrap();
    fs::write(root.join("files/b-replaced"), b"foreign").unwrap();

    let error = empty_verified_trash_scan(
        super::super::model::TrashScan {
            entries,
            skipped: Vec::new(),
        },
        CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("partially emptied"));
    assert!(!root.join("files/a-valid").exists());
    assert!(!root.join("info/a-valid.trashinfo").exists());
    assert_eq!(fs::read(root.join("files/b-replaced")).unwrap(), b"foreign");
    assert!(root.join("info/b-replaced.trashinfo").exists());
}

#[tokio::test]
async fn empty_trash_does_not_report_advisory_metadata_warning_as_failure() {
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let root = data_home.join("Trash");
    create_location(&root);
    fs::write(root.join("files/item"), b"payload").unwrap();
    fs::write(
        root.join("info/item.trashinfo"),
        format!(
            "[Trash Info]\nPath={}\nDeletionDate=invalid\n",
            fixture.path().join("original.txt").display()
        ),
    )
    .unwrap();
    let catalog =
        discover_trash_locations_from_mountinfo(&data_home, effective_user_id(), b"").unwrap();
    let scan =
        scan_trash_with_catalog(ScanOptions::default(), CancellationToken::new(), catalog).unwrap();
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.skipped.len(), 1);

    empty_verified_trash_scan(scan, CancellationToken::new())
        .await
        .unwrap();

    assert!(!root.join("files/item").exists());
    assert!(!root.join("info/item.trashinfo").exists());
}

#[tokio::test]
async fn batch_delete_rejects_replaced_payload_and_deletes_the_rest() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("data/Trash");
    create_location(&root);
    write_entry(
        &root,
        "a-valid",
        &fixture.path().join("valid.txt").display().to_string(),
        b"valid",
    );
    write_entry(
        &root,
        "b-replaced",
        &fixture.path().join("replaced.txt").display().to_string(),
        b"original",
    );
    let mut entries = scan_home_entries(fixture.path());
    assert_eq!(entries.len(), 2);
    let replaced_index = entries
        .iter()
        .position(|entry| entry.trash_path == root.join("files/b-replaced"))
        .unwrap();
    let replaced = entries.remove(replaced_index).restore_entry();
    let valid = entries.remove(0).restore_entry();
    fs::remove_file(&replaced.trash_path).unwrap();
    fs::write(&replaced.trash_path, b"foreign").unwrap();

    let batch = super::super::batch::TrashVerificationBatch::new();
    batch.delete_entry(valid).await.unwrap();
    let error = batch.delete_entry(replaced.clone()).await.unwrap_err();

    assert!(error.to_string().contains("identity changed"));
    assert_eq!(fs::read(&replaced.trash_path).unwrap(), b"foreign");
    assert!(replaced.info_path.exists());
    assert!(!root.join("files/a-valid").exists());
    assert!(!root.join("info/a-valid.trashinfo").exists());
}

#[tokio::test]
async fn batch_restore_restores_every_entry_of_the_batch() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("data/Trash");
    create_location(&root);
    let first_original = fixture.path().join("first.txt");
    let second_original = fixture.path().join("second.txt");
    write_entry(&root, "first", &first_original.display().to_string(), b"first");
    write_entry(&root, "second", &second_original.display().to_string(), b"second");
    let entries = scan_home_entries(fixture.path());
    assert_eq!(entries.len(), 2);

    let batch = super::super::batch::TrashVerificationBatch::new();
    for entry in entries {
        let restored = batch
            .restore_entry(entry.restore_entry(), TransferConflictStrategy::Fail)
            .await
            .unwrap();
        assert!(restored == first_original || restored == second_original);
    }

    assert_eq!(fs::read(&first_original).unwrap(), b"first");
    assert_eq!(fs::read(&second_original).unwrap(), b"second");
    assert!(!root.join("files/first").exists());
    assert!(!root.join("files/second").exists());
    assert!(!root.join("info/first.trashinfo").exists());
    assert!(!root.join("info/second.trashinfo").exists());
}

#[test]
fn tracking_requires_an_absolute_source_before_any_trash_side_effect() {
    let error = canonical_trash_source_path(Path::new("relative")).unwrap_err();
    assert!(error.to_string().contains("absolute source path"));
}

#[test]
fn tracking_ignores_preexisting_matching_entry_after_directory_metadata_changes() {
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let root = data_home.join("Trash");
    create_location(&root);
    let original = fixture.path().join("original.txt");
    let original_text = original.display().to_string();
    write_entry(&root, "original.txt", &original_text, b"existing");
    let existing = scan_home_entries(fixture.path()).remove(0);
    let existing_identity = existing.identity.clone().unwrap();
    let plan = TrashTrackingPlan {
        original_path: original,
        scope: TrashTrackingScope::Home,
        data_home,
        uid: effective_user_id(),
        mountinfo: Vec::new(),
        before_info_objects: vec![existing_identity.info.clone()],
    };
    write_entry(&root, "original.txt.2", &original_text, b"new");

    let tracked = plan.find_committed_entry().unwrap();

    assert_eq!(tracked.trash_path, root.join("files/original.txt.2"));
}

#[test]
fn post_commit_tracking_rejects_ambiguous_new_entries() {
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let root = data_home.join("Trash");
    create_location(&root);
    let original = fixture.path().join("original.txt");
    let plan = TrashTrackingPlan {
        original_path: original.clone(),
        scope: TrashTrackingScope::Home,
        data_home: data_home.clone(),
        uid: effective_user_id(),
        mountinfo: Vec::new(),
        before_info_objects: Vec::new(),
    };
    let original_text = original.display().to_string();
    write_entry(&root, "original.txt", &original_text, b"first");
    write_entry(&root, "original.txt.2", &original_text, b"second");

    let error = plan.find_committed_entry().unwrap_err();

    assert!(error.contains("2 matching entries"));
}
