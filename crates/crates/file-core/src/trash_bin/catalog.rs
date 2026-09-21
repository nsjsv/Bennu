use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(all(unix, test))]
use std::os::unix::fs::PermissionsExt;

use crate::{FileError, ScanWarning};

use super::model::{
    OriginalPathBase, TrashLocationGuard, TrashLocationKind, TrashObjectIdentity, TrashObjectKind,
    VerifiedTrashDirectory,
};
use super::mountinfo::{mounts_candidate_for_trash_probing, parse_mountinfo, MOUNTINFO_PATH};
use crate::mount_table::MountTableEntry;

#[derive(Debug)]
pub(super) struct TrashLocationCatalog {
    pub locations: Vec<TrashLocationGuard>,
    pub warnings: Vec<ScanWarning>,
}

#[derive(Debug, Clone, Copy)]
enum DirectoryExpectation {
    MountTop,
    SharedRoot { device: u64 },
    OwnedTrash { uid: u32, device: Option<u64> },
}

pub(super) fn discover_trash_locations(
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TrashLocationCatalog, FileError> {
    check_cancellation(cancellation)?;
    let data_home = trash_data_home()?;
    discover_trash_locations_from_mountinfo_read(
        &data_home,
        effective_user_id(),
        fs::read(MOUNTINFO_PATH),
        cancellation,
    )
}

fn discover_trash_locations_from_mountinfo_read(
    data_home: &Path,
    uid: u32,
    mountinfo: io::Result<Vec<u8>>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TrashLocationCatalog, FileError> {
    check_cancellation(cancellation)?;
    match mountinfo {
        Ok(mountinfo) => discover_trash_locations_from_mountinfo_with_cancellation(
            data_home,
            uid,
            &mountinfo,
            cancellation,
        ),
        Err(source) => {
            let mut catalog =
                discover_trash_locations_from_mount_points(data_home, uid, &[], cancellation)?;
            if catalog.locations.is_empty() {
                return Err(FileError::ReadDirectory {
                    path: PathBuf::from(MOUNTINFO_PATH),
                    source,
                });
            }
            catalog.warnings.push(ScanWarning {
                path: PathBuf::from(MOUNTINFO_PATH),
                message: source.to_string(),
            });
            Ok(catalog)
        }
    }
}

pub(super) fn discover_trash_locations_from_mountinfo(
    data_home: &Path,
    uid: u32,
    mountinfo: &[u8],
) -> Result<TrashLocationCatalog, FileError> {
    discover_trash_locations_from_mountinfo_with_cancellation(
        data_home,
        uid,
        mountinfo,
        &tokio_util::sync::CancellationToken::new(),
    )
}

pub(super) fn discover_trash_locations_from_mountinfo_with_cancellation(
    data_home: &Path,
    uid: u32,
    mountinfo: &[u8],
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TrashLocationCatalog, FileError> {
    check_cancellation(cancellation)?;
    let snapshot = parse_mountinfo(mountinfo);
    let mut catalog =
        discover_trash_locations_from_mount_points(data_home, uid, &snapshot.mounts, cancellation)?;
    catalog.warnings.splice(0..0, snapshot.warnings);
    Ok(catalog)
}

fn discover_trash_locations_from_mount_points(
    data_home: &Path,
    uid: u32,
    mounts: &[MountTableEntry],
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TrashLocationCatalog, FileError> {
    check_cancellation(cancellation)?;
    let mut locations = Vec::new();
    let mut warnings = Vec::new();

    let home_root = data_home.join("Trash");
    if let Some(location) = probe_home_location(data_home, &home_root, uid, &mut warnings) {
        locations.push(location);
    }

    for entry in mounts
        .iter()
        .filter(|mount| mounts_candidate_for_trash_probing(mount))
    {
        let top_directory = &entry.mount_point;
        check_cancellation(cancellation)?;
        let shared_root_path = top_directory.join(".Trash");
        let private_root = top_directory.join(format!(".Trash-{uid}"));
        let shared_candidate = trash_candidate_exists(&shared_root_path, &mut warnings);
        let private_candidate = trash_candidate_exists(&private_root, &mut warnings);
        if !shared_candidate && !private_candidate {
            continue;
        }

        let top = match inspect_directory(top_directory, DirectoryExpectation::MountTop) {
            Ok(top) => top,
            Err(problem) => {
                warnings.push(problem);
                continue;
            }
        };
        if shared_candidate {
            match inspect_directory(
                &shared_root_path,
                DirectoryExpectation::SharedRoot {
                    device: top.identity.device,
                },
            ) {
                Ok(shared_root) => {
                    let user_root = shared_root_path.join(uid.to_string());
                    if let Some(location) = probe_volume_location(
                        TrashLocationKind::SharedVolume,
                        top_directory,
                        &top,
                        Some(shared_root),
                        &user_root,
                        uid,
                        &mut warnings,
                    ) {
                        locations.push(location);
                    }
                }
                Err(problem) => warnings.push(problem),
            }
        }

        if private_candidate {
            if let Some(location) = probe_volume_location(
                TrashLocationKind::PrivateVolume,
                top_directory,
                &top,
                None,
                &private_root,
                uid,
                &mut warnings,
            ) {
                locations.push(location);
            }
        }
    }

    let mut seen = HashSet::new();
    locations.retain(|location| {
        seen.insert((
            location.files.identity.device,
            location.files.identity.inode,
            location.info.identity.device,
            location.info.identity.inode,
        ))
    });

    Ok(TrashLocationCatalog {
        locations,
        warnings,
    })
}

fn check_cancellation(cancellation: &tokio_util::sync::CancellationToken) -> Result<(), FileError> {
    if cancellation.is_cancelled() {
        Err(FileError::Cancelled)
    } else {
        Ok(())
    }
}

fn trash_candidate_exists(path: &Path, warnings: &mut Vec<ScanWarning>) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            false
        }
        Err(error) => {
            warnings.push(io_warning(path, error));
            false
        }
    }
}

fn probe_home_location(
    data_home: &Path,
    root_path: &Path,
    uid: u32,
    warnings: &mut Vec<ScanWarning>,
) -> Option<TrashLocationGuard> {
    let root = match probe_owned_root(root_path, uid, None, warnings)? {
        Ok(root) => root,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };
    let files = match inspect_directory(
        &root_path.join("files"),
        DirectoryExpectation::OwnedTrash {
            uid,
            device: Some(root.identity.device),
        },
    ) {
        Ok(directory) => directory,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };
    let info = match inspect_directory(
        &root_path.join("info"),
        DirectoryExpectation::OwnedTrash {
            uid,
            device: Some(root.identity.device),
        },
    ) {
        Ok(directory) => directory,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };

    Some(TrashLocationGuard {
        kind: TrashLocationKind::Home,
        top_directory: data_home.to_path_buf(),
        top_identity: None,
        shared_root: None,
        trash_root: root,
        files,
        info,
        original_path_base: OriginalPathBase::Absolute,
    })
}

fn probe_volume_location(
    kind: TrashLocationKind,
    top_directory: &Path,
    top: &VerifiedTrashDirectory,
    shared_root: Option<VerifiedTrashDirectory>,
    root_path: &Path,
    uid: u32,
    warnings: &mut Vec<ScanWarning>,
) -> Option<TrashLocationGuard> {
    let root = match probe_owned_root(root_path, uid, Some(top.identity.device), warnings)? {
        Ok(root) => root,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };
    let files = match inspect_directory(
        &root_path.join("files"),
        DirectoryExpectation::OwnedTrash {
            uid,
            device: Some(top.identity.device),
        },
    ) {
        Ok(directory) => directory,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };
    let info = match inspect_directory(
        &root_path.join("info"),
        DirectoryExpectation::OwnedTrash {
            uid,
            device: Some(top.identity.device),
        },
    ) {
        Ok(directory) => directory,
        Err(problem) => {
            warnings.push(problem);
            return None;
        }
    };

    Some(TrashLocationGuard {
        kind,
        top_directory: top_directory.to_path_buf(),
        top_identity: Some(top.identity.clone()),
        shared_root,
        trash_root: root,
        files,
        info,
        original_path_base: OriginalPathBase::RelativeToTopDirectory,
    })
}

fn probe_owned_root(
    path: &Path,
    uid: u32,
    device: Option<u64>,
    warnings: &mut Vec<ScanWarning>,
) -> Option<Result<VerifiedTrashDirectory, ScanWarning>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            warnings.push(io_warning(path, error));
            None
        }
        Ok(_) => Some(inspect_directory(
            path,
            DirectoryExpectation::OwnedTrash { uid, device },
        )),
    }
}

fn inspect_directory(
    path: &Path,
    expectation: DirectoryExpectation,
) -> Result<VerifiedTrashDirectory, ScanWarning> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_warning(path, error))?;
    if !metadata.file_type().is_dir() {
        return Err(validation_warning(
            path,
            "expected a non-symbolic-link directory",
        ));
    }

    #[cfg(unix)]
    {
        let mode = metadata.mode();
        match expectation {
            DirectoryExpectation::MountTop => {}
            DirectoryExpectation::SharedRoot { device } => {
                if metadata.dev() != device {
                    return Err(validation_warning(path, "directory is on another device"));
                }
                if mode & libc::S_ISVTX == 0 {
                    return Err(validation_warning(
                        path,
                        "shared Trash directory has no sticky bit",
                    ));
                }
            }
            DirectoryExpectation::OwnedTrash { uid, device } => {
                if metadata.uid() != uid {
                    return Err(validation_warning(
                        path,
                        "Trash directory has another owner",
                    ));
                }
                if mode & 0o022 != 0 {
                    return Err(validation_warning(
                        path,
                        "Trash directory is writable by group or other users",
                    ));
                }
                if device.is_some_and(|device| metadata.dev() != device) {
                    return Err(validation_warning(
                        path,
                        "Trash directory is on another device",
                    ));
                }
            }
        }
    }

    #[cfg(not(unix))]
    let _ = expectation;

    Ok(VerifiedTrashDirectory {
        path: path.to_path_buf(),
        identity: trash_object_identity(&metadata),
    })
}

pub(super) fn revalidate_trash_location(location: &TrashLocationGuard) -> Result<(), FileError> {
    let uid = effective_user_id();
    if let Some(expected_top) = &location.top_identity {
        let current = inspect_directory(&location.top_directory, DirectoryExpectation::MountTop)
            .map_err(|problem| location_error(&problem.path, problem.message))?;
        require_same_directory(&current, expected_top, "mounted top-level directory")?;
    }
    if let Some(expected_shared) = &location.shared_root {
        let current = inspect_directory(
            &expected_shared.path,
            DirectoryExpectation::SharedRoot {
                device: expected_shared.identity.device,
            },
        )
        .map_err(|problem| location_error(&problem.path, problem.message))?;
        require_same_directory(
            &current,
            &expected_shared.identity,
            "shared Trash directory",
        )?;
    }

    let expected_device = location.trash_root.identity.device;
    for (expected, label) in [
        (&location.trash_root, "Trash root"),
        (&location.files, "Trash files directory"),
        (&location.info, "Trash info directory"),
    ] {
        let current = inspect_directory(
            &expected.path,
            DirectoryExpectation::OwnedTrash {
                uid,
                device: Some(expected_device),
            },
        )
        .map_err(|problem| location_error(&problem.path, problem.message))?;
        require_same_directory(&current, &expected.identity, label)?;
    }
    Ok(())
}

fn require_same_directory(
    current: &VerifiedTrashDirectory,
    expected: &TrashObjectIdentity,
    label: &str,
) -> Result<(), FileError> {
    if current.identity.same_object(expected) {
        Ok(())
    } else {
        Err(location_error(
            &current.path,
            format!("{label} identity changed since the Trash snapshot"),
        ))
    }
}

fn location_error(path: &Path, message: impl Into<String>) -> FileError {
    FileError::Trash {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

pub(super) fn inspect_trash_object(path: &Path) -> io::Result<TrashObjectIdentity> {
    fs::symlink_metadata(path).map(|metadata| trash_object_identity(&metadata))
}

pub(super) fn trash_object_identity(metadata: &fs::Metadata) -> TrashObjectIdentity {
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        TrashObjectKind::RegularFile
    } else if file_type.is_dir() {
        TrashObjectKind::Directory
    } else if file_type.is_symlink() {
        TrashObjectKind::SymbolicLink
    } else {
        TrashObjectKind::Other
    };

    #[cfg(unix)]
    {
        TrashObjectIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            kind,
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    #[cfg(not(unix))]
    {
        TrashObjectIdentity {
            device: 0,
            inode: 0,
            kind,
            size: metadata.len(),
            modified_seconds: 0,
            modified_nanoseconds: 0,
            changed_seconds: 0,
            changed_nanoseconds: 0,
        }
    }
}

pub(super) fn trash_data_home() -> Result<PathBuf, FileError> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or(FileError::Unsupported(
            "trash requires HOME or XDG_DATA_HOME",
        ))
}

/// Home trash directories worth watching for change-driven refresh: the
/// trash root plus its `info` and `files` children. Watching the root also
/// captures creation and removal of the child directories. Empty when
/// HOME/XDG_DATA_HOME is unavailable.
pub fn trash_watch_directories() -> Vec<PathBuf> {
    trash_data_home()
        .map(|data_home| {
            let trash_root = data_home.join("Trash");
            vec![
                trash_root.clone(),
                trash_root.join("info"),
                trash_root.join("files"),
            ]
        })
        .unwrap_or_default()
}

pub(super) fn effective_user_id() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn validation_warning(path: &Path, message: impl Into<String>) -> ScanWarning {
    ScanWarning {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

fn io_warning(path: &Path, error: io::Error) -> ScanWarning {
    validation_warning(path, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_owned_location(root: &Path) {
        fs::create_dir_all(root.join("files")).unwrap();
        fs::create_dir_all(root.join("info")).unwrap();
        #[cfg(unix)]
        for path in [root.to_path_buf(), root.join("files"), root.join("info")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn cancelled_catalog_scan_stops_before_probing_locations() {
        let fixture = tempdir().unwrap();
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();

        let error = discover_trash_locations_from_mountinfo_with_cancellation(
            &fixture.path().join("data"),
            effective_user_id(),
            b"1 0 8:1 / /missing rw - ext4 /dev/test rw\n",
            &cancellation,
        )
        .unwrap_err();

        assert!(matches!(error, FileError::Cancelled));
    }

    #[test]
    fn mountinfo_read_failure_preserves_home_and_reports_the_system_boundary() {
        let fixture = tempdir().unwrap();
        let data_home = fixture.path().join("data");
        create_owned_location(&data_home.join("Trash"));

        let catalog = discover_trash_locations_from_mountinfo_read(
            &data_home,
            effective_user_id(),
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "mountinfo denied",
            )),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();

        assert_eq!(catalog.locations.len(), 1);
        assert_eq!(catalog.locations[0].kind, TrashLocationKind::Home);
        assert_eq!(catalog.warnings.len(), 1);
        assert_eq!(catalog.warnings[0].path, Path::new(MOUNTINFO_PATH));
    }

    #[test]
    fn mountinfo_read_failure_without_home_is_a_top_level_error() {
        let fixture = tempdir().unwrap();
        let error = discover_trash_locations_from_mountinfo_read(
            &fixture.path().join("missing-data"),
            effective_user_id(),
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "mountinfo denied",
            )),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap_err();

        assert!(matches!(error, FileError::ReadDirectory { .. }));
    }

    #[test]
    fn catalog_discovers_home_shared_and_private_locations_together() {
        let fixture = tempdir().unwrap();
        let data_home = fixture.path().join("data");
        create_owned_location(&data_home.join("Trash"));
        let volume = fixture.path().join("volume");
        fs::create_dir(&volume).unwrap();
        let shared = volume.join(".Trash");
        fs::create_dir(&shared).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
        create_owned_location(&shared.join(effective_user_id().to_string()));
        create_owned_location(&volume.join(format!(".Trash-{}", effective_user_id())));
        let mountinfo = format!("1 0 8:1 / {} rw - ext4 /dev/test rw\n", volume.display());

        let catalog = discover_trash_locations_from_mountinfo(
            &data_home,
            effective_user_id(),
            mountinfo.as_bytes(),
        )
        .unwrap();

        assert_eq!(catalog.locations.len(), 3);
        assert_eq!(catalog.locations[0].kind, TrashLocationKind::Home);
        assert_eq!(catalog.locations[1].kind, TrashLocationKind::SharedVolume);
        assert_eq!(catalog.locations[2].kind, TrashLocationKind::PrivateVolume);
        assert!(catalog.warnings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn catalog_rejects_shared_root_without_sticky_bit() {
        let fixture = tempdir().unwrap();
        let data_home = fixture.path().join("data");
        let volume = fixture.path().join("volume");
        fs::create_dir(&volume).unwrap();
        let shared = volume.join(".Trash");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
        create_owned_location(&shared.join(effective_user_id().to_string()));
        let mountinfo = format!("1 0 8:1 / {} rw - ext4 /dev/test rw\n", volume.display());

        let catalog = discover_trash_locations_from_mountinfo(
            &data_home,
            effective_user_id(),
            mountinfo.as_bytes(),
        )
        .unwrap();

        assert!(catalog.locations.is_empty());
        assert!(catalog
            .warnings
            .iter()
            .any(|warning| warning.message.contains("sticky bit")));
    }

    #[cfg(unix)]
    #[test]
    fn owned_trash_directory_rejects_another_owner() {
        let fixture = tempdir().unwrap();
        let actual_uid = effective_user_id();
        let expected_uid = actual_uid.checked_add(1).unwrap_or(actual_uid - 1);

        let warning = inspect_directory(
            fixture.path(),
            DirectoryExpectation::OwnedTrash {
                uid: expected_uid,
                device: None,
            },
        )
        .unwrap_err();

        assert!(warning.message.contains("another owner"));
    }

    #[cfg(unix)]
    #[test]
    fn catalog_rejects_shared_symlinks_and_insecure_user_roots() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().unwrap();
        let data_home = fixture.path().join("data");
        create_owned_location(&data_home.join("Trash"));
        let volume = fixture.path().join("volume");
        fs::create_dir(&volume).unwrap();
        let real_shared = fixture.path().join("real-shared");
        fs::create_dir(&real_shared).unwrap();
        symlink(&real_shared, volume.join(".Trash")).unwrap();
        create_owned_location(&volume.join(format!(".Trash-{}", effective_user_id())));
        fs::set_permissions(
            volume.join(format!(".Trash-{}", effective_user_id())),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        let mountinfo = format!("1 0 8:1 / {} rw - ext4 /dev/test rw\n", volume.display());

        let catalog = discover_trash_locations_from_mountinfo(
            &data_home,
            effective_user_id(),
            mountinfo.as_bytes(),
        )
        .unwrap();

        assert_eq!(catalog.locations.len(), 1);
        assert_eq!(catalog.locations[0].kind, TrashLocationKind::Home);
        assert_eq!(catalog.warnings.len(), 2);
        assert!(catalog
            .warnings
            .iter()
            .any(|warning| warning.message.contains("non-symbolic-link")));
        assert!(catalog
            .warnings
            .iter()
            .any(|warning| warning.message.contains("writable")));
    }
}
