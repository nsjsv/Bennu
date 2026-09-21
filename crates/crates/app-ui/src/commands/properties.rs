use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use file_core::FileKind;
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::directory_summary::{
    metadata_disk_size, read_directory_contents_summary, read_directory_tree_summary,
    DirectoryContentsSummary, DirectorySummaryError,
};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesIdentity, FilePropertiesMessage,
    FilePropertiesPermissionBaseline, FilePropertiesPermissionWriteOutcome,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesRequest,
    FilePropertiesSnapshot, Message, PermissionBatchOutcome, PermissionBatchPathFailure,
};

const FILE_PROPERTIES_CHANNEL_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesPermissionTargets {
    Single(PathBuf),
    TargetSet(Vec<FilePropertiesPermissionBaseline>),
}

pub(crate) fn file_properties_command(
    request: FilePropertiesRequest,
    cancellation: CancellationToken,
) -> Task<Message> {
    let single_path = request.targets.single_path().map(Path::to_path_buf);
    if let Some(path) = single_path {
        single_file_properties_command(request, path, cancellation)
    } else {
        aggregate_file_properties_command(request, cancellation)
    }
}

fn single_file_properties_command(
    request: FilePropertiesRequest,
    path: PathBuf,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        FILE_PROPERTIES_CHANNEL_SIZE,
        async move |mut output| {
            let properties_outcome = load_file_properties(path).await;
            let directory_contents_required = properties_outcome.as_ref().is_ok_and(|snapshot| {
                matches!(
                    snapshot.directory_contents,
                    FilePropertiesDirectoryContentsState::Loading(_)
                )
            });

            if output
                .send(Message::FileProperties(FilePropertiesMessage::Loaded(
                    request.clone(),
                    properties_outcome.map(FilePropertiesPresentation::Single),
                )))
                .await
                .is_err()
            {
                return;
            }

            if !directory_contents_required || cancellation.is_cancelled() {
                return;
            }

            let contents_outcome =
                load_directory_properties_contents(request.clone(), cancellation, &mut output)
                    .await;
            let _ = output
                .send(Message::FileProperties(
                    FilePropertiesMessage::DirectoryContentsLoaded(request, contents_outcome),
                ))
                .await;
        },
    ))
}

fn aggregate_file_properties_command(
    request: FilePropertiesRequest,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        FILE_PROPERTIES_CHANNEL_SIZE,
        async move |mut output| {
            let targets = request.targets.clone();
            let progress_request = request.clone();
            let mut progress_output = output.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                read_aggregate_file_properties(targets.paths(), cancellation, |snapshot| {
                    let _ = progress_output.try_send(Message::FileProperties(
                        FilePropertiesMessage::AggregateUpdated(progress_request.clone(), snapshot),
                    ));
                })
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|outcome| outcome)
            .map(FilePropertiesPresentation::Aggregate);
            let _ = output
                .send(Message::FileProperties(FilePropertiesMessage::Loaded(
                    request, outcome,
                )))
                .await;
        },
    ))
}

pub(crate) fn set_file_properties_permissions_command(
    request: FilePropertiesRequest,
    targets: FilePropertiesPermissionTargets,
    permissions: FilePropertiesPermissions,
) -> Task<Message> {
    Task::perform(
        set_file_properties_permissions(targets, permissions),
        move |permissions_outcome| {
            Message::FileProperties(FilePropertiesMessage::PermissionsUpdated(
                request.clone(),
                permissions_outcome,
            ))
        },
    )
}

pub(crate) fn apply_file_properties_permissions_to_enclosed_items_command(
    request: FilePropertiesRequest,
    permissions: FilePropertiesPermissions,
) -> Task<Message> {
    let path = request
        .targets
        .single_path()
        .expect("enclosed permissions require one target")
        .to_path_buf();
    Task::perform(
        apply_file_properties_permissions_to_enclosed_items(path, permissions),
        move |permissions_outcome| {
            Message::FileProperties(FilePropertiesMessage::EnclosedPermissionsUpdated(
                request.clone(),
                permissions_outcome,
            ))
        },
    )
}

async fn load_file_properties(path: PathBuf) -> Result<FilePropertiesSnapshot, String> {
    tokio::task::spawn_blocking(move || read_file_properties(path))
        .await
        .map_err(|error| error.to_string())?
}

async fn set_file_properties_permissions(
    targets: FilePropertiesPermissionTargets,
    permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissionWriteOutcome, String> {
    tokio::task::spawn_blocking(move || match targets {
        FilePropertiesPermissionTargets::Single(path) => {
            write_file_properties_permissions(path, permissions)
                .map(FilePropertiesPermissionWriteOutcome::Single)
        }
        FilePropertiesPermissionTargets::TargetSet(baselines) => {
            Ok(FilePropertiesPermissionWriteOutcome::Batch(
                write_file_properties_target_set_permissions(baselines, permissions),
            ))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn apply_file_properties_permissions_to_enclosed_items(
    path: PathBuf,
    permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissions, String> {
    tokio::task::spawn_blocking(move || {
        write_file_properties_permissions_to_enclosed_items(path, permissions)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn read_file_properties(path: PathBuf) -> Result<FilePropertiesSnapshot, String> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else if file_type.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    let type_label = if file_type.is_symlink() {
        "Symbolic Link".to_owned()
    } else if file_type.is_dir() {
        "Folder".to_owned()
    } else if file_type.is_file() {
        "File".to_owned()
    } else {
        "Other".to_owned()
    };
    let name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| path.as_os_str().to_os_string());
    let location = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));

    let directory_contents = if file_type.is_dir() {
        FilePropertiesDirectoryContentsState::Loading(None)
    } else {
        FilePropertiesDirectoryContentsState::NotDirectory
    };
    let size_bytes = metadata.len();
    let disk_size_bytes = metadata_disk_size(&metadata);

    Ok(FilePropertiesSnapshot {
        name,
        kind,
        type_label,
        location,
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
        accessed: metadata.accessed().ok(),
        size_bytes,
        disk_size_bytes,
        directory_contents,
        permissions: metadata_properties_permissions(&metadata, file_type.is_symlink()),
    })
}

fn read_aggregate_file_properties(
    paths: &[PathBuf],
    cancellation: CancellationToken,
    mut progress: impl FnMut(FilePropertiesAggregateSnapshot),
) -> Result<FilePropertiesAggregateSnapshot, String> {
    let mut inspected = Vec::with_capacity(paths.len());
    for path in paths {
        if cancellation.is_cancelled() {
            return Err("operation cancelled".to_owned());
        }
        inspected.push(inspect_aggregate_target(path.clone())?);
    }
    let mut snapshot = aggregate_snapshot_for_targets(&inspected)?;

    for target in inspected
        .iter()
        .filter(|target| target.kind == FileKind::Directory)
    {
        if cancellation.is_cancelled() {
            return Err("operation cancelled".to_owned());
        }
        let completed_size = snapshot.total_size_bytes;
        let completed_disk_size = snapshot.total_disk_size_bytes;
        let completed_contents = snapshot.recursive_contents.clone();
        let summary =
            read_directory_tree_summary(&target.path, cancellation.clone(), |directory_summary| {
                let mut current = snapshot.clone();
                current.total_size_bytes =
                    completed_size.saturating_add(directory_summary.total_size_bytes);
                current.total_disk_size_bytes =
                    completed_disk_size.saturating_add(directory_summary.total_disk_size_bytes);
                current.recursive_contents = FilePropertiesDirectoryContents {
                    file_count: completed_contents
                        .file_count
                        .saturating_add(directory_summary.file_count),
                    directory_count: completed_contents
                        .directory_count
                        .saturating_add(directory_summary.directory_count),
                    total_size_bytes: completed_contents
                        .total_size_bytes
                        .saturating_add(directory_summary.total_size_bytes),
                    total_disk_size_bytes: completed_contents
                        .total_disk_size_bytes
                        .saturating_add(directory_summary.total_disk_size_bytes),
                };
                progress(current);
            })
            .map_err(|error| match error {
                DirectorySummaryError::Cancelled => "operation cancelled".to_owned(),
                DirectorySummaryError::Io(error) => {
                    format!(
                        "could not read directory contents for {:?}: {error}",
                        target.path
                    )
                }
                DirectorySummaryError::Overflow(field) => {
                    format!("directory summary {field} overflowed for {:?}", target.path)
                }
            })?;
        add_directory_summary(&mut snapshot, summary)?;
        progress(snapshot.clone());
    }

    Ok(snapshot)
}

#[derive(Debug)]
struct InspectedAggregateTarget {
    path: PathBuf,
    metadata: std::fs::Metadata,
    kind: FileKind,
}

fn inspect_aggregate_target(path: PathBuf) -> Result<InspectedAggregateTarget, String> {
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect {:?}: {error}", path))?;
    let kind = file_kind_for_metadata(&metadata);
    Ok(InspectedAggregateTarget {
        path,
        metadata,
        kind,
    })
}

fn aggregate_snapshot_for_targets(
    targets: &[InspectedAggregateTarget],
) -> Result<FilePropertiesAggregateSnapshot, String> {
    let first = targets
        .first()
        .ok_or_else(|| "file properties require at least one target".to_owned())?;
    let mut file_count = 0usize;
    let mut directory_count = 0usize;
    let mut symlink_count = 0usize;
    let mut other_count = 0usize;
    let mut total_size_bytes = 0u64;
    let mut total_disk_size_bytes = 0u64;
    let first_parent = first.path.parent().map(Path::to_path_buf);
    let mut common_parent = first_parent.clone();
    let mut common_kind = Some(first.kind);
    let mut common_created = first.metadata.created().ok();
    let mut common_modified = first.metadata.modified().ok();
    let mut common_accessed = first.metadata.accessed().ok();
    let mut common_permissions =
        metadata_properties_permissions(&first.metadata, first.metadata.file_type().is_symlink());
    let mut permission_baselines = Vec::with_capacity(targets.len());

    for target in targets {
        match target.kind {
            FileKind::File => file_count += 1,
            FileKind::Directory => directory_count += 1,
            FileKind::Symlink => symlink_count += 1,
            FileKind::Other => other_count += 1,
        }
        if target.kind != FileKind::Directory {
            total_size_bytes = total_size_bytes
                .checked_add(target.metadata.len())
                .ok_or_else(|| "aggregate logical size overflowed u64".to_owned())?;
            total_disk_size_bytes = total_disk_size_bytes
                .checked_add(metadata_disk_size(&target.metadata))
                .ok_or_else(|| "aggregate disk size overflowed u64".to_owned())?;
        }
        if target.path.parent().map(Path::to_path_buf) != first_parent {
            common_parent = None;
        }
        retain_common_value(&mut common_kind, target.kind);
        retain_common_optional(&mut common_created, target.metadata.created().ok());
        retain_common_optional(&mut common_modified, target.metadata.modified().ok());
        retain_common_optional(&mut common_accessed, target.metadata.accessed().ok());
        let permissions = metadata_properties_permissions(
            &target.metadata,
            target.metadata.file_type().is_symlink(),
        );
        if permissions != common_permissions {
            common_permissions = None;
        }
        if let (Some(permissions), Some(identity)) =
            (permissions, metadata_properties_identity(&target.metadata))
        {
            permission_baselines.push(FilePropertiesPermissionBaseline {
                path: target.path.clone(),
                identity,
                permissions,
            });
        }
    }

    if permission_baselines.len() != targets.len() {
        common_permissions = None;
        permission_baselines.clear();
    }

    Ok(FilePropertiesAggregateSnapshot {
        target_count: targets.len(),
        file_count,
        directory_count,
        symlink_count,
        other_count,
        total_size_bytes,
        total_disk_size_bytes,
        recursive_contents: FilePropertiesDirectoryContents {
            file_count: 0,
            directory_count: 0,
            total_size_bytes: 0,
            total_disk_size_bytes: 0,
        },
        common_parent,
        common_kind,
        common_created,
        common_modified,
        common_accessed,
        permissions: common_permissions,
        permission_baselines,
    })
}

fn retain_common_value<T: Copy + PartialEq>(common: &mut Option<T>, candidate: T) {
    if common.is_some_and(|value| value != candidate) {
        *common = None;
    }
}

fn retain_common_optional<T: PartialEq>(common: &mut Option<T>, candidate: Option<T>) {
    if common.as_ref() != candidate.as_ref() {
        *common = None;
    }
}

fn add_directory_summary(
    snapshot: &mut FilePropertiesAggregateSnapshot,
    summary: DirectoryContentsSummary,
) -> Result<(), String> {
    snapshot.total_size_bytes = snapshot
        .total_size_bytes
        .checked_add(summary.total_size_bytes)
        .ok_or_else(|| "aggregate logical size overflowed u64".to_owned())?;
    snapshot.total_disk_size_bytes = snapshot
        .total_disk_size_bytes
        .checked_add(summary.total_disk_size_bytes)
        .ok_or_else(|| "aggregate disk size overflowed u64".to_owned())?;
    snapshot.recursive_contents.file_count = snapshot
        .recursive_contents
        .file_count
        .checked_add(summary.file_count)
        .ok_or_else(|| "aggregate file count overflowed usize".to_owned())?;
    snapshot.recursive_contents.directory_count = snapshot
        .recursive_contents
        .directory_count
        .checked_add(summary.directory_count)
        .ok_or_else(|| "aggregate directory count overflowed usize".to_owned())?;
    snapshot.recursive_contents.total_size_bytes = snapshot
        .recursive_contents
        .total_size_bytes
        .checked_add(summary.total_size_bytes)
        .ok_or_else(|| "aggregate logical size overflowed u64".to_owned())?;
    snapshot.recursive_contents.total_disk_size_bytes = snapshot
        .recursive_contents
        .total_disk_size_bytes
        .checked_add(summary.total_disk_size_bytes)
        .ok_or_else(|| "aggregate disk size overflowed u64".to_owned())?;
    Ok(())
}

fn file_kind_for_metadata(metadata: &std::fs::Metadata) -> FileKind {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else if file_type.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    }
}

#[cfg(unix)]
fn metadata_properties_identity(metadata: &std::fs::Metadata) -> Option<FilePropertiesIdentity> {
    Some(FilePropertiesIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        kind: file_kind_for_metadata(metadata),
    })
}

#[cfg(not(unix))]
fn metadata_properties_identity(_metadata: &std::fs::Metadata) -> Option<FilePropertiesIdentity> {
    None
}

async fn load_directory_properties_contents(
    request: FilePropertiesRequest,
    cancellation: CancellationToken,
    output: &mut IcedSender<Message>,
) -> Result<FilePropertiesDirectoryContents, String> {
    let progress = {
        let request = request.clone();
        let mut output = output.clone();
        move |contents: FilePropertiesDirectoryContents| {
            let _ = output.try_send(Message::FileProperties(
                FilePropertiesMessage::DirectoryContentsUpdated(request.clone(), contents),
            ));
        }
    };

    let join_path = request
        .targets
        .single_path()
        .expect("directory contents require one target")
        .to_path_buf();
    let directory_path = join_path.clone();
    tokio::task::spawn_blocking(move || {
        read_directory_properties_contents(&directory_path, cancellation, progress)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| match error {
        DirectorySummaryError::Cancelled => "operation cancelled".to_owned(),
        DirectorySummaryError::Io(error) => error.to_string(),
        DirectorySummaryError::Overflow(field) => {
            format!("directory summary {field} overflowed")
        }
    })
    .map_err(|error| {
        if error.is_empty() {
            format!("could not read directory contents for {:?}", join_path)
        } else {
            error
        }
    })
}

#[cfg(unix)]
fn metadata_properties_permissions(
    metadata: &std::fs::Metadata,
    is_symlink: bool,
) -> Option<FilePropertiesPermissions> {
    (!is_symlink).then(|| FilePropertiesPermissions::from_mode(metadata.permissions().mode()))
}

#[cfg(not(unix))]
fn metadata_properties_permissions(
    _metadata: &std::fs::Metadata,
    _is_symlink: bool,
) -> Option<FilePropertiesPermissions> {
    None
}

#[cfg(unix)]
fn write_file_properties_permissions(
    path: PathBuf,
    permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissions, String> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("symbolic link permissions cannot be changed".to_owned());
    }

    let mut fs_permissions = metadata.permissions();
    fs_permissions.set_mode(permissions.mode());
    std::fs::set_permissions(&path, fs_permissions).map_err(|error| error.to_string())?;

    let refreshed = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    Ok(FilePropertiesPermissions::from_mode(
        refreshed.permissions().mode(),
    ))
}

#[cfg(unix)]
fn write_file_properties_target_set_permissions(
    baselines: Vec<FilePropertiesPermissionBaseline>,
    permissions: FilePropertiesPermissions,
) -> PermissionBatchOutcome {
    let mut succeeded_paths = Vec::new();
    let mut failures = Vec::new();
    for baseline in baselines {
        match write_verified_file_properties_permissions(&baseline, permissions) {
            Ok(()) => succeeded_paths.push(baseline.path),
            Err(error) => failures.push(PermissionBatchPathFailure {
                path: baseline.path,
                error,
            }),
        }
    }
    PermissionBatchOutcome {
        succeeded_paths,
        failures,
    }
}

#[cfg(unix)]
fn write_verified_file_properties_permissions(
    baseline: &FilePropertiesPermissionBaseline,
    permissions: FilePropertiesPermissions,
) -> Result<(), String> {
    use rustix::fs::{fstat, open, FileType, Mode, OFlags};
    use std::os::fd::AsRawFd;

    let descriptor = open(
        &baseline.path,
        OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| format!("could not open target without following links: {error}"))?;
    let status = fstat(&descriptor)
        .map_err(|error| format!("could not verify opened target identity: {error}"))?;
    let opened_kind = match FileType::from_raw_mode(status.st_mode) {
        FileType::RegularFile => FileKind::File,
        FileType::Directory => FileKind::Directory,
        FileType::Symlink => FileKind::Symlink,
        _ => FileKind::Other,
    };
    if status.st_dev != baseline.identity.device
        || status.st_ino != baseline.identity.inode
        || opened_kind != baseline.identity.kind
    {
        return Err("target identity changed before permission update".to_owned());
    }
    let current_permissions = FilePropertiesPermissions::from_mode(status.st_mode as u32);
    if current_permissions != baseline.permissions {
        return Err("target permissions changed before permission update".to_owned());
    }
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", descriptor.as_raw_fd()));
    std::fs::set_permissions(
        descriptor_path,
        std::fs::Permissions::from_mode(permissions.mode()),
    )
    .map_err(|error| format!("could not set permissions: {error}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_file_properties_target_set_permissions(
    baselines: Vec<FilePropertiesPermissionBaseline>,
    _permissions: FilePropertiesPermissions,
) -> PermissionBatchOutcome {
    PermissionBatchOutcome {
        succeeded_paths: Vec::new(),
        failures: baselines
            .into_iter()
            .map(|baseline| PermissionBatchPathFailure {
                path: baseline.path,
                error: "permission editing is only available on Unix filesystems".to_owned(),
            })
            .collect(),
    }
}

#[cfg(unix)]
fn write_file_properties_permissions_to_enclosed_items(
    path: PathBuf,
    permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissions, String> {
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect {:?}: {error}", path))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "could not apply permissions to enclosed items for {:?}: symbolic links are not directories",
            path
        ));
    }
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "could not apply permissions to enclosed items for {:?}: item is not a folder",
            path
        ));
    }

    write_permissions_postorder(&path, permissions)?;
    let refreshed = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("could not refresh permissions for {:?}: {error}", path))?;
    Ok(FilePropertiesPermissions::from_mode(
        refreshed.permissions().mode(),
    ))
}

#[cfg(unix)]
fn write_permissions_postorder(
    path: &Path,
    permissions: FilePropertiesPermissions,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {:?}: {error}", path))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(());
    }

    if file_type.is_dir() {
        for entry in std::fs::read_dir(path)
            .map_err(|error| format!("could not list folder {:?}: {error}", path))?
        {
            let entry = entry
                .map_err(|error| format!("could not read entry in folder {:?}: {error}", path))?;
            write_permissions_postorder(&entry.path(), permissions)?;
        }
    }

    let mut fs_permissions = metadata.permissions();
    fs_permissions.set_mode(permissions.mode());
    std::fs::set_permissions(path, fs_permissions)
        .map_err(|error| format!("could not set permissions for {:?}: {error}", path))
}

#[cfg(not(unix))]
fn write_file_properties_permissions(
    _path: PathBuf,
    _permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissions, String> {
    Err("permission editing is only available on Unix filesystems".to_owned())
}

#[cfg(not(unix))]
fn write_file_properties_permissions_to_enclosed_items(
    _path: PathBuf,
    _permissions: FilePropertiesPermissions,
) -> Result<FilePropertiesPermissions, String> {
    Err("permission editing is only available on Unix filesystems".to_owned())
}

fn read_directory_properties_contents(
    path: &Path,
    cancellation: CancellationToken,
    mut progress: impl FnMut(FilePropertiesDirectoryContents),
) -> Result<FilePropertiesDirectoryContents, DirectorySummaryError> {
    read_directory_contents_summary(path, cancellation, |summary| {
        progress(file_properties_directory_contents(summary));
    })
    .map(file_properties_directory_contents)
}

fn file_properties_directory_contents(
    summary: DirectoryContentsSummary,
) -> FilePropertiesDirectoryContents {
    FilePropertiesDirectoryContents {
        file_count: summary.file_count,
        directory_count: summary.directory_count,
        total_size_bytes: summary.total_size_bytes,
        total_disk_size_bytes: summary.total_disk_size_bytes,
    }
}

#[cfg(all(test, unix))]
#[path = "properties_tests.rs"]
mod tests;
