//! 一个条目的全部数据只存这一份。扫描得到的列表（[`DiscoveredDirectoryEntry`]）和界面用的列表
//! （[`DirectoryEntry`]）都只放指向它的 `Arc`：原来两边各存一遍路径、名字和文件信息，10 万个文件
//! 的目录平均每条约 640 字节。按需补的文件信息写进这里的一次性格子，持有句柄的地方都能直接读到，
//! 不用再按序号回扫描列表里找最新值。
//!
//! [`DiscoveredDirectoryEntry`]: crate::DiscoveredDirectoryEntry
//! [`DirectoryEntry`]: crate::DirectoryEntry

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::sync::OnceCell;

use crate::directory_metadata::{
    metadata_state, DirectoryFilesystemMetadata, DirectoryIdentityNames, DirectoryMetadataState,
    DirectoryMetadataUnavailable,
};
use crate::media::{media_by_name, MediaByName};
use crate::{DirectoryMetadataAvailability, EntryMetadata, FileKind};

/// 文件信息格子：只写一次，`Pending → Complete` 或 `Pending → Unavailable`。补信息是异步的，
/// 同一条目被几个请求同时要时只查一次文件系统，所以用 tokio 的。
pub(crate) type FilesystemCell =
    OnceCell<Result<DirectoryFilesystemMetadata, DirectoryMetadataUnavailable>>;
pub(crate) type IdentityCell =
    OnceCell<Result<DirectoryIdentityNames, DirectoryMetadataUnavailable>>;

/// 现成条目（搜索命中、占位条目等）说拿不到信息时没有原因可写。
const UNAVAILABLE_WITHOUT_REASON: &str = "metadata unavailable";

#[derive(Debug)]
pub(crate) struct EntryRecord {
    path: PathBuf,
    /// 显示名和路径最后一段不同时才存（回收站里的条目等）；绝大多数条目省一次分配。
    name: Option<Box<OsStr>>,
    pub(crate) kind: FileKind,
    pub(crate) is_hidden: bool,
    pub(crate) is_symlink: bool,
    pub(crate) filesystem_metadata: FilesystemCell,
    pub(crate) identity_names: IdentityCell,
    /// 界面每帧都要问可见条目的类别（能不能出缩略图），按名字查类型库要几微秒：第一次问起时
    /// 算、记在这里。不在列目录时算：大目录里大部分条目从不显示。
    media_by_name: OnceLock<MediaByName>,
}

impl EntryRecord {
    pub(crate) fn new(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        is_hidden: bool,
        is_symlink: bool,
        filesystem_metadata: FilesystemCell,
        identity_names: IdentityCell,
    ) -> Self {
        let name = (name.as_os_str() != last_component(&path)).then(|| name.into_boxed_os_str());
        Self {
            path,
            name,
            kind,
            is_hidden,
            is_symlink,
            filesystem_metadata,
            identity_names,
            media_by_name: OnceLock::new(),
        }
    }

    /// 由一份现成的文件信息建条目（搜索命中、占位条目等，不经扫描）：按各部分的状态填格子，
    /// 还没有的留空。链接是否失效只在文件信息齐了时才有意义。
    pub(crate) fn from_metadata(
        path: PathBuf,
        name: OsString,
        kind: FileKind,
        metadata: EntryMetadata,
        is_hidden: bool,
        is_symlink: bool,
        is_broken_symlink: bool,
    ) -> Self {
        let filesystem_metadata = match metadata.filesystem_availability {
            DirectoryMetadataAvailability::Pending => OnceCell::new(),
            DirectoryMetadataAvailability::Complete => {
                OnceCell::new_with(Some(Ok(DirectoryFilesystemMetadata {
                    len: metadata.len,
                    modified: metadata.modified,
                    accessed: metadata.accessed,
                    created: metadata.created,
                    changed: metadata.changed,
                    readonly: metadata.readonly,
                    permissions_mode: metadata.permissions_mode,
                    user_id: None,
                    group_id: None,
                    is_broken_symlink,
                })))
            }
            DirectoryMetadataAvailability::Unavailable => OnceCell::new_with(Some(Err(
                DirectoryMetadataUnavailable::new(UNAVAILABLE_WITHOUT_REASON),
            ))),
        };
        let identity_names = match metadata.identity_names_availability {
            DirectoryMetadataAvailability::Pending => OnceCell::new(),
            DirectoryMetadataAvailability::Complete => {
                OnceCell::new_with(Some(Ok(DirectoryIdentityNames {
                    owner_name: metadata.owner_name,
                    group_name: metadata.group_name,
                })))
            }
            DirectoryMetadataAvailability::Unavailable => OnceCell::new_with(Some(Err(
                DirectoryMetadataUnavailable::new(UNAVAILABLE_WITHOUT_REASON),
            ))),
        };
        Self::new(
            path,
            name,
            kind,
            is_hidden,
            is_symlink,
            filesystem_metadata,
            identity_names,
        )
    }

    /// 换到新路径（改名、移动）：文件信息不变，已补的照用；类别按新名字重新推断。
    pub(crate) fn renamed_to(&self, path: PathBuf, name: OsString) -> Self {
        Self::new(
            path,
            name,
            self.kind,
            self.is_hidden,
            self.is_symlink,
            self.filesystem_metadata.clone(),
            self.identity_names.clone(),
        )
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn name(&self) -> &OsStr {
        self.name
            .as_deref()
            .unwrap_or_else(|| last_component(&self.path))
    }

    pub(crate) fn filesystem_metadata(
        &self,
    ) -> DirectoryMetadataState<'_, DirectoryFilesystemMetadata> {
        metadata_state(&self.filesystem_metadata)
    }

    pub(crate) fn identity_names(&self) -> DirectoryMetadataState<'_, DirectoryIdentityNames> {
        metadata_state(&self.identity_names)
    }

    pub(crate) fn media_by_name(&self) -> MediaByName {
        *self.media_by_name.get_or_init(|| media_by_name(&self.path))
    }

    pub(crate) fn is_broken_symlink(&self) -> bool {
        matches!(
            self.filesystem_metadata(),
            DirectoryMetadataState::Complete(metadata) if metadata.is_broken_symlink
        )
    }

    /// 此刻的文件信息：还没补的部分标成 `Pending`，拿不到的标成 `Unavailable`。
    pub(crate) fn metadata(&self) -> EntryMetadata {
        let mut metadata = EntryMetadata::pending();
        match self.filesystem_metadata() {
            DirectoryMetadataState::Pending => {}
            DirectoryMetadataState::Complete(filesystem) => {
                metadata.filesystem_availability = DirectoryMetadataAvailability::Complete;
                metadata.len = filesystem.len;
                metadata.modified = filesystem.modified;
                metadata.accessed = filesystem.accessed;
                metadata.created = filesystem.created;
                metadata.changed = filesystem.changed;
                metadata.readonly = filesystem.readonly;
                metadata.permissions_mode = filesystem.permissions_mode;
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
        metadata
    }
}

/// 没有最后一段的路径（`/`）拿整个路径当名字。
fn last_component(path: &Path) -> &OsStr {
    path.file_name().unwrap_or(path.as_os_str())
}
