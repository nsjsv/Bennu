use std::ffi::OsString;
use std::path::PathBuf;

use crate::{DirectoryEntry, ScanWarning};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashScan {
    pub entries: Vec<TrashEntry>,
    pub skipped: Vec<ScanWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashEntry {
    pub trash_path: PathBuf,
    pub info_path: PathBuf,
    pub original_path: PathBuf,
    pub deletion_date: Option<String>,
    pub entry: DirectoryEntry,
    pub(super) identity: Option<TrashEntryIdentity>,
}

impl TrashEntry {
    pub fn from_historical_entry(
        trash_path: PathBuf,
        info_path: PathBuf,
        original_path: PathBuf,
        deletion_date: Option<String>,
        entry: DirectoryEntry,
    ) -> Self {
        Self {
            trash_path,
            info_path,
            original_path,
            deletion_date,
            entry,
            identity: None,
        }
    }

    pub fn restore_entry(&self) -> TrashRestoreEntry {
        TrashRestoreEntry {
            trash_path: self.trash_path.clone(),
            info_path: self.info_path.clone(),
            original_path: self.original_path.clone(),
            identity: self.identity.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRestoreEntry {
    pub trash_path: PathBuf,
    pub info_path: PathBuf,
    pub original_path: PathBuf,
    pub(super) identity: Option<TrashEntryIdentity>,
}

impl TrashRestoreEntry {
    pub fn from_historical_paths(
        trash_path: PathBuf,
        info_path: PathBuf,
        original_path: PathBuf,
    ) -> Self {
        Self {
            trash_path,
            info_path,
            original_path,
            identity: None,
        }
    }

    pub fn has_verified_identity(&self) -> bool {
        self.identity.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashTrackingWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashCommitOutcome {
    Tracked(Box<TrashRestoreEntry>),
    CommittedWithoutRestoreEntry(TrashTrackingWarning),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrashLocationKind {
    Home,
    SharedVolume,
    PrivateVolume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OriginalPathBase {
    Absolute,
    RelativeToTopDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrashObjectKind {
    RegularFile,
    Directory,
    SymbolicLink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrashObjectIdentity {
    pub device: u64,
    pub inode: u64,
    pub kind: TrashObjectKind,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
}

impl TrashObjectIdentity {
    pub fn same_object(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode && self.kind == other.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifiedTrashDirectory {
    pub path: PathBuf,
    pub identity: TrashObjectIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrashLocationGuard {
    pub kind: TrashLocationKind,
    pub top_directory: PathBuf,
    pub top_identity: Option<TrashObjectIdentity>,
    pub shared_root: Option<VerifiedTrashDirectory>,
    pub trash_root: VerifiedTrashDirectory,
    pub files: VerifiedTrashDirectory,
    pub info: VerifiedTrashDirectory,
    pub original_path_base: OriginalPathBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrashEntryIdentity {
    pub location: TrashLocationGuard,
    pub item_name: OsString,
    pub info: TrashObjectIdentity,
    pub payload: TrashObjectIdentity,
}
