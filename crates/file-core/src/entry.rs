use crate::archive_extraction::is_supported_archive_path;
use crate::directory_metadata::DirectoryMetadataState;
use crate::entry_record::EntryRecord;
use crate::media::MediaByName;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

    /// 类别问一次就记在共享记录里，克隆（列表快照）指向同一份；记没记不影响条目相等。
    #[test]
    fn media_by_name_is_inferred_once_and_shared_by_clones() {
        let entry = file_entry("/workspace/photo.png");
        let clone = entry.clone();

        let kind = MediaByName::Image(crate::ImageKind::Raster(crate::RasterSource::Picture));
        assert_eq!(entry.media_by_name(), kind);
        assert!(Arc::ptr_eq(&entry.record, &clone.record));
        assert_eq!(clone.media_by_name(), kind);
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

/// 条目句柄：路径、名字和文件信息都在共享的 [`EntryRecord`] 里（为什么只存一份见那里），
/// 克隆只加引用计数。补上的文件信息写进那一份，这里读到的总是最新的。
#[derive(Clone)]
pub struct DirectoryEntry {
    record: Arc<EntryRecord>,
    /// 在所属目录扫描列表里的位置；不是扫描来的条目（搜索命中等）没有。
    discovery_index: Option<u32>,
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
        let name = path.file_name().unwrap_or(path.as_os_str()).to_os_string();
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
        Self::from_record(Arc::new(EntryRecord::from_metadata(
            path,
            name,
            kind,
            metadata,
            is_hidden,
            is_symlink,
            is_broken_symlink,
        )))
    }

    pub(crate) fn from_record(record: Arc<EntryRecord>) -> Self {
        Self {
            record,
            discovery_index: None,
        }
    }

    pub fn with_discovery_index(mut self, index: usize) -> Self {
        self.discovery_index =
            Some(u32::try_from(index).expect("a directory has fewer than 2^32 entries"));
        self
    }

    pub fn discovery_index(&self) -> Option<usize> {
        self.discovery_index.map(|index| index as usize)
    }

    /// 所属目录的扫描列表不要了（改名迁移后、换目录时）：序号不再指向任何东西。
    pub fn detach_from_discovery(&mut self) {
        self.discovery_index = None;
    }

    pub fn path(&self) -> &Path {
        self.record.path()
    }

    pub fn name(&self) -> &OsStr {
        self.record.name()
    }

    pub fn kind(&self) -> FileKind {
        self.record.kind
    }

    pub fn is_hidden(&self) -> bool {
        self.record.is_hidden
    }

    pub fn is_symlink(&self) -> bool {
        self.record.is_symlink
    }

    pub fn is_broken_symlink(&self) -> bool {
        self.record.is_broken_symlink()
    }

    /// 文件大小，不复制别的信息（排序、汇总每次比较都要读）；还没补或拿不到时是 0。
    pub fn size(&self) -> u64 {
        match self.record.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => metadata.len,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => 0,
        }
    }

    /// 文件大小，还没补上时是 `None`（汇总要知道是不是全都知道了）；拿不到时算 0，和读不了的
    /// 条目一样不计入。
    pub fn size_if_resolved(&self) -> Option<u64> {
        match self.record.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => Some(metadata.len),
            DirectoryMetadataState::Unavailable(_) => Some(0),
            DirectoryMetadataState::Pending => None,
        }
    }

    /// 修改时间，用法同 [`Self::size`]；还没补或拿不到时是 `None`。
    pub fn modified(&self) -> Option<SystemTime> {
        match self.record.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => metadata.modified,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => None,
        }
    }

    /// 创建时间，用法同 [`Self::size`]。
    pub fn created(&self) -> Option<SystemTime> {
        match self.record.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => metadata.created,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => None,
        }
    }

    /// 访问时间，用法同 [`Self::size`]。
    pub fn accessed(&self) -> Option<SystemTime> {
        match self.record.filesystem_metadata() {
            DirectoryMetadataState::Complete(metadata) => metadata.accessed,
            DirectoryMetadataState::Pending | DirectoryMetadataState::Unavailable(_) => None,
        }
    }

    /// 此刻的文件信息；还在补的部分是 `Pending`。
    pub fn metadata(&self) -> EntryMetadata {
        self.record.metadata()
    }

    /// 换到新路径（改名、移动）：名字和按名字推断的类别跟着换，文件信息照旧。只换自己这个
    /// 句柄，别的列表里指向原记录的不受影响。
    pub fn relocate(&mut self, path: PathBuf) {
        let name = path.file_name().unwrap_or(path.as_os_str()).to_os_string();
        self.record = Arc::new(self.record.renamed_to(path, name));
    }

    pub fn media_by_name(&self) -> MediaByName {
        self.record.media_by_name()
    }

    /// 条目能否在视图中就地展开:真实目录,或受支持的压缩包(视作
    /// 目录)。列表展开、图标网格 disclosure、视图切换迁移共用这一判定。
    pub fn expands_as_directory(&self) -> bool {
        self.kind() == FileKind::Directory || is_supported_archive_path(self.path())
    }
}

/// 按内容比：两个句柄指向不同记录、内容一样也算相等。
impl PartialEq for DirectoryEntry {
    fn eq(&self, other: &Self) -> bool {
        self.discovery_index == other.discovery_index
            && (Arc::ptr_eq(&self.record, &other.record)
                || (self.path() == other.path()
                    && self.name() == other.name()
                    && self.kind() == other.kind()
                    && self.is_hidden() == other.is_hidden()
                    && self.is_symlink() == other.is_symlink()
                    && self.is_broken_symlink() == other.is_broken_symlink()
                    && self.metadata() == other.metadata()))
    }
}

impl Eq for DirectoryEntry {}

impl fmt::Debug for DirectoryEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectoryEntry")
            .field("path", &self.path())
            .field("name", &self.name())
            .field("kind", &self.kind())
            .field("metadata", &self.metadata())
            .field("is_hidden", &self.is_hidden())
            .field("is_symlink", &self.is_symlink())
            .field("is_broken_symlink", &self.is_broken_symlink())
            .field("discovery_index", &self.discovery_index)
            .finish()
    }
}
