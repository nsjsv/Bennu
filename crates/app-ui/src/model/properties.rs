use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bennu_preview::info_overlay::InfoRow;
use file_core::{FileKind, VolumeUsage};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FilePropertiesTargetSet {
    paths: Vec<PathBuf>,
}

impl FilePropertiesTargetSet {
    pub(crate) fn new(paths: Vec<PathBuf>) -> Result<Self, &'static str> {
        let mut seen = HashSet::new();
        let paths = paths
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Err("file properties require at least one target");
        }
        Ok(Self { paths })
    }

    pub(crate) fn single(path: PathBuf) -> Self {
        Self { paths: vec![path] }
    }

    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub(crate) fn single_path(&self) -> Option<&Path> {
        (self.paths.len() == 1).then(|| self.paths[0].as_path())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePropertiesRequest {
    pub(crate) targets: FilePropertiesTargetSet,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesMessage {
    Loaded(
        FilePropertiesRequest,
        Result<FilePropertiesPresentation, String>,
    ),
    AggregateUpdated(FilePropertiesRequest, FilePropertiesAggregateSnapshot),
    DirectoryContentsUpdated(FilePropertiesRequest, FilePropertiesDirectoryContents),
    DirectoryContentsLoaded(
        FilePropertiesRequest,
        Result<FilePropertiesDirectoryContents, String>,
    ),
    DefaultApplicationLoaded(FilePropertiesRequest, Result<Option<String>, String>),
    MediaDetailsLoaded(FilePropertiesRequest, Result<Vec<InfoRow>, String>),
    PermissionToggled(
        FilePropertiesPermissionClass,
        FilePropertiesPermissionAccess,
    ),
    ApplyPermissionsToEnclosedItems,
    PermissionsUpdated(
        FilePropertiesRequest,
        Result<FilePropertiesPermissionWriteOutcome, String>,
    ),
    EnclosedPermissionsUpdated(
        FilePropertiesRequest,
        Result<FilePropertiesPermissions, String>,
    ),
    Requested(PathBuf),
    /// 顶栏的刷新按钮或刷新键：全部重读。
    RefreshRequested,
    /// 看着的目录（目标所在的、单个文件夹自己）有变化。
    DiskChanged(file_core::DirectoryEntryChanges),
    /// 磁盘上的变化攒够一会儿了，按攒下的重读。
    ChangesSettled,
}

/// 重读多少。目标本身（大小、时间、权限、所有者、类型）每次都重新 stat；慢项（打开方式、
/// 详细信息、文件夹大小）只在它们依据的东西变了时再读，没再读的沿用上次的。按变化从小到大
/// 排，攒下几次变化取最大的那个。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FilePropertiesReread {
    /// 目标本身变了：类型变了才再问打开方式，大小或修改时间变了才再读详细信息，文件夹不重量。
    Item,
    /// 文件夹里的东西变了：另外重量大小。
    Contents,
    /// 第一次打开、用户按了刷新：全部重读。
    All,
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesState {
    pub(crate) targets: FilePropertiesTargetSet,
    pub(crate) load_state: FilePropertiesLoadState,
    pub(crate) permission_update: FilePropertiesPermissionUpdate,
    pub(crate) load_generation: u64,
    /// 这次加载的所有后台读取（内容统计、打开方式、详细信息）共用；换目标、关窗时取消。
    pub(crate) load_cancel: CancellationToken,
    /// 磁盘上有了变化、还没重读：在等它们停一会儿（`ChangesSettled`）。
    pub(crate) unsettled_changes: Option<FilePropertiesReread>,
}

impl FilePropertiesState {
    pub(crate) fn loading(request: FilePropertiesRequest, cancellation: CancellationToken) -> Self {
        Self {
            targets: request.targets,
            load_state: FilePropertiesLoadState::Loading,
            permission_update: FilePropertiesPermissionUpdate::Idle,
            load_generation: request.generation,
            load_cancel: cancellation,
            unsettled_changes: None,
        }
    }

    pub(crate) fn cancel_load(&mut self) {
        self.load_cancel.cancel();
        self.load_generation = self.load_generation.wrapping_add(1);
    }

    /// 要看着的目录：目标所在的文件夹（改名、删除、大小时间权限的变化都在那里报），单个文件夹
    /// 再加它自己（里面的东西变了，大小要重新量）。
    pub(crate) fn watched_directories(&self) -> Vec<PathBuf> {
        let mut directories = self
            .targets
            .paths()
            .iter()
            .filter_map(|path| path.parent())
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        if let Some(directory) = self.single_directory() {
            directories.push(directory.to_path_buf());
        }
        directories.sort();
        directories.dedup();
        directories
    }

    /// 只看着一个文件夹、并且已经读出它是文件夹时，这个文件夹。
    pub(crate) fn single_directory(&self) -> Option<&Path> {
        self.single_snapshot()
            .filter(|snapshot| snapshot.details.kind() == FileKind::Directory)
            .and(self.targets.single_path())
    }

    /// 读好了的单个目标。
    pub(crate) fn single_snapshot(&self) -> Option<&FilePropertiesSnapshot> {
        match &self.load_state {
            FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) => {
                Some(snapshot)
            }
            _ => None,
        }
    }

    /// 同一批目标重读：作废还在读的、换上新代号；正在显示的留着，新结果到了再整个换。
    pub(crate) fn begin_reread(
        &mut self,
        generation: u64,
    ) -> (FilePropertiesRequest, CancellationToken) {
        self.load_cancel.cancel();
        self.load_cancel = CancellationToken::new();
        self.load_generation = generation;
        let request = FilePropertiesRequest {
            targets: self.targets.clone(),
            generation,
        };
        (request, self.load_cancel.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesLoadState {
    Loading,
    LoadingAggregate(FilePropertiesAggregateSnapshot),
    Loaded(FilePropertiesPresentation),
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesPresentation {
    Single(FilePropertiesSnapshot),
    Aggregate(FilePropertiesAggregateSnapshot),
}

impl FilePropertiesPresentation {
    pub(crate) fn permissions(&self) -> Option<FilePropertiesPermissions> {
        match self {
            Self::Single(snapshot) => snapshot.permissions,
            Self::Aggregate(snapshot) => snapshot.permissions,
        }
    }

    pub(crate) fn set_permissions(&mut self, permissions: FilePropertiesPermissions) {
        match self {
            Self::Single(snapshot) => snapshot.permissions = Some(permissions),
            Self::Aggregate(snapshot) => snapshot.permissions = Some(permissions),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesSnapshot {
    pub(crate) name: OsString,
    pub(crate) location: PathBuf,
    pub(crate) created: Option<SystemTime>,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) accessed: Option<SystemTime>,
    /// 状态改变时间（ctime）。
    pub(crate) changed: Option<SystemTime>,
    pub(crate) size_bytes: u64,
    pub(crate) disk_size_bytes: u64,
    pub(crate) details: FilePropertiesKindDetails,
    pub(crate) owner: FilePropertiesOwner,
    pub(crate) volume: Result<VolumeUsage, String>,
    pub(crate) permissions: Option<FilePropertiesPermissions>,
}

/// 只有某种条目才有的信息跟着种类走：文件夹才有内容统计，链接才有指向。
#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesKindDetails {
    File(FilePropertiesFileDetails),
    Directory(FilePropertiesDirectoryContentsState),
    Symlink(Result<PathBuf, String>),
    Other,
}

impl FilePropertiesKindDetails {
    pub(crate) fn kind(&self) -> FileKind {
        match self {
            Self::File(_) => FileKind::File,
            Self::Directory(_) => FileKind::Directory,
            Self::Symlink(_) => FileKind::Symlink,
            Self::Other => FileKind::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesFileDetails {
    pub(crate) mime_type: String,
    /// 类型库里的类型名（「PNG 图像」）；库里没有这个类型是 None。
    pub(crate) description: Option<String>,
    /// `Ready(None)`：这种类型没设默认程序。
    pub(crate) default_application: FilePropertiesLookup<Option<String>>,
    /// 不是图片、音视频是 None。
    pub(crate) media_details: Option<FilePropertiesLookup<Vec<InfoRow>>>,
}

/// 后到的一项：先占位，到了填上，读不了留下原因。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FilePropertiesLookup<T> {
    Pending,
    Ready(T),
    Failed(String),
}

impl<T> From<Result<T, String>> for FilePropertiesLookup<T> {
    fn from(outcome: Result<T, String>) -> Self {
        match outcome {
            Ok(value) => Self::Ready(value),
            Err(error) => Self::Failed(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePropertiesOwner {
    pub(crate) user: String,
    pub(crate) group: String,
}

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesDirectoryContentsState {
    Loading(Option<FilePropertiesDirectoryContents>),
    /// 里面的东西变了，正在重新量：一直显示上次量好的，量完整个换掉，不显示量到一半的数。
    Refreshing(FilePropertiesDirectoryContents),
    Loaded(FilePropertiesDirectoryContents),
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesAggregateSnapshot {
    pub(crate) target_count: usize,
    pub(crate) file_count: usize,
    pub(crate) directory_count: usize,
    pub(crate) symlink_count: usize,
    pub(crate) other_count: usize,
    pub(crate) total_size_bytes: u64,
    pub(crate) total_disk_size_bytes: u64,
    pub(crate) recursive_contents: FilePropertiesDirectoryContents,
    pub(crate) common_parent: Option<PathBuf>,
    pub(crate) common_kind: Option<FileKind>,
    pub(crate) common_created: Option<SystemTime>,
    pub(crate) common_modified: Option<SystemTime>,
    pub(crate) common_accessed: Option<SystemTime>,
    pub(crate) common_changed: Option<SystemTime>,
    pub(crate) permissions: Option<FilePropertiesPermissions>,
    pub(crate) permission_baselines: Vec<FilePropertiesPermissionBaseline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePropertiesPermissionBaseline {
    pub(crate) path: PathBuf,
    pub(crate) identity: FilePropertiesIdentity,
    pub(crate) permissions: FilePropertiesPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePropertiesIdentity {
    #[cfg(unix)]
    pub(crate) device: u64,
    #[cfg(unix)]
    pub(crate) inode: u64,
    pub(crate) kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionBatchPathFailure {
    pub(crate) path: PathBuf,
    pub(crate) error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionBatchOutcome {
    pub(crate) succeeded_paths: Vec<PathBuf>,
    pub(crate) failures: Vec<PermissionBatchPathFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilePropertiesPermissions {
    mode: u32,
}

impl FilePropertiesPermissions {
    const DISPLAY_MODE_MASK: u32 = 0o7777;

    pub(crate) fn from_mode(mode: u32) -> Self {
        Self {
            mode: mode & Self::DISPLAY_MODE_MASK,
        }
    }

    pub(crate) fn mode(self) -> u32 {
        self.mode
    }

    pub(crate) fn contains(
        self,
        class: FilePropertiesPermissionClass,
        access: FilePropertiesPermissionAccess,
    ) -> bool {
        self.mode & permission_mask(class, access) != 0
    }

    pub(crate) fn toggled(
        self,
        class: FilePropertiesPermissionClass,
        access: FilePropertiesPermissionAccess,
    ) -> Self {
        let mask = permission_mask(class, access);
        let mode = if self.mode & mask == 0 {
            self.mode | mask
        } else {
            self.mode & !mask
        };
        Self::from_mode(mode)
    }

    pub(crate) fn octal_string(self) -> String {
        format!("{:04o}", self.mode)
    }

    pub(crate) fn symbolic_string(self) -> String {
        [
            (FilePropertiesPermissionClass::Owner, 'r', 'w', 'x'),
            (FilePropertiesPermissionClass::Group, 'r', 'w', 'x'),
            (FilePropertiesPermissionClass::Others, 'r', 'w', 'x'),
        ]
        .into_iter()
        .flat_map(|(class, read, write, execute)| {
            [
                permission_char(self, class, FilePropertiesPermissionAccess::Read, read),
                permission_char(self, class, FilePropertiesPermissionAccess::Write, write),
                permission_char(
                    self,
                    class,
                    FilePropertiesPermissionAccess::Execute,
                    execute,
                ),
            ]
        })
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePropertiesPermissionClass {
    Owner,
    Group,
    Others,
}

impl FilePropertiesPermissionClass {
    pub(crate) const ALL: [Self; 3] = [Self::Owner, Self::Group, Self::Others];

    fn shift(self) -> u32 {
        match self {
            Self::Owner => 6,
            Self::Group => 3,
            Self::Others => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePropertiesPermissionAccess {
    Read,
    Write,
    Execute,
}

impl FilePropertiesPermissionAccess {
    pub(crate) const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Execute];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Read => tr!("Read"),
            Self::Write => tr!("Write"),
            Self::Execute => tr!("Execute"),
        }
    }

    fn bit(self) -> u32 {
        match self {
            Self::Read => 0o4,
            Self::Write => 0o2,
            Self::Execute => 0o1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilePropertiesPermissionWriteOutcome {
    Single(FilePropertiesPermissions),
    Batch(PermissionBatchOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilePropertiesPermissionUpdate {
    Idle,
    SavingCurrentItem {
        permissions: FilePropertiesPermissions,
    },
    SavingTargetSet {
        permissions: FilePropertiesPermissions,
    },
    ApplyingToEnclosedItems {
        permissions: FilePropertiesPermissions,
    },
    TargetSetCompleted {
        succeeded_count: usize,
        failures: Vec<PermissionBatchPathFailure>,
    },
    Failed(String),
}

impl FilePropertiesPermissionUpdate {
    pub(crate) fn is_in_progress(&self) -> bool {
        matches!(
            self,
            Self::SavingCurrentItem { .. }
                | Self::SavingTargetSet { .. }
                | Self::ApplyingToEnclosedItems { .. }
        )
    }

    pub(crate) fn pending_permissions(&self) -> Option<FilePropertiesPermissions> {
        match self {
            Self::SavingCurrentItem { permissions }
            | Self::SavingTargetSet { permissions }
            | Self::ApplyingToEnclosedItems { permissions } => Some(*permissions),
            Self::Idle | Self::TargetSetCompleted { .. } | Self::Failed(_) => None,
        }
    }
}

fn permission_mask(
    class: FilePropertiesPermissionClass,
    access: FilePropertiesPermissionAccess,
) -> u32 {
    access.bit() << class.shift()
}

fn permission_char(
    permissions: FilePropertiesPermissions,
    class: FilePropertiesPermissionClass,
    access: FilePropertiesPermissionAccess,
    enabled_char: char,
) -> char {
    if permissions.contains(class, access) {
        enabled_char
    } else {
        '-'
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesDirectoryContents {
    pub(crate) file_count: usize,
    pub(crate) directory_count: usize,
    pub(crate) total_size_bytes: u64,
    pub(crate) total_disk_size_bytes: u64,
    /// 读不了、没算进来的子目录或条目数；非零时界面标注"部分内容无法读取"。
    pub(crate) unreadable_entries: u64,
}
