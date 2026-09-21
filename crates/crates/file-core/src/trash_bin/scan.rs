use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use tokio_util::sync::CancellationToken;

use crate::scan::{entry_from_metadata, is_hidden_name};
use crate::{compare_entries, FileError, ScanOptions, ScanWarning};

use super::catalog::{discover_trash_locations, trash_object_identity, TrashLocationCatalog};
use super::model::{TrashEntry, TrashEntryIdentity, TrashScan};
use super::trash_info::read_trash_info;

pub async fn scan_trash(options: ScanOptions) -> Result<TrashScan, FileError> {
    scan_trash_with_cancellation(options, CancellationToken::new()).await
}

pub async fn scan_trash_with_cancellation(
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Result<TrashScan, FileError> {
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let worker_cancellation = cancellation.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let catalog = discover_trash_locations(&worker_cancellation)?;
        scan_trash_with_catalog(options, worker_cancellation, catalog)
    })
    .await
    .map_err(|join_error| FileError::Trash {
        path: PathBuf::from("/proc/self/mountinfo"),
        message: format!("Trash scan worker failed: {join_error}"),
    })?;
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    outcome
}

pub(super) fn scan_trash_with_catalog(
    options: ScanOptions,
    cancellation: CancellationToken,
    catalog: TrashLocationCatalog,
) -> Result<TrashScan, FileError> {
    check_cancellation(&cancellation)?;
    let mut entries = Vec::new();
    let mut skipped = catalog.warnings;

    for location in catalog.locations {
        check_cancellation(&cancellation)?;
        let info_entries = match fs::read_dir(&location.info.path) {
            Ok(entries) => entries,
            Err(error) => {
                skipped.push(warning(&location.info.path, error));
                continue;
            }
        };
        for info_entry in info_entries {
            check_cancellation(&cancellation)?;
            let info_entry = match info_entry {
                Ok(info_entry) => info_entry,
                Err(error) => {
                    skipped.push(warning(&location.info.path, error));
                    continue;
                }
            };
            let info_name = info_entry.file_name();
            let Some(item_name) = trash_item_name(&info_name) else {
                continue;
            };
            let info_path = info_entry.path();
            let parsed = match read_trash_info(
                &info_path,
                location.original_path_base,
                &location.top_directory,
            ) {
                Ok(parsed) => parsed,
                Err(problem) => {
                    skipped.push(problem);
                    continue;
                }
            };
            skipped.extend(parsed.warnings);

            let display_name = parsed
                .original_path
                .file_name()
                .map(OsStr::to_os_string)
                .unwrap_or_else(|| item_name.clone());
            let is_hidden = is_hidden_name(&display_name);
            if is_hidden && !options.include_hidden {
                continue;
            }
            let trash_path = location.files.path.join(&item_name);
            let payload_metadata = match fs::symlink_metadata(&trash_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    skipped.push(warning(&trash_path, error));
                    continue;
                }
            };
            let payload_identity = trash_object_identity(&payload_metadata);
            let is_broken_symlink = payload_metadata.file_type().is_symlink()
                && matches!(fs::metadata(&trash_path), Err(error) if error.kind() == io::ErrorKind::NotFound);
            let entry = entry_from_metadata(
                trash_path.clone(),
                display_name,
                is_hidden,
                &payload_metadata,
                is_broken_symlink,
            );
            entries.push(TrashEntry {
                trash_path,
                info_path,
                original_path: parsed.original_path,
                deletion_date: parsed.deletion_date,
                entry,
                identity: Some(TrashEntryIdentity {
                    location: location.clone(),
                    item_name,
                    info: parsed.identity,
                    payload: payload_identity,
                }),
            });
        }
    }

    entries.sort_unstable_by(|left, right| {
        compare_entries(&left.entry, &right.entry, &options)
            .then_with(|| left.trash_path.cmp(&right.trash_path))
    });
    Ok(TrashScan { entries, skipped })
}

fn check_cancellation(cancellation: &CancellationToken) -> Result<(), FileError> {
    if cancellation.is_cancelled() {
        Err(FileError::Cancelled)
    } else {
        Ok(())
    }
}

fn trash_item_name(info_name: &OsStr) -> Option<OsString> {
    #[cfg(unix)]
    {
        let bytes = info_name.as_bytes();
        let stem = bytes.strip_suffix(b".trashinfo")?;
        (!stem.is_empty()).then(|| OsString::from_vec(stem.to_vec()))
    }
    #[cfg(not(unix))]
    {
        let name = info_name.to_string_lossy();
        name.strip_suffix(".trashinfo")
            .filter(|stem| !stem.is_empty())
            .map(OsString::from)
    }
}

fn warning(path: &Path, error: io::Error) -> ScanWarning {
    ScanWarning {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::super::catalog::{discover_trash_locations_from_mountinfo, effective_user_id};
    use super::*;

    fn create_location(root: &Path) {
        fs::create_dir_all(root.join("files")).unwrap();
        fs::create_dir_all(root.join("info")).unwrap();
        for path in [root.to_path_buf(), root.join("files"), root.join("info")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn scan_combines_home_and_volume_entries_and_keeps_entry_failures_local() {
        let fixture = tempdir().unwrap();
        let data_home = fixture.path().join("data");
        let home_root = data_home.join("Trash");
        create_location(&home_root);
        let home_original = fixture.path().join("home-original.txt");
        fs::write(home_root.join("files/home-entry"), b"home").unwrap();
        fs::write(
            home_root.join("info/home-entry.trashinfo"),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-08-02T12:00:00\n",
                home_original.display()
            ),
        )
        .unwrap();
        fs::write(
            home_root.join("info/bad.trashinfo"),
            b"[Trash Info]\nPath=%ZZ\n",
        )
        .unwrap();
        let visible_original = fixture.path().join("visible-original.txt");
        fs::write(home_root.join("files/.opaque-storage-name"), b"visible").unwrap();
        fs::write(
            home_root.join("info/.opaque-storage-name.trashinfo"),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-08-02T12:30:00\n",
                visible_original.display()
            ),
        )
        .unwrap();

        let volume = fixture.path().join("volume");
        fs::create_dir(&volume).unwrap();
        let private_root = volume.join(format!(".Trash-{}", effective_user_id()));
        create_location(&private_root);
        fs::write(private_root.join("files/volume-entry"), b"volume").unwrap();
        fs::write(
            private_root.join("info/volume-entry.trashinfo"),
            b"[Trash Info]\nPath=folder/item.txt\nDeletionDate=2026-08-02T13:00:00\n",
        )
        .unwrap();
        let mountinfo = format!("1 0 8:1 / {} rw - ext4 /dev/test rw\n", volume.display());
        let catalog = discover_trash_locations_from_mountinfo(
            &data_home,
            effective_user_id(),
            mountinfo.as_bytes(),
        )
        .unwrap();

        let scan =
            scan_trash_with_catalog(ScanOptions::default(), CancellationToken::new(), catalog)
                .unwrap();

        assert_eq!(scan.entries.len(), 3);
        assert!(scan.entries.iter().any(|entry| {
            entry.original_path == home_original
                && entry.entry.name.to_string_lossy() == "home-original.txt"
        }));
        assert!(scan.entries.iter().any(|entry| {
            entry.original_path == volume.join("folder/item.txt")
                && entry.entry.name.to_string_lossy() == "item.txt"
        }));
        assert!(scan.entries.iter().any(|entry| {
            entry.original_path == visible_original
                && entry.entry.name.to_string_lossy() == "visible-original.txt"
                && !entry.entry.is_hidden
        }));
        assert_eq!(scan.skipped.len(), 1);
    }

    #[test]
    fn canceled_scan_stops_before_reading_locations() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = scan_trash_with_catalog(
            ScanOptions::default(),
            cancellation,
            TrashLocationCatalog {
                locations: Vec::new(),
                warnings: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, FileError::Cancelled));
    }

    #[test]
    fn helper_identity_read_matches_payload_snapshot() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("payload");
        fs::write(&path, b"payload").unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert_eq!(
            super::super::catalog::inspect_trash_object(&path).unwrap(),
            trash_object_identity(&metadata)
        );
    }
}
