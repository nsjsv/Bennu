use file_search::mime::NameType;
use std::path::PathBuf;

use crate::error::{SearchError, SearchResult};
use crate::model::SearchFileKind;

use super::IndexedEntryStageState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryObservationState {
    Observable,
    Inaccessible,
}

impl EntryObservationState {
    pub(super) fn as_storage_value(self) -> &'static str {
        match self {
            Self::Observable => "observable",
            Self::Inaccessible => "inaccessible",
        }
    }

    pub(super) fn from_storage_value(value: &str) -> SearchResult<Self> {
        match value {
            "observable" => Ok(Self::Observable),
            "inaccessible" => Ok(Self::Inaccessible),
            _ => Err(SearchError::InvalidQuery(format!(
                "unsupported entry observation state: {value}"
            ))),
        }
    }
}

/// 已入库文件的持久化签名，用于增量扫描判断是否需要重新索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSignature {
    pub device: Option<u64>,
    pub inode: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
    pub size: u64,
}

impl FileSignature {
    /// device、inode、mtime 或 ctime 缺失时无法证明内容和访问权限未变，必须重新索引。
    pub(crate) fn matches(self, observed: Self) -> bool {
        self.device.is_some()
            && self.inode.is_some()
            && self.mtime_ns.is_some()
            && self.ctime_ns.is_some()
            && self == observed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectorySignature {
    pub device: u64,
    pub inode: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownFileEntry {
    pub path: PathBuf,
    pub state: KnownEntryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectorySnapshot {
    pub path: PathBuf,
    pub parent_path: PathBuf,
    pub root_path: PathBuf,
    pub signature: DirectorySignature,
    pub observation_state: EntryObservationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownDirectChild {
    pub path: PathBuf,
    pub kind: SearchFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownEntryState {
    pub signature: FileSignature,
    pub stage_state: IndexedEntryStageState,
    pub(crate) mime_type: Option<String>,
    pub(crate) observation_state: EntryObservationState,
}

impl KnownEntryState {
    /// 文件没变、类型判断也不会变时可以跳过。名字能定类型时，存的类型要和名字现在给的
    /// 一样（类型库更新后按新规则重做）；类型来自内容时，签名没变内容就没变。
    pub fn allows_signature_skip(&self, observed: FileSignature, name_type: &NameType) -> bool {
        let type_unchanged = match name_type {
            NameType::Known(mime) => self.mime_type.as_deref() == Some(mime.as_str()),
            NameType::Candidates(_) | NameType::Unknown => true,
        };
        self.observation_state == EntryObservationState::Observable
            && self.stage_state.allows_signature_skip()
            && type_unchanged
            && self.signature.matches(observed)
    }
}
