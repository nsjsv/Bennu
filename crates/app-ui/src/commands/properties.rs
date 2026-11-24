use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use file_core::mime::MimeDatabase;
use file_core::{FileKind, MediaByName};
use iced::futures::channel::mpsc::Sender as IcedSender;
use iced::futures::SinkExt;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::directory_summary::{measure_directories, metadata_disk_size, MeasureFailure};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesFileDetails, FilePropertiesIdentity,
    FilePropertiesKindDetails, FilePropertiesLookup, FilePropertiesMessage, FilePropertiesOwner,
    FilePropertiesPermissionBaseline, FilePropertiesPermissionWriteOutcome,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesRequest,
    FilePropertiesReread, FilePropertiesSnapshot, Message, PermissionBatchOutcome,
    PermissionBatchPathFailure,
};
use file_core::DirectoryUsage;

const FILE_PROPERTIES_CHANNEL_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesPermissionTargets {
    Single(PathBuf),
    TargetSet(Vec<FilePropertiesPermissionBaseline>),
}

/// 信息面板读一个条目：属性窗口同一个快速读取，再在后台读详细信息（图片、音视频）和 PDF
/// 页数。不读文件夹里面、不查默认打开程序：目标换得勤，这两样不值。被取消（目标换了）时
/// 后台的 ffprobe、pdfinfo 跟着停。
pub(crate) fn info_panel_command(
    request: u64,
    path: PathBuf,
    cancellation: CancellationToken,
) -> Task<Message> {
    use crate::app::info_panel::InfoPanelMessage;
    Task::stream(iced::stream::channel(
        FILE_PROPERTIES_CHANNEL_SIZE,
        async move |mut output| {
            let loaded = load_file_properties(path.clone()).await;
            let media = loaded.as_ref().ok().and_then(|(_, lookups)| lookups.media);
            let is_pdf = loaded.as_ref().is_ok_and(|(snapshot, _)| {
                matches!(snapshot.details, FilePropertiesKindDetails::File(_))
            }) && bennu_preview::document_preview::document_preview_format_for_path(&path)
                == Some(bennu_preview::document_preview::DocumentPreviewFormat::Pdf);
            let outcome = loaded.map(|(snapshot, _)| Box::new(snapshot));
            let message = Message::InfoPanel(InfoPanelMessage::Loaded { request, outcome });
            if output.send(message).await.is_err() {
                return;
            }
            let details = async {
                if let Some(media) = media {
                    let outcome =
                        bennu_preview::media_details::media_detail_rows(path.clone(), media).await;
                    let message =
                        Message::InfoPanel(InfoPanelMessage::DetailsLoaded { request, outcome });
                    let _ = output.clone().send(message).await;
                }
            };
            let pages = async {
                if is_pdf {
                    let pages = bennu_preview::commands::document_preview::pdf_page_count(
                        path.clone(),
                        cancellation.clone(),
                    )
                    .await;
                    if let Some(pages) = pages {
                        let message =
                            Message::InfoPanel(InfoPanelMessage::PagesLoaded { request, pages });
                        let _ = output.clone().send(message).await;
                    }
                }
            };
            tokio::select! {
                () = cancellation.cancelled() => {}
                _ = async { tokio::join!(details, pages) } => {}
            }
        },
    ))
}

/// 读一遍属性。重读时 `previous` 是上次读到的单个目标：慢项先摆上次的，`reread` 说不用再读的
/// 就不读（多个目标总是整个重读）。
pub(crate) fn file_properties_command(
    request: FilePropertiesRequest,
    cancellation: CancellationToken,
    reread: FilePropertiesReread,
    previous: Option<FilePropertiesSnapshot>,
) -> Task<Message> {
    let single_path = request.targets.single_path().map(Path::to_path_buf);
    if let Some(path) = single_path {
        single_file_properties_command(request, path, cancellation, reread, previous)
    } else {
        aggregate_file_properties_command(request, cancellation)
    }
}

fn single_file_properties_command(
    request: FilePropertiesRequest,
    path: PathBuf,
    cancellation: CancellationToken,
    reread: FilePropertiesReread,
    previous: Option<FilePropertiesSnapshot>,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        FILE_PROPERTIES_CHANNEL_SIZE,
        async move |mut output| {
            let (outcome, lookups) = match load_file_properties(path.clone()).await {
                Ok((mut snapshot, mut lookups)) => {
                    if let Some(previous) = &previous {
                        keep_previous_slow_properties(
                            &mut snapshot,
                            &mut lookups,
                            previous,
                            reread,
                        );
                    }
                    (Ok(snapshot), Some(lookups))
                }
                Err(error) => (Err(error), None),
            };
            if output
                .send(Message::FileProperties(FilePropertiesMessage::Loaded(
                    request.clone(),
                    outcome.map(FilePropertiesPresentation::Single),
                )))
                .await
                .is_err()
            {
                return;
            }
            let Some(lookups) = lookups else {
                return;
            };
            tokio::select! {
                () = cancellation.cancelled() => {}
                () = read_slow_properties(&request, &path, lookups, &cancellation, &output) => {}
            }
        },
    ))
}

#[path = "properties_reread.rs"]
mod reread;
use reread::keep_previous_slow_properties;

/// 快照发出去以后在后台读的慢项；各读各的，谁先到谁先填。
struct SlowPropertyLookups {
    directory_contents: bool,
    default_application_for: Option<String>,
    media: Option<MediaByName>,
}

async fn read_slow_properties(
    request: &FilePropertiesRequest,
    path: &Path,
    lookups: SlowPropertyLookups,
    cancellation: &CancellationToken,
    output: &IcedSender<Message>,
) {
    let contents = async {
        if lookups.directory_contents {
            let mut output = output.clone();
            let outcome =
                load_directory_properties_contents(request.clone(), cancellation, &mut output)
                    .await;
            let _ = output
                .send(Message::FileProperties(
                    FilePropertiesMessage::DirectoryContentsLoaded(request.clone(), outcome),
                ))
                .await;
        }
    };
    let application = async {
        if let Some(mime_type) = &lookups.default_application_for {
            let outcome = desktop_linux::default_application_name(
                path,
                mime_type,
                bennu_localization::current_language().locale_tags(),
            )
            .await
            .map_err(|error| error.to_string());
            let _ = output
                .clone()
                .send(Message::FileProperties(
                    FilePropertiesMessage::DefaultApplicationLoaded(request.clone(), outcome),
                ))
                .await;
        }
    };
    let media = async {
        if let Some(media) = lookups.media {
            let outcome =
                bennu_preview::media_details::media_detail_rows(path.to_path_buf(), media).await;
            let _ = output
                .clone()
                .send(Message::FileProperties(
                    FilePropertiesMessage::MediaDetailsLoaded(request.clone(), outcome),
                ))
                .await;
        }
    };
    tokio::join!(contents, application, media);
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
            let outcome =
                read_aggregate_file_properties(targets.paths(), cancellation, |snapshot| {
                    let _ = progress_output.try_send(Message::FileProperties(
                        FilePropertiesMessage::AggregateUpdated(progress_request.clone(), snapshot),
                    ));
                })
                .await
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

async fn load_file_properties(
    path: PathBuf,
) -> Result<(FilePropertiesSnapshot, SlowPropertyLookups), String> {
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

/// 快的一次读完：stat、所有者名、类型和类型名、链接指向、所在磁盘；慢的记下来后台读。
fn read_file_properties(
    path: PathBuf,
) -> Result<(FilePropertiesSnapshot, SlowPropertyLookups), String> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| path.as_os_str().to_os_string());
    let location = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut lookups = SlowPropertyLookups {
        directory_contents: false,
        default_application_for: None,
        media: None,
    };
    let details = match file_kind_for_metadata(&metadata) {
        FileKind::File => {
            let types = MimeDatabase::system();
            let mime_type = types.type_of_file(&path);
            let media = file_core::media_by_name(&path);
            lookups.default_application_for = Some(mime_type.clone());
            lookups.media = (media != MediaByName::Other).then_some(media);
            FilePropertiesKindDetails::File(FilePropertiesFileDetails {
                description: types.description(
                    &mime_type,
                    bennu_localization::current_language().locale_tags(),
                ),
                mime_type,
                default_application: FilePropertiesLookup::Pending,
                media_details: lookups.media.map(|_| FilePropertiesLookup::Pending),
            })
        }
        FileKind::Directory => {
            lookups.directory_contents = true;
            FilePropertiesKindDetails::Directory(FilePropertiesDirectoryContentsState::Loading(
                None,
            ))
        }
        FileKind::Symlink => FilePropertiesKindDetails::Symlink(
            std::fs::read_link(&path).map_err(|error| error.to_string()),
        ),
        FileKind::Other => FilePropertiesKindDetails::Other,
    };
    // 文件夹可能自己就是挂载点；其余条目（链接本身也是）在所在文件夹的磁盘上。
    let volume = file_core::volume_usage(if metadata.is_dir() { &path } else { &location })
        .map_err(|error| error.to_string());

    Ok((
        FilePropertiesSnapshot {
            name,
            location,
            created: metadata.created().ok(),
            modified: metadata.modified().ok(),
            accessed: metadata.accessed().ok(),
            changed: file_core::status_changed(&metadata),
            size_bytes: metadata.len(),
            disk_size_bytes: metadata_disk_size(&metadata),
            details,
            owner: FilePropertiesOwner {
                user: file_core::user_name(metadata.uid()),
                group: file_core::group_name(metadata.gid()),
            },
            volume,
            permissions: metadata_properties_permissions(&metadata, metadata.is_symlink()),
        },
        lookups,
    ))
}

async fn read_aggregate_file_properties(
    paths: &[PathBuf],
    cancellation: CancellationToken,
    mut progress: impl FnMut(FilePropertiesAggregateSnapshot),
) -> Result<FilePropertiesAggregateSnapshot, String> {
    let inspection_paths = paths.to_vec();
    let inspection_cancellation = cancellation.clone();
    let (snapshot, directories) = tokio::task::spawn_blocking(move || {
        let mut inspected = Vec::with_capacity(inspection_paths.len());
        for path in inspection_paths {
            if inspection_cancellation.is_cancelled() {
                return Err("operation cancelled".to_owned());
            }
            inspected.push(inspect_aggregate_target(path)?);
        }
        let snapshot = aggregate_snapshot_for_targets(&inspected)?;
        let directories = inspected
            .into_iter()
            .filter(|target| target.kind == FileKind::Directory)
            .map(|target| target.path)
            .collect::<Vec<_>>();
        Ok((snapshot, directories))
    })
    .await
    .map_err(|error| error.to_string())??;

    let usage = measure_directories(&directories, &cancellation, |usage| {
        progress(with_directory_usage(&snapshot, usage));
    })
    .await
    .map_err(|failure| match failure {
        MeasureFailure::Cancelled => "operation cancelled".to_owned(),
        MeasureFailure::Unreadable(path, error) => {
            format!("could not read directory contents for {path:?}: {error}")
        }
    })?;
    let snapshot = with_directory_usage(&snapshot, usage);
    progress(snapshot.clone());
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
    let mut common_changed = file_core::status_changed(&first.metadata);
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
        retain_common_optional(
            &mut common_changed,
            file_core::status_changed(&target.metadata),
        );
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
            unreadable_entries: 0,
        },
        common_parent,
        common_kind,
        common_created,
        common_modified,
        common_accessed,
        common_changed,
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

/// 选中的那几个文件夹里面的用量加到选中项本身的合计上。
fn with_directory_usage(
    snapshot: &FilePropertiesAggregateSnapshot,
    usage: DirectoryUsage,
) -> FilePropertiesAggregateSnapshot {
    let mut snapshot = snapshot.clone();
    snapshot.total_size_bytes = snapshot.total_size_bytes.saturating_add(usage.bytes);
    snapshot.total_disk_size_bytes = snapshot
        .total_disk_size_bytes
        .saturating_add(usage.disk_bytes);
    snapshot.recursive_contents = FilePropertiesDirectoryContents::from(usage);
    snapshot
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
    cancellation: &CancellationToken,
    output: &mut IcedSender<Message>,
) -> Result<FilePropertiesDirectoryContents, String> {
    let path = request
        .targets
        .single_path()
        .expect("directory contents require one target")
        .to_path_buf();
    let mut progress_output = output.clone();
    measure_directories(&[path], cancellation, |usage| {
        let _ = progress_output.try_send(Message::FileProperties(
            FilePropertiesMessage::DirectoryContentsUpdated(request.clone(), usage.into()),
        ));
    })
    .await
    .map(FilePropertiesDirectoryContents::from)
    .map_err(|failure| match failure {
        MeasureFailure::Cancelled => "operation cancelled".to_owned(),
        MeasureFailure::Unreadable(_, error) => error,
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

impl From<DirectoryUsage> for FilePropertiesDirectoryContents {
    fn from(usage: DirectoryUsage) -> Self {
        Self {
            file_count: usage.files as usize,
            directory_count: usage.folders as usize,
            total_size_bytes: usage.bytes,
            total_disk_size_bytes: usage.disk_bytes,
            unreadable_entries: usage.unreadable,
        }
    }
}

#[cfg(all(test, unix))]
#[path = "properties_tests.rs"]
mod tests;
