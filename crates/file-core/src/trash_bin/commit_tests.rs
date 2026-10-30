use std::os::unix::fs::{symlink, PermissionsExt};

use tempfile::{tempdir, TempDir};

use super::super::catalog::discover_trash_locations_from_mountinfo;
use super::super::scan::scan_trash_with_catalog;
use super::*;
use crate::{restore_trash_entry, ScanOptions, TransferConflictStrategy};

struct Fixture {
    _directory: TempDir,
    root: PathBuf,
    volume: PathBuf,
    context: TrashCommitContext,
}

/// 假 mountinfo：`/` 装着 home，`root/volume` 是另一个挂载（其实同一真实文件系统，
/// 所以 rename 照常能做）。
fn fixture(volume_fs_type: &str, volume_device: &str) -> Fixture {
    let directory = tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let volume = root.join("volume");
    fs::create_dir(&volume).unwrap();
    let mount = |mount_point: &Path, fs_type: &str, device: &str| MountTableEntry {
        mount_point: mount_point.to_path_buf(),
        fs_type: fs_type.into(),
        device: device.into(),
    };
    let context = TrashCommitContext {
        data_home: root.join("data"),
        uid: effective_user_id(),
        mounts: vec![
            mount(Path::new("/"), "ext4", "/dev/root"),
            mount(&volume, volume_fs_type, volume_device),
        ],
    };
    Fixture {
        _directory: directory,
        root,
        volume,
        context,
    }
}

impl Fixture {
    fn trash(&self, path: &Path) -> Result<TrashRestoreEntry, FileError> {
        self.context.trash_path(path, &CancellationToken::new())
    }

    /// 扫描到的条目路径。目录身份里的时间戳会随放入变化，还原只认目录是不是同一个，
    /// 所以这里比路径，确切身份由随后的还原来验。
    fn scanned_paths(&self) -> Vec<[PathBuf; 3]> {
        let mountinfo = format!(
            "1 0 8:1 / {} rw - ext4 /dev/test rw\n",
            self.volume.display()
        );
        let catalog = discover_trash_locations_from_mountinfo(
            &self.context.data_home,
            self.context.uid,
            mountinfo.as_bytes(),
        )
        .unwrap();
        scan_trash_with_catalog(
            ScanOptions {
                include_hidden: true,
                ..ScanOptions::default()
            },
            CancellationToken::new(),
            catalog,
        )
        .unwrap()
        .entries
        .iter()
        .map(|entry| paths(&entry.restore_entry()))
        .collect()
    }
}

fn paths(entry: &TrashRestoreEntry) -> [PathBuf; 3] {
    [
        entry.trash_path.clone(),
        entry.info_path.clone(),
        entry.original_path.clone(),
    ]
}

fn write_file(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"payload").unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[tokio::test]
async fn home_entry_matches_disk_and_scan_and_restores() {
    let fixture = fixture("ext4", "/dev/test");
    let source = fixture.root.join("report.txt");
    write_file(&source);

    let entry = fixture.trash(&source).unwrap();

    let trash_root = fixture.context.data_home.join("Trash");
    assert_eq!(entry.trash_path, trash_root.join("files/report.txt"));
    assert_eq!(
        entry.info_path,
        trash_root.join("info/report.txt.trashinfo")
    );
    assert_eq!(entry.original_path, source);
    assert!(!source.exists());
    let info = fs::read_to_string(&entry.info_path).unwrap();
    assert!(info.starts_with(&format!(
        "[Trash Info]\nPath={}\nDeletionDate=",
        source.display()
    )));
    assert_eq!(mode(&entry.info_path), 0o600);
    assert_eq!(mode(&trash_root), 0o700);
    assert_eq!(fixture.scanned_paths(), [paths(&entry)]);

    let restored = restore_trash_entry(entry, TransferConflictStrategy::Fail)
        .await
        .unwrap();
    assert_eq!(restored, source);
    assert_eq!(fs::read(&source).unwrap(), b"payload");
}

#[test]
fn same_names_are_numbered_like_glib() {
    let fixture = fixture("ext4", "/dev/test");
    let trashed_names = |name: &str| {
        (0..3)
            .map(|_| {
                let source = fixture.root.join(name);
                write_file(&source);
                fixture
                    .trash(&source)
                    .unwrap()
                    .trash_path
                    .file_name()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        trashed_names("a.tar.gz"),
        ["a.tar.gz", "a.2.tar.gz", "a.3.tar.gz"]
    );
    assert_eq!(trashed_names(".env"), [".env", ".2.env", ".3.env"]);
    assert_eq!(trashed_names("noext"), ["noext", "noext.2", "noext.3"]);
    assert_eq!(fixture.scanned_paths().len(), 9);
}

#[test]
fn names_taken_in_files_or_info_are_skipped_without_touching_them() {
    let fixture = fixture("ext4", "/dev/test");
    let trash_root = fixture.context.data_home.join("Trash");
    fs::create_dir(&fixture.context.data_home).unwrap();
    create_trash_directories(&trash_root).unwrap();
    fs::write(trash_root.join("files/a.txt"), b"orphan payload").unwrap();
    fs::write(trash_root.join("info/a.2.txt.trashinfo"), b"orphan info").unwrap();
    let source = fixture.root.join("a.txt");
    write_file(&source);

    let entry = fixture.trash(&source).unwrap();

    assert_eq!(entry.trash_path, trash_root.join("files/a.3.txt"));
    assert_eq!(
        fs::read(trash_root.join("files/a.txt")).unwrap(),
        b"orphan payload"
    );
    assert_eq!(
        fs::read(trash_root.join("info/a.2.txt.trashinfo")).unwrap(),
        b"orphan info"
    );
    assert!(!trash_root.join("info/a.txt.trashinfo").exists());
}

#[tokio::test]
async fn volume_uses_the_sticky_shared_trash_with_a_relative_path() {
    let fixture = fixture("ext4", "/dev/test");
    let shared = fixture.volume.join(".Trash");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    let source = fixture.volume.join("folder/item.txt");
    write_file(&source);

    let entry = fixture.trash(&source).unwrap();

    let user_root = shared.join(fixture.context.uid.to_string());
    assert_eq!(entry.trash_path, user_root.join("files/item.txt"));
    let info = fs::read_to_string(&entry.info_path).unwrap();
    assert!(info.contains("\nPath=folder/item.txt\n"));
    assert_eq!(fixture.scanned_paths(), [paths(&entry)]);
    let restored = restore_trash_entry(entry, TransferConflictStrategy::Fail)
        .await
        .unwrap();
    assert_eq!(restored, source);
}

#[test]
fn volume_without_a_sticky_shared_trash_uses_the_private_trash() {
    let fixture = fixture("ext4", "/dev/test");
    fs::create_dir(fixture.volume.join(".Trash")).unwrap();
    fs::set_permissions(
        fixture.volume.join(".Trash"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let source = fixture.volume.join("item.txt");
    write_file(&source);

    let entry = fixture.trash(&source).unwrap();

    let private_root = fixture
        .volume
        .join(format!(".Trash-{}", fixture.context.uid));
    assert_eq!(entry.trash_path, private_root.join("files/item.txt"));
    assert_eq!(mode(&private_root), 0o700);
    assert_eq!(fixture.scanned_paths(), [paths(&entry)]);
}

#[test]
fn volume_that_cannot_hold_a_trash_is_unavailable_and_left_alone() {
    let fixture = fixture("ext4", "/dev/test");
    let source = fixture.volume.join("item.txt");
    write_file(&source);
    fs::set_permissions(&fixture.volume, fs::Permissions::from_mode(0o555)).unwrap();

    let error = fixture.trash(&source).unwrap_err();

    fs::set_permissions(&fixture.volume, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(error, FileError::TrashUnavailable { .. }),
        "{error}"
    );
    assert!(source.exists());
    assert_eq!(fs::read_dir(&fixture.volume).unwrap().count(), 1);
}

#[test]
fn system_internal_mount_is_unavailable_and_nothing_is_created() {
    let fixture = fixture("tmpfs", "tmpfs");
    let source = fixture.volume.join("item.txt");
    write_file(&source);

    let error = fixture.trash(&source).unwrap_err();

    assert!(
        matches!(&error, FileError::TrashUnavailable { reason, .. } if reason.contains("system internal")),
        "{error}"
    );
    assert!(source.exists());
    assert_eq!(fs::read_dir(&fixture.volume).unwrap().count(), 1);
}

#[test]
fn cancelled_request_moves_nothing() {
    let fixture = fixture("ext4", "/dev/test");
    let source = fixture.root.join("item.txt");
    write_file(&source);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = fixture
        .context
        .trash_path(&source, &cancellation)
        .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(source.exists());
    assert!(!fixture.context.data_home.exists());
}

#[test]
fn rename_without_noreplace_support_still_never_replaces() {
    let fixture = tempdir().unwrap();
    let source = fixture.path().join("source");
    let target = fixture.path().join("target");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();

    assert!(matches!(
        rename_if_absent(&source, &target),
        Err(NoReplaceRenameError::TargetExists)
    ));
    assert_eq!(fs::read(&target).unwrap(), b"target");

    fs::remove_file(&target).unwrap();
    rename_if_absent(&source, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"source");
}

#[test]
fn source_canonicalizes_the_parent_without_following_the_leaf() {
    let fixture = tempdir().unwrap();
    let actual_parent = fs::canonicalize(fixture.path()).unwrap().join("actual");
    fs::create_dir(&actual_parent).unwrap();
    let parent_link = fixture.path().join("parent-link");
    symlink(&actual_parent, &parent_link).unwrap();
    let leaf = actual_parent.join("leaf-link");
    symlink(fixture.path().join("external-target"), &leaf).unwrap();

    assert_eq!(
        canonical_trash_source_path(&parent_link.join("leaf-link")).unwrap(),
        leaf
    );
    assert!(canonical_trash_source_path(Path::new("relative")).is_err());
}

#[test]
fn home_mount_follows_a_symlinked_data_home_and_the_deepest_mount() {
    let fixture = tempdir().unwrap();
    let physical = fs::canonicalize(fixture.path()).unwrap().join("physical");
    fs::create_dir(&physical).unwrap();
    let linked = fixture.path().join("linked");
    symlink(&physical, &linked).unwrap();
    assert_eq!(
        canonicalize_path_or_existing_parent(&linked.join("data/Trash")).unwrap(),
        physical.join("data/Trash")
    );

    let mount = |mount_point: &str| MountTableEntry {
        mount_point: PathBuf::from(mount_point),
        fs_type: "ext4".into(),
        device: "/dev/sda1".into(),
    };
    let mounts = [mount("/"), mount("/srv/home-bind")];
    let mount_point = |path: &str| {
        mount_containing(&mounts, Path::new(path)).map(|mount| mount.mount_point.clone())
    };
    assert_eq!(
        mount_point("/srv/home-bind/user/item"),
        Some(PathBuf::from("/srv/home-bind"))
    );
    assert_eq!(
        mount_point("/home/user/.local/share"),
        Some(PathBuf::from("/"))
    );
}
