use crate::archive_extraction::is_supported_archive_path;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectoryMetadataAvailability {
    Pending,
    #[default]
    Complete,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntryMetadata {
    pub filesystem_availability: DirectoryMetadataAvailability,
    pub identity_names_availability: DirectoryMetadataAvailability,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub readonly: bool,
    pub owner_name: Option<String>,
    pub group_name: Option<String>,
    pub permissions_mode: Option<u32>,
}

impl EntryMetadata {
    pub(crate) fn pending() -> Self {
        Self {
            filesystem_availability: DirectoryMetadataAvailability::Pending,
            identity_names_availability: DirectoryMetadataAvailability::Pending,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_entry_metadata_leaves_optional_fields_empty() {
        let metadata = EntryMetadata::default();

        assert_eq!(metadata.len, 0);
        assert_eq!(
            metadata.filesystem_availability,
            DirectoryMetadataAvailability::Complete
        );
        assert_eq!(
            metadata.identity_names_availability,
            DirectoryMetadataAvailability::Complete
        );
        assert_eq!(metadata.modified, None);
        assert_eq!(metadata.accessed, None);
        assert_eq!(metadata.created, None);
        assert_eq!(metadata.owner_name, None);
        assert_eq!(metadata.group_name, None);
        assert_eq!(metadata.permissions_mode, None);
        assert!(!metadata.readonly);
    }

    #[test]
    fn expands_as_directory_covers_directories_and_supported_archives() {
        let entry = |path: &str, kind: FileKind| {
            DirectoryEntry::new(
                PathBuf::from(path),
                kind,
                EntryMetadata::default(),
                false,
                false,
                false,
            )
        };

        assert!(entry("/workspace/notes", FileKind::Directory).expands_as_directory());
        assert!(entry("/workspace/bundle.zip", FileKind::File).expands_as_directory());
        assert!(entry("/workspace/photos.tar.gz", FileKind::File).expands_as_directory());
        assert!(!entry("/workspace/notes.txt", FileKind::File).expands_as_directory());
        // 与 entry_acts_as_directory 同口径:按路径扩展名判定,不区分 symlink。
        assert!(entry("/workspace/bundle.zip", FileKind::Symlink).expands_as_directory());
        assert!(!entry("/workspace/link", FileKind::Symlink).expands_as_directory());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub path: PathBuf,
    pub name: OsString,
    pub kind: FileKind,
    pub metadata: EntryMetadata,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_broken_symlink: bool,
    pub discovery_index: Option<usize>,
}

impl DirectoryEntry {
    pub fn new(
        path: PathBuf,
        kind: FileKind,
        metadata: EntryMetadata,
        is_hidden: bool,
        is_symlink: bool,
        is_broken_symlink: bool,
    ) -> Self {
        let name = path
            .file_name()
            .map(OsStr::to_os_string)
            .unwrap_or_else(|| path.as_os_str().to_os_string());

        Self::with_file_name(
            path,
            name,
            kind,
            metadata,
            is_hidden,
            is_symlink,
            is_broken_symlink,
        )
    }

    pub(crate) fn with_file_name(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        metadata: EntryMetadata,
        is_hidden: bool,
        is_symlink: bool,
        is_broken_symlink: bool,
    ) -> Self {
        Self {
            path,
            name,
            kind,
            metadata,
            is_hidden,
            is_symlink,
            is_broken_symlink,
            discovery_index: None,
        }
    }

    pub fn with_discovery_index(mut self, index: usize) -> Self {
        self.discovery_index = Some(index);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// 条目能否在视图中就地展开:真实目录,或受支持的压缩包(视作
    /// 目录)。列表展开、图标网格 disclosure、视图切换迁移共用这一判定。
    pub fn expands_as_directory(&self) -> bool {
        self.kind == FileKind::Directory || is_supported_archive_path(&self.path)
    }
}
