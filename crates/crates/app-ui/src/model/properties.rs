use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use file_core::FileKind;
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
    PermissionToggled(
        FilePropertiesPermissionClass,
        FilePropertiesPermissionAccess,
    ),
    ApplyPermissionsToEnclosedItems,
    CategorySelected(FilePropertiesCategory),
    PermissionsUpdated(
        FilePropertiesRequest,
        Result<FilePropertiesPermissionWriteOutcome, String>,
    ),
    EnclosedPermissionsUpdated(
        FilePropertiesRequest,
        Result<FilePropertiesPermissions, String>,
    ),
    Requested(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct FilePropertiesState {
    pub(crate) targets: FilePropertiesTargetSet,
    pub(crate) load_state: FilePropertiesLoadState,
    pub(crate) selected_category: FilePropertiesCategory,
    pub(crate) permission_update: FilePropertiesPermissionUpdate,
    pub(crate) load_generation: u64,
    pub(crate) load_cancel: Option<CancellationToken>,
}

impl FilePropertiesState {
    pub(crate) fn loading(request: FilePropertiesRequest, cancellation: CancellationToken) -> Self {
        Self {
            targets: request.targets,
            load_state: FilePropertiesLoadState::Loading,
            selected_category: FilePropertiesCategory::Information,
            permission_update: FilePropertiesPermissionUpdate::Idle,
            load_generation: request.generation,
            load_cancel: Some(cancellation),
        }
    }

    pub(crate) fn cancel_load(&mut self) {
        if let Some(cancel) = self.load_cancel.take() {
            cancel.cancel();
        }
        self.load_generation = self.load_generation.wrapping_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePropertiesCategory {
    Information,
    Permissions,
}

impl FilePropertiesCategory {
    pub(crate) const ALL: [Self; 2] = [Self::Information, Self::Permissions];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Information => "File Information",
            Self::Permissions => "Permissions",
        }
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
    pub(crate) kind: FileKind,
    pub(crate) type_label: String,
    pub(crate) location: PathBuf,
    pub(crate) created: Option<SystemTime>,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) accessed: Option<SystemTime>,
    pub(crate) size_bytes: u64,
    pub(crate) disk_size_bytes: u64,
    pub(crate) directory_contents: FilePropertiesDirectoryContentsState,
    pub(crate) permissions: Option<FilePropertiesPermissions>,
}

#[derive(Debug, Clone)]
pub(crate) enum FilePropertiesDirectoryContentsState {
    NotDirectory,
    Loading(Option<FilePropertiesDirectoryContents>),
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

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Execute => "Execute",
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
}
