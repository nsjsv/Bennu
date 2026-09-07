use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use tokio_util::sync::CancellationToken;

use crate::ops::rename_noreplace;
use crate::transfer_conflict::{
    available_transfer_target_path_candidate, transfer_target_metadata_if_exists,
};
use crate::{FileError, ScanOptions, TransferConflictStrategy};

use super::catalog::{
    discover_trash_locations_from_mountinfo,
    discover_trash_locations_from_mountinfo_with_cancellation, effective_user_id,
    inspect_trash_object, revalidate_trash_location, trash_data_home, trash_object_identity,
};
use super::model::{
    TrashCommitOutcome, TrashEntry, TrashEntryIdentity, TrashLocationGuard, TrashLocationKind,
    TrashObjectIdentity, TrashRestoreEntry, TrashTrackingWarning,
};
use super::mountinfo::{parse_mountinfo, MOUNTINFO_PATH};
use super::trash_info::{normalize_new_volume_trash_info, read_trash_info};
use crate::scan::entry_from_metadata;

pub async fn trash_path(path: impl AsRef<Path>) -> Result<(), FileError> {
    let path = path.as_ref().to_path_buf();
    let path_for_worker = path.clone();
    tokio::task::spawn_blocking(move || trash::delete(&path_for_worker))
        .await
        .map_err(|join_error| FileError::Trash {
            path: path.clone(),
            message: format!("Trash worker failed: {join_error}"),
        })?
        .map_err(|error| FileError::Trash {
            path,
            message: error.to_string(),
        })
}

pub async fn trash_path_with_restore_entry(
    path: impl AsRef<Path>,
) -> Result<TrashCommitOutcome, FileError> {
    trash_path_with_restore_entry_and_cancellation(path, CancellationToken::new()).await
}

pub async fn trash_path_with_restore_entry_and_cancellation(
    path: impl AsRef<Path>,
    cancellation: CancellationToken,
) -> Result<TrashCommitOutcome, FileError> {
    let path = path.as_ref().to_path_buf();
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let worker_cancellation = cancellation.clone();
    tokio::task::spawn_blocking(move || {
        trash_path_with_tracking_blocking(path, worker_cancellation)
    })
    .await
    .map_err(|join_error| FileError::Trash {
        path: PathBuf::from("/proc/self/mountinfo"),
        message: format!("Trash operation worker failed: {join_error}"),
    })?
}

pub async fn restore_trash_entry(
    entry: TrashRestoreEntry,
    conflict_strategy: TransferConflictStrategy,
) -> Result<PathBuf, FileError> {
    let identity = verify_restore_entry(&entry).await?;
    let restore_target = prepare_restore_target(&entry, conflict_strategy).await?;
    let target = match &restore_target {
        RestoreTarget::Skip => return Ok(entry.original_path),
        RestoreTarget::MoveNoReplace(target) | RestoreTarget::MergeDirectory(target) => {
            target.clone()
        }
    };
    if let Some(parent) = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| FileError::Move {
                from: entry.trash_path.clone(),
                to: target.clone(),
                source,
            })?;
    }
    match restore_target {
        RestoreTarget::MoveNoReplace(_) => {
            rename_noreplace(&entry.trash_path, &target).map_err(|error| FileError::Move {
                from: entry.trash_path.clone(),
                to: target.clone(),
                source: error.into_io_error(),
            })?;
        }
        RestoreTarget::MergeDirectory(_) => {
            tokio::fs::rename(&entry.trash_path, &target)
                .await
                .map_err(|source| FileError::Move {
                    from: entry.trash_path.clone(),
                    to: target.clone(),
                    source,
                })?;
        }
        RestoreTarget::Skip => unreachable!("skip returns before restore commit"),
    }
    verify_info_after_payload_action(&entry, &identity).await?;
    tokio::fs::remove_file(&entry.info_path)
        .await
        .map_err(|source| FileError::Delete {
            path: entry.info_path.clone(),
            source,
        })?;
    Ok(target)
}

enum RestoreTarget {
    Skip,
    MoveNoReplace(PathBuf),
    MergeDirectory(PathBuf),
}

async fn prepare_restore_target(
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
        TransferConflictStrategy::KeepBoth => available_transfer_target_path_candidate(target)
            .await
            .map(RestoreTarget::MoveNoReplace)
            .map_err(|source| FileError::Move {
                from: entry.trash_path.clone(),
                to: target.clone(),
                source,
            }),
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
    let identity = verify_restore_entry(&entry).await?;
    if matches!(
        identity.payload.kind,
        super::model::TrashObjectKind::Directory
    ) {
        tokio::fs::remove_dir_all(&entry.trash_path)
            .await
            .map_err(|source| FileError::Delete {
                path: entry.trash_path.clone(),
                source,
            })?;
    } else {
        tokio::fs::remove_file(&entry.trash_path)
            .await
            .map_err(|source| FileError::Delete {
                path: entry.trash_path.clone(),
                source,
            })?;
    }
    verify_info_after_payload_action(&entry, &identity).await?;
    tokio::fs::remove_file(&entry.info_path)
        .await
        .map_err(|source| FileError::Delete {
            path: entry.info_path,
            source,
        })?;
    Ok(())
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
    for entry in scan.entries {
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let path = entry.trash_path.clone();
        if let Err(error) = delete_trash_entry(entry.restore_entry()).await {
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

async fn verify_restore_entry(entry: &TrashRestoreEntry) -> Result<TrashEntryIdentity, FileError> {
    let entry = entry.clone();
    tokio::task::spawn_blocking(move || verify_restore_entry_blocking(&entry))
        .await
        .map_err(|join_error| FileError::Trash {
            path: PathBuf::from("Trash"),
            message: format!("Trash identity verification worker failed: {join_error}"),
        })?
}

fn verify_restore_entry_blocking(
    entry: &TrashRestoreEntry,
) -> Result<TrashEntryIdentity, FileError> {
    let expected = entry.identity.as_ref().ok_or_else(|| FileError::Trash {
        path: entry.trash_path.clone(),
        message: "Trash entry has no verified location and object identity".to_owned(),
    })?;
    revalidate_trash_location(&expected.location)?;

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

async fn verify_info_after_payload_action(
    entry: &TrashRestoreEntry,
    expected: &TrashEntryIdentity,
) -> Result<(), FileError> {
    let entry = entry.clone();
    let expected = expected.clone();
    tokio::task::spawn_blocking(move || {
        revalidate_trash_location(&expected.location)?;
        verify_trash_info_identity(&entry, &expected)
    })
    .await
    .map_err(|join_error| FileError::Trash {
        path: PathBuf::from("Trash"),
        message: format!("Trash info verification worker failed: {join_error}"),
    })?
}

fn verify_trash_info_identity(
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

fn trash_path_with_tracking_blocking(
    path: PathBuf,
    cancellation: CancellationToken,
) -> Result<TrashCommitOutcome, FileError> {
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let path = canonical_trash_source_path(&path)?;
    let tracking = TrashTrackingPlan::prepare(&path, cancellation.clone())?;
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    trash::delete(&path).map_err(|error| FileError::Trash {
        path: path.clone(),
        message: error.to_string(),
    })?;

    let find_result = tracking.find_committed_entry();
    match find_result {
        Ok(entry) => Ok(TrashCommitOutcome::Tracked(Box::new(entry))),
        Err(message) => Ok(TrashCommitOutcome::CommittedWithoutRestoreEntry(
            TrashTrackingWarning { path, message },
        )),
    }
}

fn canonical_trash_source_path(path: &Path) -> Result<PathBuf, FileError> {
    if !path.is_absolute() {
        return Err(FileError::Trash {
            path: path.to_path_buf(),
            message: "Trash tracking requires an absolute source path".to_owned(),
        });
    }
    let parent = path.parent().ok_or_else(|| FileError::Trash {
        path: path.to_path_buf(),
        message: "the filesystem root cannot be moved to Trash".to_owned(),
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|source| FileError::Metadata {
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(path
        .file_name()
        .map_or(canonical_parent.clone(), |name| canonical_parent.join(name)))
}

#[derive(Debug)]
struct TrashTrackingPlan {
    original_path: PathBuf,
    scope: TrashTrackingScope,
    data_home: PathBuf,
    uid: u32,
    mountinfo: Vec<u8>,
    before_info_objects: Vec<TrashObjectIdentity>,
}

#[derive(Debug, Clone)]
enum TrashTrackingScope {
    Home,
    Volume {
        top_directory: PathBuf,
        top_identity: super::model::TrashObjectIdentity,
    },
}

impl TrashTrackingPlan {
    fn prepare(path: &Path, cancellation: CancellationToken) -> Result<Self, FileError> {
        let source_identity = inspect_trash_object(path).map_err(|source| FileError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
        let data_home = trash_data_home()?;
        let uid = effective_user_id();
        let mountinfo = fs::read(MOUNTINFO_PATH).map_err(|source| FileError::ReadDirectory {
            path: PathBuf::from(MOUNTINFO_PATH),
            source,
        })?;
        let snapshot = parse_mountinfo(&mountinfo);
        // Mount resolution here backs Trash tracking scope detection, not the
        // discovery-time probe filter, so no mount is excluded.
        let mount_points: Vec<PathBuf> = snapshot
            .mounts
            .iter()
            .map(|mount| mount.mount_point.clone())
            .collect();
        let source_mount =
            deepest_mount_point(&mount_points, path).ok_or_else(|| FileError::Trash {
                path: path.to_path_buf(),
                message: "could not identify the mounted top-level directory for Trash tracking"
                    .to_owned(),
            })?;
        let home_trash =
            canonicalize_path_or_existing_parent(&data_home.join("Trash")).map_err(|source| {
                FileError::Metadata {
                    path: data_home.join("Trash"),
                    source,
                }
            })?;
        let home_mount =
            deepest_mount_point(&mount_points, &home_trash).ok_or_else(|| FileError::Trash {
                path: data_home.clone(),
                message: "could not identify the Home Trash mount point".to_owned(),
            })?;
        let scope = if source_mount == home_mount {
            TrashTrackingScope::Home
        } else {
            let top_identity =
                inspect_trash_object(&source_mount).map_err(|source| FileError::Metadata {
                    path: source_mount.clone(),
                    source,
                })?;
            if top_identity.device != source_identity.device {
                return Err(FileError::Trash {
                    path: path.to_path_buf(),
                    message: "source object does not belong to its mounted top-level directory"
                        .to_owned(),
                });
            }
            TrashTrackingScope::Volume {
                top_directory: source_mount,
                top_identity,
            }
        };
        let catalog = discover_trash_locations_from_mountinfo_with_cancellation(
            &data_home,
            uid,
            &mountinfo,
            &cancellation,
        )?;
        let before_info_objects =
            snapshot_scope_info_objects(&scope, &catalog.locations, &cancellation)?;
        Ok(Self {
            original_path: path.to_path_buf(),
            scope,
            data_home,
            uid,
            mountinfo,
            before_info_objects,
        })
    }

    fn normalize_new_volume_info_files(
        &self,
        locations: &[TrashLocationGuard],
    ) -> Result<(), String> {
        if matches!(self.scope, TrashTrackingScope::Home) {
            return Ok(());
        }
        for location in locations
            .iter()
            .filter(|location| self.scope.matches_location(location))
        {
            let info_entries = fs::read_dir(&location.info.path).map_err(|error| {
                format!(
                    "the item was moved to Trash, but its info directory could not be read: {}: {error}",
                    location.info.path.display()
                )
            })?;
            for info_entry in info_entries {
                let info_entry = info_entry.map_err(|error| {
                    format!(
                        "the item was moved to Trash, but an info entry could not be read: {}: {error}",
                        location.info.path.display()
                    )
                })?;
                if !is_trash_info_name(&info_entry.file_name()) {
                    continue;
                }
                let info_path = info_entry.path();
                let identity = inspect_trash_object(&info_path).map_err(|error| {
                    format!(
                        "the item was moved to Trash, but an info entry could not be inspected: {}: {error}",
                        info_path.display()
                    )
                })?;
                if self
                    .before_info_objects
                    .iter()
                    .any(|before| identity.same_object(before))
                {
                    continue;
                }
                normalize_new_volume_trash_info(
                    &info_path,
                    &identity,
                    &location.top_directory,
                    &self.original_path,
                )
                .map_err(|warning| {
                    format!(
                        "the item was moved to Trash, but its new info entry could not be normalized: {}: {}",
                        warning.path.display(),
                        warning.message
                    )
                })?;
            }
        }
        Ok(())
    }

    fn find_committed_entry(self) -> Result<TrashRestoreEntry, String> {
        let catalog =
            discover_trash_locations_from_mountinfo(&self.data_home, self.uid, &self.mountinfo)
                .map_err(|error| {
                    format!("the item was moved to Trash, but refresh failed: {error}")
                })?;
        self.normalize_new_volume_info_files(&catalog.locations)?;
        let original_name = self
            .original_path
            .file_name()
            .ok_or_else(|| {
                "the item was moved to Trash, but its file name could not be determined".to_owned()
            })?
            .to_os_string();

        let mut candidates = Vec::new();
        for location in catalog
            .locations
            .iter()
            .filter(|location| self.scope.matches_location(location))
        {
            let info_entries = fs::read_dir(&location.info.path).map_err(|error| {
                format!(
                    "the item was moved to Trash, but its info directory could not be read: {}: {error}",
                    location.info.path.display()
                )
            })?;
            for info_entry in info_entries {
                let info_entry = info_entry.map_err(|error| {
                    format!(
                        "the item was moved to Trash, but an info entry could not be read: {}: {error}",
                        location.info.path.display()
                    )
                })?;
                let info_name = info_entry.file_name();
                if !is_trash_info_name(&info_name) {
                    continue;
                }
                let Some(item_name) = candidate_trash_item_name(&info_name, &original_name) else {
                    continue;
                };
                let info_path = info_entry.path();
                let parsed = match read_trash_info(
                    &info_path,
                    location.original_path_base,
                    &location.top_directory,
                ) {
                    Ok(parsed) => parsed,
                    Err(_) => continue,
                };
                if parsed.original_path != self.original_path {
                    continue;
                }
                if self
                    .before_info_objects
                    .iter()
                    .any(|before| before.same_object(&parsed.identity))
                {
                    continue;
                }
                let trash_path = location.files.path.join(&item_name);
                let payload_metadata = match fs::symlink_metadata(&trash_path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                let payload_identity = trash_object_identity(&payload_metadata);
                let is_broken_symlink = payload_metadata.file_type().is_symlink()
                    && matches!(fs::metadata(&trash_path), Err(error) if error.kind() == io::ErrorKind::NotFound);
                let entry = entry_from_metadata(
                    trash_path.clone(),
                    original_name.clone(),
                    false,
                    &payload_metadata,
                    is_broken_symlink,
                );
                candidates.push(TrashEntry {
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
        if candidates.len() == 1 {
            return Ok(candidates.remove(0).restore_entry());
        }
        Err(format!(
            "the item was moved to Trash, but no precise undo entry could be recorded: post-commit lookup found {} matching entries",
            candidates.len()
        ))
    }
}

impl TrashTrackingScope {
    fn matches_location(&self, location: &TrashLocationGuard) -> bool {
        match self {
            Self::Home => location.kind == TrashLocationKind::Home,
            Self::Volume {
                top_directory,
                top_identity,
            } => {
                location.kind != TrashLocationKind::Home
                    && location.top_directory == *top_directory
                    && location
                        .top_identity
                        .as_ref()
                        .is_some_and(|identity| identity.same_object(top_identity))
            }
        }
    }
}

fn snapshot_scope_info_objects(
    scope: &TrashTrackingScope,
    locations: &[TrashLocationGuard],
    cancellation: &CancellationToken,
) -> Result<Vec<TrashObjectIdentity>, FileError> {
    let mut identities = Vec::new();
    for location in locations
        .iter()
        .filter(|location| scope.matches_location(location))
    {
        let info_entries =
            fs::read_dir(&location.info.path).map_err(|source| FileError::ReadDirectory {
                path: location.info.path.clone(),
                source,
            })?;
        for info_entry in info_entries {
            if cancellation.is_cancelled() {
                return Err(FileError::Cancelled);
            }
            let info_entry = info_entry.map_err(|source| FileError::ReadDirectory {
                path: location.info.path.clone(),
                source,
            })?;
            if !is_trash_info_name(&info_entry.file_name()) {
                continue;
            }
            identities.push(inspect_trash_object(&info_entry.path()).map_err(|source| {
                FileError::Metadata {
                    path: info_entry.path(),
                    source,
                }
            })?);
        }
    }
    Ok(identities)
}

fn is_trash_info_name(name: &std::ffi::OsStr) -> bool {
    #[cfg(unix)]
    {
        let bytes = name.as_bytes();
        bytes.len() > b".trashinfo".len() && bytes.ends_with(b".trashinfo")
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy()
            .strip_suffix(".trashinfo")
            .is_some_and(|stem| !stem.is_empty())
    }
}

fn canonicalize_path_or_existing_parent(path: &Path) -> io::Result<PathBuf> {
    let mut existing_parent = path;
    let mut missing_components = Vec::new();
    loop {
        match fs::canonicalize(existing_parent) {
            Ok(canonical) => {
                return Ok(missing_components
                    .iter()
                    .rev()
                    .fold(canonical, |resolved, component: &OsString| {
                        resolved.join(component)
                    }));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(component) = existing_parent.file_name() else {
                    return Err(error);
                };
                missing_components.push(component.to_os_string());
                let Some(parent) = existing_parent.parent() else {
                    return Err(error);
                };
                existing_parent = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

fn deepest_mount_point(mount_points: &[PathBuf], path: &Path) -> Option<PathBuf> {
    mount_points
        .iter()
        .filter(|mount_point| path.starts_with(mount_point))
        .max_by_key(|mount_point| mount_point.components().count())
        .cloned()
}

fn candidate_trash_item_name(
    info_name: &std::ffi::OsStr,
    original_name: &std::ffi::OsStr,
) -> Option<OsString> {
    // info 文件名 = {trash 条目名}.trashinfo。trash crate 的同名冲突规则是向条目名
    // 追加 ".{n}" 后缀，因此候选 = 原文件名本身，或 原文件名 + ".数字"。
    #[cfg(unix)]
    {
        let stem = info_name.as_bytes().strip_suffix(b".trashinfo")?.to_vec();
        if stem.as_slice() == original_name.as_bytes() {
            return Some(OsString::from_vec(stem));
        }
        let mut parts = stem.rsplitn(2, |byte| *byte == b'.');
        let tail = parts.next()?;
        let head = parts.next()?;
        if !tail.is_empty()
            && tail.iter().all(|byte| byte.is_ascii_digit())
            && head == original_name.as_bytes()
        {
            return Some(OsString::from_vec(stem));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let stem = info_name.to_string_lossy().strip_suffix(".trashinfo")?;
        if stem == original_name {
            return Some(OsString::from(stem));
        }
        let (head, tail) = stem.rsplit_once('.')?;
        if !tail.is_empty()
            && tail.bytes().all(|byte| byte.is_ascii_digit())
            && head == original_name
        {
            return Some(OsString::from(stem));
        }
        None
    }
}

fn trash_info_name(item_name: &std::ffi::OsStr) -> OsString {
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
