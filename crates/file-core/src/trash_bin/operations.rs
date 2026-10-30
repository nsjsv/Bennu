use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use tokio_util::sync::CancellationToken;

use crate::transfer_conflict::{first_available_entry_path, transfer_target_metadata_if_exists};
use crate::{EntryNameKind, FileError, ScanOptions, TransferConflictStrategy};

use super::batch::VerifiedTrashLocations;
use super::catalog::inspect_trash_object;
use super::model::{TrashEntryIdentity, TrashObjectKind, TrashRestoreEntry};
use super::trash_info::read_trash_info;

pub async fn restore_trash_entry(
    entry: TrashRestoreEntry,
    conflict_strategy: TransferConflictStrategy,
) -> Result<PathBuf, FileError> {
    super::batch::TrashVerificationBatch::single_entry()
        .restore_entry(entry, conflict_strategy)
        .await
}

pub(super) enum RestoreTarget {
    Skip,
    MoveNoReplace(PathBuf),
    MergeDirectory(PathBuf),
}

pub(super) async fn prepare_restore_target(
    entry: &TrashRestoreEntry,
    conflict_strategy: TransferConflictStrategy,
) -> Result<RestoreTarget, FileError> {
    let target = &entry.original_path;
    let Some(target_metadata) =
        transfer_target_metadata_if_exists(target)
            .await
            .map_err(|source| FileError::Move {
                from: entry.trash_path.clone(),
                to: target.clone(),
                source,
            })?
    else {
        return Ok(RestoreTarget::MoveNoReplace(target.clone()));
    };

    match conflict_strategy {
        TransferConflictStrategy::Fail => Err(FileError::Move {
            from: entry.trash_path.clone(),
            to: target.clone(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "target already exists"),
        }),
        TransferConflictStrategy::Replace => {
            let removal = if target_metadata.is_dir() {
                tokio::fs::remove_dir_all(target).await
            } else {
                tokio::fs::remove_file(target).await
            };
            removal.map_err(|source| FileError::Move {
                from: entry.trash_path.clone(),
                to: target.clone(),
                source,
            })?;
            Ok(RestoreTarget::MoveNoReplace(target.clone()))
        }
        TransferConflictStrategy::Skip => Ok(RestoreTarget::Skip),
        TransferConflictStrategy::KeepBoth => {
            // 编号按被还原的条目是不是目录决定，与复制的"保留两者"同一规则。
            let restored_metadata = tokio::fs::symlink_metadata(&entry.trash_path)
                .await
                .map_err(|source| FileError::Metadata {
                    path: entry.trash_path.clone(),
                    source,
                })?;
            first_available_entry_path(target, EntryNameKind::of_metadata(&restored_metadata))
                .await
                .map(RestoreTarget::MoveNoReplace)
        }
        TransferConflictStrategy::Merge => {
            let source_metadata = tokio::fs::symlink_metadata(&entry.trash_path)
                .await
                .map_err(|source| FileError::Metadata {
                    path: entry.trash_path.clone(),
                    source,
                })?;
            if source_metadata.is_dir() && target_metadata.is_dir() {
                Ok(RestoreTarget::MergeDirectory(target.clone()))
            } else {
                Ok(RestoreTarget::Skip)
            }
        }
    }
}

pub async fn delete_trash_entry(entry: TrashRestoreEntry) -> Result<(), FileError> {
    super::batch::TrashVerificationBatch::single_entry()
        .delete_entry(entry)
        .await
}

pub async fn empty_trash() -> Result<(), FileError> {
    empty_trash_with_cancellation(CancellationToken::new()).await
}

pub async fn empty_trash_with_cancellation(
    cancellation: CancellationToken,
) -> Result<(), FileError> {
    let scan = super::scan::scan_trash_with_cancellation(
        ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        },
        cancellation.clone(),
    )
    .await?;
    empty_verified_trash_scan(scan, cancellation).await
}

async fn empty_verified_trash_scan(
    scan: super::model::TrashScan,
    cancellation: CancellationToken,
) -> Result<(), FileError> {
    let represented_info_paths = scan
        .entries
        .iter()
        .map(|entry| entry.info_path.clone())
        .collect::<HashSet<_>>();
    let mut failures = scan
        .skipped
        .into_iter()
        .filter(|warning| !represented_info_paths.contains(&warning.path))
        .map(|warning| format!("{}: {}", warning.path.display(), warning.message))
        .collect::<Vec<_>>();
    let batch = super::batch::TrashVerificationBatch::new();
    for entry in scan.entries {
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let path = entry.trash_path.clone();
        if let Err(error) = batch.delete_entry(entry.restore_entry()).await {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(FileError::Trash {
            path: PathBuf::from("Trash"),
            message: format!(
                "Trash was only partially emptied; {} item(s) or location(s) failed: {}",
                failures.len(),
                failures.join("; ")
            ),
        })
    }
}

/// 单条 API 传入空缓存:必然未命中,退化为逐条全套位置验证,行为不变。
pub(super) fn verify_entry_identity_with_locations(
    entry: &TrashRestoreEntry,
    locations: &mut VerifiedTrashLocations,
) -> Result<TrashEntryIdentity, FileError> {
    let expected = entry.identity.as_ref().ok_or_else(|| FileError::Trash {
        path: entry.trash_path.clone(),
        message: "Trash entry has no verified location and object identity".to_owned(),
    })?;
    locations.revalidate_once(&expected.location)?;

    let expected_trash_path = expected.location.files.path.join(&expected.item_name);
    let expected_info_path = expected
        .location
        .info
        .path
        .join(trash_info_name(&expected.item_name));
    if entry.trash_path != expected_trash_path || entry.info_path != expected_info_path {
        return Err(FileError::Trash {
            path: entry.trash_path.clone(),
            message: "Trash entry paths do not match the verified location identity".to_owned(),
        });
    }

    verify_trash_info_identity(entry, expected)?;
    let payload =
        inspect_trash_object(&entry.trash_path).map_err(|source| FileError::Metadata {
            path: entry.trash_path.clone(),
            source,
        })?;
    if payload != expected.payload {
        return Err(FileError::Trash {
            path: entry.trash_path.clone(),
            message: "Trash payload identity changed since the Trash snapshot".to_owned(),
        });
    }
    Ok(expected.clone())
}

pub(super) fn remove_trash_payload(
    entry: &TrashRestoreEntry,
    kind: TrashObjectKind,
) -> Result<(), FileError> {
    let outcome = if matches!(kind, TrashObjectKind::Directory) {
        fs::remove_dir_all(&entry.trash_path)
    } else {
        fs::remove_file(&entry.trash_path)
    };
    outcome.map_err(|source| FileError::Delete {
        path: entry.trash_path.clone(),
        source,
    })
}

pub(super) fn remove_trash_info(entry: &TrashRestoreEntry) -> Result<(), FileError> {
    fs::remove_file(&entry.info_path).map_err(|source| FileError::Delete {
        path: entry.info_path.clone(),
        source,
    })
}

pub(super) fn verify_trash_info_identity(
    entry: &TrashRestoreEntry,
    expected: &TrashEntryIdentity,
) -> Result<(), FileError> {
    let parsed = read_trash_info(
        &entry.info_path,
        expected.location.original_path_base,
        &expected.location.top_directory,
    )
    .map_err(|problem| FileError::Trash {
        path: problem.path,
        message: problem.message,
    })?;
    if parsed.original_path != entry.original_path || parsed.identity != expected.info {
        return Err(FileError::Trash {
            path: entry.info_path.clone(),
            message: ".trashinfo identity or original path changed since the Trash snapshot"
                .to_owned(),
        });
    }
    Ok(())
}

pub(super) fn trash_info_name(item_name: &std::ffi::OsStr) -> OsString {
    #[cfg(unix)]
    {
        let mut bytes = item_name.as_bytes().to_vec();
        bytes.extend_from_slice(b".trashinfo");
        OsString::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        let mut name = item_name.to_os_string();
        name.push(".trashinfo");
        name
    }
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
