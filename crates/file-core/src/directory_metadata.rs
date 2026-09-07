use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use tokio::fs;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::{
    DirectoryEntry, DirectoryMetadataAvailability, EntryMetadata, FileError, FileKind, ScanWarning,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryFilesystemMetadata {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub readonly: bool,
    pub permissions_mode: Option<u32>,
    pub user_id: Option<u32>,
    pub group_id: Option<u32>,
    pub is_broken_symlink: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryIdentityNames {
    pub owner_name: Option<String>,
    pub group_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMetadataUnavailable {
    message: String,
}

impl DirectoryMetadataUnavailable {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryMetadataState<'a, T> {
    Pending,
    Complete(&'a T),
    Unavailable(&'a DirectoryMetadataUnavailable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectoryMetadataRequirement {
    Filesystem,
    IdentityNames,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMetadataRequest {
    pub request_generation: u64,
    pub requirement: DirectoryMetadataRequirement,
    pub targets: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMetadataResolution {
    pub request_generation: u64,
    pub requirement: DirectoryMetadataRequirement,
    pub requested_targets: usize,
    pub resolved_indices: Vec<usize>,
    pub warnings: Vec<ScanWarning>,
    pub filesystem_calls: usize,
    pub user_name_lookups: usize,
    pub group_name_lookups: usize,
    pub identity_worker_runs: usize,
}

#[derive(Debug, Default)]
struct DirectoryMetadataCounters {
    filesystem_calls: AtomicUsize,
    user_name_lookups: AtomicUsize,
    group_name_lookups: AtomicUsize,
    identity_worker_runs: AtomicUsize,
}

#[derive(Debug, Clone)]
pub struct DirectoryMetadataResolver {
    path: PathBuf,
    entries: Arc<Vec<DiscoveredDirectoryEntry>>,
    user_names: Arc<Mutex<HashMap<u32, String>>>,
    group_names: Arc<Mutex<HashMap<u32, String>>>,
}

impl DirectoryMetadataResolver {
    pub(crate) fn new(path: PathBuf, entries: Arc<Vec<DiscoveredDirectoryEntry>>) -> Self {
        Self {
            path,
            entries,
            user_names: Arc::new(Mutex::new(HashMap::new())),
            group_names: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Debug)]
struct DiscoveredDirectoryEntryInner {
    path: PathBuf,
    name: OsString,
    kind: FileKind,
    is_hidden: bool,
    is_symlink: bool,
    filesystem_metadata:
        OnceCell<Result<DirectoryFilesystemMetadata, DirectoryMetadataUnavailable>>,
    identity_names: OnceCell<Result<DirectoryIdentityNames, DirectoryMetadataUnavailable>>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredDirectoryEntry {
    inner: Arc<DiscoveredDirectoryEntryInner>,
}

impl DiscoveredDirectoryEntry {
    pub(crate) fn new(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        is_hidden: bool,
        is_symlink: bool,
    ) -> Self {
        Self {
            inner: Arc::new(DiscoveredDirectoryEntryInner {
                path,
                name,
                kind,
                is_hidden,
                is_symlink,
                filesystem_metadata: OnceCell::new(),
                identity_names: OnceCell::new(),
            }),
        }
    }

    pub(crate) fn with_unavailable_filesystem_metadata(
        path: PathBuf,
        name: OsString,
        is_hidden: bool,
        message: impl Into<String>,
    ) -> Self {
        let filesystem_metadata = OnceCell::new();
        filesystem_metadata
            .set(Err(DirectoryMetadataUnavailable::new(message)))
            .expect("new filesystem metadata cell must be empty");
        Self {
            inner: Arc::new(DiscoveredDirectoryEntryInner {
                path,
                name,
                kind: FileKind::Other,
                is_hidden,
                is_symlink: false,
                filesystem_metadata,
                identity_names: OnceCell::new(),
            }),
        }
    }

    // 增量更新入口：从 std::fs::Metadata 构造，filesystem cell 预填完整，
    // display_entry() 无需再发起 demand。
    /// 增量更新入口的公开构造：测试与跨 crate 应用方需要直接构造带完整元数据的条目。
    pub fn with_complete_filesystem_metadata(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        is_hidden: bool,
        is_symlink: bool,
        metadata: &std::fs::Metadata,
    ) -> Self {
        let filesystem_metadata = OnceCell::new();
        filesystem_metadata
            .set(Ok(directory_filesystem_metadata(metadata, false)))
            .expect("new filesystem metadata cell must be empty");
        Self {
            inner: Arc::new(DiscoveredDirectoryEntryInner {
                path,
                name,
                kind,
                is_hidden,
                is_symlink,
                filesystem_metadata,
                identity_names: OnceCell::new(),
            }),
        }
    }

    /// rename 不改变文件元数据：共享已填写的 metadata/identity cells，
    /// 仅替换 path/name。未填写的 cell 克隆后仍是待解析状态。
    pub fn renamed_to(&self, path: PathBuf, name: OsString) -> Self {
        Self {
            inner: Arc::new(DiscoveredDirectoryEntryInner {
                path,
                name,
                kind: self.inner.kind,
                is_hidden: self.inner.is_hidden,
                is_symlink: self.inner.is_symlink,
                filesystem_metadata: self.inner.filesystem_metadata.clone(),
                identity_names: self.inner.identity_names.clone(),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn name(&self) -> &OsStr {
        &self.inner.name
    }

    pub fn kind(&self) -> FileKind {
        self.inner.kind
    }

    pub fn is_hidden(&self) -> bool {
        self.inner.is_hidden
    }

    pub fn is_symlink(&self) -> bool {
        self.inner.is_symlink
    }

    pub fn filesystem_metadata(&self) -> DirectoryMetadataState<'_, DirectoryFilesystemMetadata> {
        metadata_state(&self.inner.filesystem_metadata)
    }

    pub fn identity_names(&self) -> DirectoryMetadataState<'_, DirectoryIdentityNames> {
        metadata_state(&self.inner.identity_names)
    }

    pub fn display_entry(&self) -> DirectoryEntry {
        let mut metadata = EntryMetadata::pending();
        let mut is_broken_symlink = false;
        match self.filesystem_metadata() {
            DirectoryMetadataState::Pending => {}
            DirectoryMetadataState::Complete(filesystem) => {
                metadata.filesystem_availability = DirectoryMetadataAvailability::Complete;
                metadata.len = filesystem.len;
                metadata.modified = filesystem.modified;
                metadata.accessed = filesystem.accessed;
                metadata.created = filesystem.created;
                metadata.readonly = filesystem.readonly;
                metadata.permissions_mode = filesystem.permissions_mode;
                is_broken_symlink = filesystem.is_broken_symlink;
            }
            DirectoryMetadataState::Unavailable(_) => {
                metadata.filesystem_availability = DirectoryMetadataAvailability::Unavailable;
            }
        }
        match self.identity_names() {
            DirectoryMetadataState::Pending => {}
            DirectoryMetadataState::Complete(identity_names) => {
                metadata.identity_names_availability = DirectoryMetadataAvailability::Complete;
                metadata.owner_name = identity_names.owner_name.clone();
                metadata.group_name = identity_names.group_name.clone();
            }
            DirectoryMetadataState::Unavailable(_) => {
                metadata.identity_names_availability = DirectoryMetadataAvailability::Unavailable;
            }
        }
        DirectoryEntry::with_file_name(
            self.path().to_path_buf(),
            self.name().to_os_string(),
            self.kind(),
            metadata,
            self.is_hidden(),
            self.is_symlink(),
            is_broken_symlink,
        )
    }

    pub(crate) fn complete_entry(&self) -> Option<DirectoryEntry> {
        let filesystem_metadata = match self.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => metadata,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => {
                return None
            }
        };
        let identity_names = match self.identity_names() {
            DirectoryMetadataState::Complete(names) => names,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => {
                return None
            }
        };
        Some(directory_entry_from_resolved_metadata(
            self.path().to_path_buf(),
            self.name().to_os_string(),
            self.kind(),
            self.is_hidden(),
            self.is_symlink(),
            filesystem_metadata,
            identity_names,
        ))
    }
}

pub async fn resolve_directory_metadata(
    resolver: DirectoryMetadataResolver,
    request: DirectoryMetadataRequest,
    cancellation: CancellationToken,
) -> Result<DirectoryMetadataResolution, FileError> {
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let DirectoryMetadataRequest {
        request_generation,
        requirement,
        targets,
    } = request;
    let requested_targets = targets.len();
    let targets = targets.into_iter().collect::<BTreeSet<_>>();
    if let Some(invalid_index) = targets
        .iter()
        .copied()
        .find(|index| *index >= resolver.entries.len())
    {
        return Err(FileError::InvalidInput {
            path: resolver.path.clone(),
            message: format!("directory metadata target index is out of range: {invalid_index}"),
        });
    }
    let counters = Arc::new(DirectoryMetadataCounters::default());
    match requirement {
        DirectoryMetadataRequirement::Filesystem => {
            for index in &targets {
                if cancellation.is_cancelled() {
                    return Err(FileError::Cancelled);
                }
                resolver
                    .resolve_filesystem_metadata(
                        &resolver.entries[*index],
                        &cancellation,
                        &counters,
                    )
                    .await?;
            }
        }
        DirectoryMetadataRequirement::IdentityNames => {
            resolver
                .resolve_identity_names(&targets, &cancellation, Arc::clone(&counters))
                .await?;
        }
    }
    let resolved_indices = targets.iter().copied().collect::<Vec<_>>();
    let warnings = targets
        .iter()
        .filter_map(|index| {
            let entry = &resolver.entries[*index];
            match entry.filesystem_metadata() {
                DirectoryMetadataState::Unavailable(unavailable) => Some(ScanWarning {
                    path: entry.path().to_path_buf(),
                    message: unavailable.message().to_owned(),
                }),
                DirectoryMetadataState::Pending | DirectoryMetadataState::Complete(_) => None,
            }
        })
        .collect();
    Ok(DirectoryMetadataResolution {
        request_generation,
        requirement,
        requested_targets,
        resolved_indices,
        warnings,
        filesystem_calls: counters.filesystem_calls.load(Ordering::Relaxed),
        user_name_lookups: counters.user_name_lookups.load(Ordering::Relaxed),
        group_name_lookups: counters.group_name_lookups.load(Ordering::Relaxed),
        identity_worker_runs: counters.identity_worker_runs.load(Ordering::Relaxed),
    })
}

impl DirectoryMetadataResolver {
    async fn resolve_filesystem_metadata(
        &self,
        entry: &DiscoveredDirectoryEntry,
        cancellation: &CancellationToken,
        counters: &DirectoryMetadataCounters,
    ) -> Result<(), FileError> {
        let path = entry.path().to_path_buf();
        let is_symlink = entry.is_symlink();
        entry
            .inner
            .filesystem_metadata
            .get_or_try_init(|| async {
                if cancellation.is_cancelled() {
                    return Err(FileError::Cancelled);
                }
                counters.filesystem_calls.fetch_add(1, Ordering::Relaxed);
                let symlink_metadata = match fs::symlink_metadata(&path).await {
                    Ok(metadata) => metadata,
                    Err(source) => {
                        return Ok(Err(DirectoryMetadataUnavailable::new(source.to_string())))
                    }
                };
                let is_broken_symlink = if is_symlink {
                    matches!(
                        fs::metadata(&path).await,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    )
                } else {
                    false
                };
                if cancellation.is_cancelled() {
                    return Err(FileError::Cancelled);
                }
                Ok(Ok(directory_filesystem_metadata(
                    &symlink_metadata,
                    is_broken_symlink,
                )))
            })
            .await?;
        Ok(())
    }

    async fn resolve_identity_names(
        &self,
        targets: &BTreeSet<usize>,
        cancellation: &CancellationToken,
        counters: Arc<DirectoryMetadataCounters>,
    ) -> Result<(), FileError> {
        for index in targets {
            if cancellation.is_cancelled() {
                return Err(FileError::Cancelled);
            }
            self.resolve_filesystem_metadata(&self.entries[*index], cancellation, &counters)
                .await?;
        }

        let mut user_ids = BTreeSet::new();
        let mut group_ids = BTreeSet::new();
        for index in targets {
            let entry = &self.entries[*index];
            if !matches!(entry.identity_names(), DirectoryMetadataState::Pending) {
                continue;
            }
            match entry.filesystem_metadata() {
                DirectoryMetadataState::Complete(metadata) => {
                    user_ids.extend(metadata.user_id);
                    group_ids.extend(metadata.group_id);
                }
                DirectoryMetadataState::Unavailable(unavailable) => {
                    let _ = entry
                        .inner
                        .identity_names
                        .set(Err(DirectoryMetadataUnavailable::new(
                            unavailable.message().to_owned(),
                        )));
                }
                DirectoryMetadataState::Pending => {
                    return Err(FileError::InvalidInput {
                        path: entry.path().to_path_buf(),
                        message: "filesystem metadata stayed pending after resolution".to_owned(),
                    })
                }
            }
        }
        user_ids.retain(|user_id| {
            !self
                .user_names
                .lock()
                .expect("user-name cache lock")
                .contains_key(user_id)
        });
        group_ids.retain(|group_id| {
            !self
                .group_names
                .lock()
                .expect("group-name cache lock")
                .contains_key(group_id)
        });
        if !user_ids.is_empty() || !group_ids.is_empty() {
            let resolver = self.clone();
            let worker_counters = Arc::clone(&counters);
            tokio::task::spawn_blocking(move || {
                worker_counters
                    .identity_worker_runs
                    .fetch_add(1, Ordering::Relaxed);
                for user_id in user_ids {
                    resolver.cached_user_name(user_id, &worker_counters);
                }
                for group_id in group_ids {
                    resolver.cached_group_name(group_id, &worker_counters);
                }
            })
            .await
            .map_err(|source| FileError::InvalidInput {
                path: self.path.clone(),
                message: format!("identity-name worker failed: {source}"),
            })?;
        }
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        for index in targets {
            let entry = &self.entries[*index];
            if !matches!(entry.identity_names(), DirectoryMetadataState::Pending) {
                continue;
            }
            let DirectoryMetadataState::Complete(metadata) = entry.filesystem_metadata() else {
                continue;
            };
            let _ = entry
                .inner
                .identity_names
                .set(Ok(self.identity_names_from_cache(metadata)));
        }
        Ok(())
    }

    fn identity_names_from_cache(
        &self,
        metadata: &DirectoryFilesystemMetadata,
    ) -> DirectoryIdentityNames {
        DirectoryIdentityNames {
            owner_name: metadata.user_id.map(|user_id| {
                self.user_names
                    .lock()
                    .expect("user-name cache lock")
                    .get(&user_id)
                    .expect("prefetched user name")
                    .clone()
            }),
            group_name: metadata.group_id.map(|group_id| {
                self.group_names
                    .lock()
                    .expect("group-name cache lock")
                    .get(&group_id)
                    .expect("prefetched group name")
                    .clone()
            }),
        }
    }

    fn cached_user_name(&self, user_id: u32, counters: &DirectoryMetadataCounters) -> String {
        let mut names = self.user_names.lock().expect("user-name cache lock");
        if let Some(name) = names.get(&user_id) {
            return name.clone();
        }
        counters.user_name_lookups.fetch_add(1, Ordering::Relaxed);
        let name = lookup_user_name(user_id);
        names.insert(user_id, name.clone());
        name
    }

    fn cached_group_name(&self, group_id: u32, counters: &DirectoryMetadataCounters) -> String {
        let mut names = self.group_names.lock().expect("group-name cache lock");
        if let Some(name) = names.get(&group_id) {
            return name.clone();
        }
        counters.group_name_lookups.fetch_add(1, Ordering::Relaxed);
        let name = lookup_group_name(group_id);
        names.insert(group_id, name.clone());
        name
    }
}

pub(crate) fn complete_entry_from_metadata(
    path: PathBuf,
    name: OsString,
    is_hidden: bool,
    metadata: &std::fs::Metadata,
    is_broken_symlink: bool,
) -> DirectoryEntry {
    let file_type = metadata.file_type();
    let is_symlink = file_type.is_symlink();
    let kind = if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else if is_symlink {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    let filesystem_metadata = directory_filesystem_metadata(metadata, is_broken_symlink);
    let identity_names = DirectoryIdentityNames {
        owner_name: filesystem_metadata.user_id.map(lookup_user_name),
        group_name: filesystem_metadata.group_id.map(lookup_group_name),
    };
    directory_entry_from_resolved_metadata(
        path,
        name,
        kind,
        is_hidden,
        is_symlink,
        &filesystem_metadata,
        &identity_names,
    )
}

fn directory_entry_from_resolved_metadata(
    path: PathBuf,
    name: OsString,
    kind: FileKind,
    is_hidden: bool,
    is_symlink: bool,
    filesystem_metadata: &DirectoryFilesystemMetadata,
    identity_names: &DirectoryIdentityNames,
) -> DirectoryEntry {
    DirectoryEntry::with_file_name(
        path,
        name,
        kind,
        EntryMetadata {
            filesystem_availability: DirectoryMetadataAvailability::Complete,
            identity_names_availability: DirectoryMetadataAvailability::Complete,
            len: filesystem_metadata.len,
            modified: filesystem_metadata.modified,
            accessed: filesystem_metadata.accessed,
            created: filesystem_metadata.created,
            readonly: filesystem_metadata.readonly,
            owner_name: identity_names.owner_name.clone(),
            group_name: identity_names.group_name.clone(),
            permissions_mode: filesystem_metadata.permissions_mode,
        },
        is_hidden,
        is_symlink,
        filesystem_metadata.is_broken_symlink,
    )
}

fn directory_filesystem_metadata(
    metadata: &std::fs::Metadata,
    is_broken_symlink: bool,
) -> DirectoryFilesystemMetadata {
    DirectoryFilesystemMetadata {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        accessed: metadata.accessed().ok(),
        created: metadata.created().ok(),
        readonly: metadata.permissions().readonly(),
        permissions_mode: permissions_mode(metadata),
        user_id: user_id(metadata),
        group_id: group_id(metadata),
        is_broken_symlink,
    }
}

#[cfg(unix)]
fn permissions_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn permissions_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn user_id(metadata: &std::fs::Metadata) -> Option<u32> {
    Some(metadata.uid())
}

#[cfg(not(unix))]
fn user_id(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn group_id(metadata: &std::fs::Metadata) -> Option<u32> {
    Some(metadata.gid())
}

#[cfg(not(unix))]
fn group_id(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn lookup_user_name(user_id: u32) -> String {
    crate::scan::lookup_unix_user_name(user_id)
}

#[cfg(not(unix))]
fn lookup_user_name(user_id: u32) -> String {
    user_id.to_string()
}

#[cfg(unix)]
fn lookup_group_name(group_id: u32) -> String {
    crate::scan::lookup_unix_group_name(group_id)
}

#[cfg(not(unix))]
fn lookup_group_name(group_id: u32) -> String {
    group_id.to_string()
}

fn metadata_state<T>(
    cell: &OnceCell<Result<T, DirectoryMetadataUnavailable>>,
) -> DirectoryMetadataState<'_, T> {
    match cell.get() {
        None => DirectoryMetadataState::Pending,
        Some(Ok(metadata)) => DirectoryMetadataState::Complete(metadata),
        Some(Err(unavailable)) => DirectoryMetadataState::Unavailable(unavailable),
    }
}
