use std::path::{Path, PathBuf};

use super::search::{SearchWorkspaceSessionId, SearchWorkspaceState};

/// 标签正在看的位置。回收站和搜索结果不是目录、没有路径：需要真实目录的代码只能经
/// [`BrowserLocation::directory`] 拿到。一次搜索的全部状态（词、筛选、在途请求、结果
/// 详情）就是这个位置本身，随标签走。
#[derive(Debug, Clone)]
pub(crate) enum BrowserLocation {
    Directory(PathBuf),
    Trash,
    Search(Box<SearchWorkspaceState>),
}

/// 位置的身份：缩略图分组、视口归属、相等比较只认它。搜索按会话号区分。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LocationKey {
    Directory(PathBuf),
    Trash,
    Search(SearchWorkspaceSessionId),
}

impl BrowserLocation {
    pub(crate) fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(directory) => Some(directory),
            Self::Trash | Self::Search(_) => None,
        }
    }

    pub(crate) fn key(&self) -> LocationKey {
        match self {
            Self::Directory(directory) => LocationKey::Directory(directory.clone()),
            Self::Trash => LocationKey::Trash,
            Self::Search(search) => LocationKey::Search(search.session_id()),
        }
    }

    /// 条目能否在原地展开：展开要挂在一个文件夹下，只有文件夹位置有。
    pub(crate) fn expands_entries_in_place(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    /// 文件夹条目是不是能打开、能往里放东西的真实文件夹。回收站里的条目只能恢复或删除。
    pub(crate) fn folder_entries_are_live(&self) -> bool {
        !matches!(self, Self::Trash)
    }

    pub(crate) fn search(&self) -> Option<&SearchWorkspaceState> {
        match self {
            Self::Search(search) => Some(search),
            Self::Directory(_) | Self::Trash => None,
        }
    }

    pub(crate) fn search_mut(&mut self) -> Option<&mut SearchWorkspaceState> {
        match self {
            Self::Search(search) => Some(search),
            Self::Directory(_) | Self::Trash => None,
        }
    }
}

impl PartialEq for BrowserLocation {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for BrowserLocation {}
