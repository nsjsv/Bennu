use crate::archive_extraction::is_supported_archive_path;
use crate::media::{media_by_name, MediaByName};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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

    fn file_entry(path: &str) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from(path),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )
    }

    /// 类别问一次就记在条目上，克隆（列表快照）带着它；记没记不影响条目相等。
    #[test]
    fn media_by_name_is_inferred_once_and_travels_with_clones() {
        let entry = file_entry("/workspace/photo.png");
        assert_eq!(entry.media_by_name.0.get(), None);

        let kind = MediaByName::Image(crate::ImageKind::Raster(crate::RasterSource::Picture));
        assert_eq!(entry.media_by_name(), kind);
        assert_eq!(entry.media_by_name.0.get(), Some(&kind));
        assert_eq!(entry.clone().media_by_name.0.get(), Some(&kind));
        assert_eq!(entry, file_entry("/workspace/photo.png"));
    }

    /// 改名、移动后名字和类别都按新路径来，不沿用旧名字的结论。
    #[test]
    fn relocate_renames_and_infers_again() {
        let mut entry = file_entry("/workspace/notes.txt");
        assert_eq!(entry.media_by_name(), MediaByName::Other);

        entry.relocate(PathBuf::from("/elsewhere/clip.mp4"));

        assert_eq!(entry.path(), Path::new("/elsewhere/clip.mp4"));
        assert_eq!(entry.name(), OsStr::new("clip.mp4"));
        assert_eq!(entry.media_by_name(), MediaByName::Video);
        assert_eq!(entry, file_entry("/elsewhere/clip.mp4"));
    }
}

/// 按名字推断的类别记在条目上、跟着 `path` 走，所以 `path`、`name` 只能经构造和
/// [`DirectoryEntry::relocate`] 改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    path: PathBuf,
    name: OsString,
    pub kind: FileKind,
    pub metadata: EntryMetadata,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_broken_symlink: bool,
    pub discovery_index: Option<usize>,
    media_by_name: MediaByNameMemo,
}

/// 界面每帧都要问每个可见条目的类别（能不能出缩略图），按名字查一次类型库要几微秒，
/// 所以第一次问起时算、记在条目上。不在列目录时就算：大目录里大部分条目从不显示。
/// 条目快照在线程间共享，所以用 `OnceLock`。
#[derive(Debug, Clone, Default)]
struct MediaByNameMemo(OnceLock<MediaByName>);

/// 类别完全由路径决定，比较条目时路径已经比过了；算没算过不算区别。
impl PartialEq for MediaByNameMemo {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for MediaByNameMemo {}

impl DirectoryEntry {
    pub fn new(
        path: PathBuf,
        kind: FileKind,
        metadata: EntryMetadata,
        is_hidden: bool,
        is_symlink: bool,
        is_broken_symlink: bool,
    ) -> Self {
        let name = entry_name(&path);
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
            media_by_name: MediaByNameMemo::default(),
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

    /// 换到新路径（改名、移动）：名字和按名字推断的类别跟着换。
    pub fn relocate(&mut self, path: PathBuf) {
        self.name = entry_name(&path);
        self.path = path;
        self.media_by_name = MediaByNameMemo::default();
    }

    pub fn media_by_name(&self) -> MediaByName {
        *self
            .media_by_name
            .0
            .get_or_init(|| media_by_name(&self.path))
    }

    /// 条目能否在视图中就地展开:真实目录,或受支持的压缩包(视作
    /// 目录)。列表展开、图标网格 disclosure、视图切换迁移共用这一判定。
    pub fn expands_as_directory(&self) -> bool {
        self.kind == FileKind::Directory || is_supported_archive_path(&self.path)
    }
}

/// 没有最后一段的路径（`/`）拿整个路径当名字。
fn entry_name(path: &Path) -> OsString {
    path.file_name().unwrap_or(path.as_os_str()).to_os_string()
}
