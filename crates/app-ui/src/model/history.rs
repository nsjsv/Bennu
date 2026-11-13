use std::path::{Path, PathBuf};

use file_operations::{path_after_completed_migrations, CompletedPathMigration};

use super::search::SearchRecall;

/// 前进后退历史里的一站：一个文件夹，或一次搜索（回去时按记下的意图重搜）。
/// 回收站不进历史。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HistoryEntry {
    Directory(PathBuf),
    Search(SearchRecall),
}

impl HistoryEntry {
    /// 会话里只存文件夹：一次搜索存成它的起点文件夹。
    pub(crate) fn session_directory(&self) -> &Path {
        match self {
            Self::Directory(directory) => directory,
            Self::Search(recall) => &recall.origin,
        }
    }

    pub(crate) fn is_directory(&self, directory: &Path) -> bool {
        matches!(self, Self::Directory(entry) if entry == directory)
    }

    /// 这一站是否落在 `subtree` 里（文件夹本身或搜索的起点）。
    pub(crate) fn lies_in(&self, subtree: &Path) -> bool {
        self.session_directory().starts_with(subtree)
    }

    pub(crate) fn migrate(&mut self, migrations: &[CompletedPathMigration]) {
        match self {
            Self::Directory(directory) => {
                *directory = path_after_completed_migrations(directory, migrations);
            }
            Self::Search(recall) => {
                recall.origin = path_after_completed_migrations(&recall.origin, migrations);
            }
        }
    }
}
