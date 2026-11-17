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

use crate::entry_record::EntryRecord;
use crate::{DirectoryEntry, FileError, FileKind, ScanWarning};

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

/// 扫描得到的条目：数据在共享的 [`EntryRecord`] 里，转成界面用的 [`DirectoryEntry`] 只是多一个
/// 指向同一份的句柄。
#[derive(Debug, Clone)]
pub struct DiscoveredDirectoryEntry {
    inner: Arc<EntryRecord>,
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
            inner: Arc::new(EntryRecord::new(
                path,
                name,
                kind,
                is_hidden,
                is_symlink,
                OnceCell::new(),
                OnceCell::new(),
            )),
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
            inner: Arc::new(EntryRecord::new(
                path,
                name,
                FileKind::Other,
                is_hidden,
                false,
                filesystem_metadata,
                OnceCell::new(),
            )),
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
            inner: Arc::new(EntryRecord::new(
                path,
                name,
                kind,
                is_hidden,
                is_symlink,
                filesystem_metadata,
                OnceCell::new(),
            )),
        }
    }

    /// 归档虚拟条目专用构造：元数据来自归档头而非文件系统，边界处
    /// 预填 Complete，让后续 demand（stat / NSS）自然短路。包内条目
    /// 恒为只读，owner/group 无文件系统语义。
    pub(crate) fn with_archive_member_metadata(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        is_hidden: bool,
        len: u64,
        modified: Option<SystemTime>,
    ) -> Self {
        let filesystem_metadata = OnceCell::new();
        filesystem_metadata
            .set(Ok(DirectoryFilesystemMetadata {
                len,
                modified,
                accessed: None,
                created: None,
                readonly: true,
                permissions_mode: None,
                user_id: None,
                group_id: None,
                is_broken_symlink: false,
            }))
            .expect("new filesystem metadata cell must be empty");
        let identity_names = OnceCell::new();
        identity_names
            .set(Ok(DirectoryIdentityNames {
                owner_name: None,
                group_name: None,
            }))
            .expect("new identity names cell must be empty");
        Self {
            inner: Arc::new(EntryRecord::new(
                path,
                name,
                kind,
                is_hidden,
                false,
                filesystem_metadata,
                identity_names,
            )),
        }
    }

    /// rename 不改变文件元数据：已填写的 metadata/identity cells 照用，
    /// 仅替换 path/name。未填写的 cell 克隆后仍是待解析状态。
    pub fn renamed_to(&self, path: PathBuf, name: OsString) -> Self {
        Self {
            inner: Arc::new(self.inner.renamed_to(path, name)),
        }
    }

    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    pub fn name(&self) -> &OsStr {
        self.inner.name()
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
        self.inner.filesystem_metadata()
    }

    pub fn identity_names(&self) -> DirectoryMetadataState<'_, DirectoryIdentityNames> {
        self.inner.identity_names()
    }

    /// 界面用的条目：指向同一份记录，之后补上的文件信息两边都看得到。
    pub fn display_entry(&self) -> DirectoryEntry {
        DirectoryEntry::from_record(Arc::clone(&self.inner))
    }

    /// 文件信息和属主都齐了才算完整（旧的一次扫完接口只要完整的条目）。
    pub(crate) fn complete_entry(&self) -> Option<DirectoryEntry> {
        let complete = matches!(
            self.filesystem_metadata(),
            DirectoryMetadataState::Complete(_)
        ) && matches!(self.identity_names(), DirectoryMetadataState::Complete(_));
        complete.then(|| self.display_entry())
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
    DirectoryEntry::from_record(Arc::new(EntryRecord::new(
        path,
        name,
        kind,
        is_hidden,
        is_symlink,
        OnceCell::new_with(Some(Ok(filesystem_metadata))),
        OnceCell::new_with(Some(Ok(identity_names))),
    )))
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

pub(crate) fn metadata_state<T>(
    cell: &OnceCell<Result<T, DirectoryMetadataUnavailable>>,
) -> DirectoryMetadataState<'_, T> {
    match cell.get() {
        None => DirectoryMetadataState::Pending,
        Some(Ok(metadata)) => DirectoryMetadataState::Complete(metadata),
        Some(Err(unavailable)) => DirectoryMetadataState::Unavailable(unavailable),
    }
}
