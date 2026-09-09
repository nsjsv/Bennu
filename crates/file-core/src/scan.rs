#[cfg(unix)]
use std::ffi::CStr;
use std::ffi::OsString;
#[cfg(unix)]
use std::mem::MaybeUninit;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::{
    resolve_directory_metadata, sort_discovered_entry_indices, sort_entries, DirectoryEntry,
    DirectoryMetadataRequest, DirectoryMetadataRequirement, DirectoryMetadataResolver,
    DiscoveredDirectoryEntry, FileKind, SortDirection, SortField,
};

const DIRECTORY_SCAN_BATCH_SIZE: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    pub include_hidden: bool,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub directories_first: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            sort_field: SortField::Name,
            sort_direction: SortDirection::Ascending,
            directories_first: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryDiscovery {
    pub path: PathBuf,
    pub entries: Arc<Vec<DiscoveredDirectoryEntry>>,
    pub order: Arc<Vec<usize>>,
    pub metadata_resolver: DirectoryMetadataResolver,
    pub warnings: Vec<ScanWarning>,
}

#[derive(Debug, Clone)]
pub struct DirectoryDiscoveryBatch {
    pub path: PathBuf,
    pub entries: Vec<DiscoveredDirectoryEntry>,
    pub warnings: Vec<ScanWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryScan {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntry>,
    pub skipped: Vec<ScanWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryScanBatch {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntry>,
    pub skipped: Vec<ScanWarning>,
}

pub async fn discover_directory_with_progress(
    path: impl AsRef<Path>,
    options: ScanOptions,
    cancellation: CancellationToken,
    mut on_batch: impl FnMut(DirectoryDiscoveryBatch),
) -> Result<DirectoryDiscovery, FileError> {
    let path = path.as_ref().to_path_buf();
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let mut reader = fs::read_dir(&path)
        .await
        .map_err(|source| FileError::ReadDirectory {
            path: path.clone(),
            source,
        })?;
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut batch_entries = Vec::new();
    let mut batch_warnings = Vec::new();

    loop {
        let next_entry = tokio::select! {
            _ = cancellation.cancelled() => return Err(FileError::Cancelled),
            next_entry = reader.next_entry() => next_entry,
        };
        let dir_entry = match next_entry {
            Ok(Some(dir_entry)) => dir_entry,
            Ok(None) => break,
            Err(source) => {
                return Err(FileError::ReadEntry {
                    path: path.clone(),
                    source,
                })
            }
        };
        let name = dir_entry.file_name();
        let is_hidden = is_hidden_name(&name);
        if is_hidden && !options.include_hidden {
            continue;
        }
        let entry_path = dir_entry.path();
        let file_type = tokio::select! {
            _ = cancellation.cancelled() => return Err(FileError::Cancelled),
            file_type = dir_entry.file_type() => file_type,
        };
        let entry = match file_type {
            Ok(file_type) => {
                let kind = if file_type.is_dir() {
                    FileKind::Directory
                } else if file_type.is_file() {
                    FileKind::File
                } else if file_type.is_symlink() {
                    FileKind::Symlink
                } else {
                    FileKind::Other
                };
                DiscoveredDirectoryEntry::new(
                    entry_path,
                    name,
                    kind,
                    is_hidden,
                    file_type.is_symlink(),
                )
            }
            Err(source) => {
                let warning = ScanWarning {
                    path: entry_path.clone(),
                    message: source.to_string(),
                };
                warnings.push(warning.clone());
                batch_warnings.push(warning);
                DiscoveredDirectoryEntry::with_unavailable_filesystem_metadata(
                    entry_path,
                    name,
                    is_hidden,
                    source.to_string(),
                )
            }
        };
        entries.push(entry.clone());
        batch_entries.push(entry);

        if batch_entries.len() + batch_warnings.len() >= DIRECTORY_SCAN_BATCH_SIZE {
            emit_directory_discovery_batch(
                &path,
                &mut batch_entries,
                &mut batch_warnings,
                &mut on_batch,
            );
        }
    }
    emit_directory_discovery_batch(
        &path,
        &mut batch_entries,
        &mut batch_warnings,
        &mut on_batch,
    );
    let order = Arc::new(sort_discovered_entry_indices(&entries, &options));
    let entries = Arc::new(entries);
    let metadata_resolver = DirectoryMetadataResolver::new(path.clone(), Arc::clone(&entries));

    Ok(DirectoryDiscovery {
        path,
        entries,
        order,
        metadata_resolver,
        warnings,
    })
}

fn emit_directory_discovery_batch(
    path: &Path,
    entries: &mut Vec<DiscoveredDirectoryEntry>,
    warnings: &mut Vec<ScanWarning>,
    on_batch: &mut impl FnMut(DirectoryDiscoveryBatch),
) {
    if entries.is_empty() && warnings.is_empty() {
        return;
    }
    on_batch(DirectoryDiscoveryBatch {
        path: path.to_path_buf(),
        entries: std::mem::take(entries),
        warnings: std::mem::take(warnings),
    });
}

pub async fn scan_directory(
    path: impl AsRef<Path>,
    options: ScanOptions,
) -> Result<DirectoryScan, FileError> {
    scan_directory_with_progress(path, options, CancellationToken::new(), |_| {}).await
}

pub async fn scan_directory_with_progress(
    path: impl AsRef<Path>,
    options: ScanOptions,
    cancellation: CancellationToken,
    mut on_batch: impl FnMut(DirectoryScanBatch),
) -> Result<DirectoryScan, FileError> {
    let discovery =
        discover_directory_with_progress(path, options.clone(), cancellation.clone(), |_| {})
            .await?;
    let target_count = discovery.entries.len();
    let metadata_resolution = resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 0,
            requirement: DirectoryMetadataRequirement::IdentityNames,
            targets: (0..target_count).collect(),
        },
        cancellation,
    )
    .await?;
    let mut entries = discovery
        .entries
        .iter()
        .filter_map(DiscoveredDirectoryEntry::complete_entry)
        .collect::<Vec<_>>();
    sort_entries(&mut entries, &options);
    let mut skipped = discovery.warnings;
    for warning in metadata_resolution.warnings {
        if !skipped.contains(&warning) {
            skipped.push(warning);
        }
    }
    emit_complete_scan_batches(&discovery.path, &entries, &skipped, &mut on_batch);

    Ok(DirectoryScan {
        path: discovery.path,
        entries,
        skipped,
    })
}

fn emit_complete_scan_batches(
    path: &Path,
    entries: &[DirectoryEntry],
    skipped: &[ScanWarning],
    on_batch: &mut impl FnMut(DirectoryScanBatch),
) {
    let mut pending_skipped = skipped.to_vec();
    for chunk in entries.chunks(DIRECTORY_SCAN_BATCH_SIZE) {
        on_batch(DirectoryScanBatch {
            path: path.to_path_buf(),
            entries: chunk.to_vec(),
            skipped: std::mem::take(&mut pending_skipped),
        });
    }
    if entries.is_empty() && !pending_skipped.is_empty() {
        on_batch(DirectoryScanBatch {
            path: path.to_path_buf(),
            entries: Vec::new(),
            skipped: pending_skipped,
        });
    }
}

pub(crate) fn entry_from_metadata(
    path: PathBuf,
    name: OsString,
    is_hidden: bool,
    metadata: &std::fs::Metadata,
    is_broken_symlink: bool,
) -> DirectoryEntry {
    crate::directory_metadata::complete_entry_from_metadata(
        path,
        name,
        is_hidden,
        metadata,
        is_broken_symlink,
    )
}

#[cfg(unix)]
pub(crate) fn lookup_unix_user_name(uid: u32) -> String {
    let mut buffer = vec![0_u8; unix_account_lookup_buffer_len()];
    loop {
        let mut passwd = MaybeUninit::<libc::passwd>::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == 0 {
            if result.is_null() {
                return uid.to_string();
            }
            let name_ptr = unsafe { (*result).pw_name };
            if name_ptr.is_null() {
                return uid.to_string();
            }
            return unsafe { CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .into_owned();
        }
        if status == libc::ERANGE {
            buffer.resize(buffer.len().saturating_mul(2).max(1), 0);
            continue;
        }
        return uid.to_string();
    }
}

#[cfg(unix)]
pub(crate) fn lookup_unix_group_name(gid: u32) -> String {
    let mut buffer = vec![0_u8; unix_group_lookup_buffer_len()];
    loop {
        let mut group = MaybeUninit::<libc::group>::zeroed();
        let mut result: *mut libc::group = std::ptr::null_mut();
        let status = unsafe {
            libc::getgrgid_r(
                gid as libc::gid_t,
                group.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == 0 {
            if result.is_null() {
                return gid.to_string();
            }
            let name_ptr = unsafe { (*result).gr_name };
            if name_ptr.is_null() {
                return gid.to_string();
            }
            return unsafe { CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .into_owned();
        }
        if status == libc::ERANGE {
            buffer.resize(buffer.len().saturating_mul(2).max(1), 0);
            continue;
        }
        return gid.to_string();
    }
}

#[cfg(unix)]
fn unix_account_lookup_buffer_len() -> usize {
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if size > 0 {
        size as usize
    } else {
        1024
    }
}

#[cfg(unix)]
fn unix_group_lookup_buffer_len() -> usize {
    let size = unsafe { libc::sysconf(libc::_SC_GETGR_R_SIZE_MAX) };
    if size > 0 {
        size as usize
    } else {
        1024
    }
}

/// 隐藏判定的唯一权威：点前缀。列表过滤与目录摘要统计必须共用此规则。
#[cfg(unix)]
pub fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.as_bytes().first() == Some(&b'.')
}

#[cfg(not(unix))]
pub fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

#[derive(Debug, Error)]
pub enum FileError {
    #[error("could not read directory {path:?}: {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read directory entry in {path:?}: {source}")]
    ReadEntry {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read metadata for {path:?}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create directory {path:?}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create file {path:?}: {source}")]
    CreateFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not rename {from:?} to {to:?}: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not copy {from:?} to {to:?}: {source}")]
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not move {from:?} to {to:?}: {source}")]
    Move {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not delete {path:?}: {source}")]
    Delete {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not move {path:?} to trash: {message}")]
    Trash { path: PathBuf, message: String },
    #[error("could not watch {path:?}: {message}")]
    Watch { path: PathBuf, message: String },
    #[error("could not create archive {path:?}: {message}")]
    Archive { path: PathBuf, message: String },
    #[error("could not convert {path:?}: {message}")]
    Convert { path: PathBuf, message: String },
    #[error("could not read {path:?} for checksum computation: {source}")]
    Checksum {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("archive {path:?} requires a password")]
    ArchivePasswordRequired { path: PathBuf },
    #[error("archive {path:?} password is incorrect")]
    ArchiveInvalidPassword { path: PathBuf },
    #[error("invalid input for {path:?}: {message}")]
    InvalidInput { path: PathBuf, message: String },
    #[error("operation cancelled")]
    Cancelled,
    #[error("application is stopping")]
    ApplicationStopping,
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
}
