//! 搜索范围：从哪个文件夹发起、可选的范围档（当前文件夹、上次位置、家目录、所有已索引
//! 位置）和它们对应的查询范围。

use std::path::{Path, PathBuf};

use file_operations::{path_after_completed_migrations, CompletedPathMigration};
use file_search::SearchScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchDirectoryScope {
    CurrentFolder,
    LastLocation,
    Home,
    AllIndexedLocations,
}

impl SearchDirectoryScope {
    pub(crate) fn label(self) -> String {
        match self {
            Self::CurrentFolder => tr!("Current folder"),
            Self::LastLocation => tr!("Last location"),
            Self::Home => tr!("Home"),
            Self::AllIndexedLocations => tr!("All indexed locations"),
        }
    }
}

/// 最近一次搜索实际使用的范围，随用户偏好持久化；下一次搜索默认沿用它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LastSearchScope {
    Global,
    Directory(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct SearchRootSnapshot {
    pub(super) current_folder: PathBuf,
    home: PathBuf,
    pub(super) last_location: Option<LastSearchScope>,
    selected_scope: SearchDirectoryScope,
}

impl SearchRootSnapshot {
    pub(crate) fn new(
        current_folder: PathBuf,
        home: PathBuf,
        last_location: Option<LastSearchScope>,
    ) -> Self {
        // 默认档恢复上次范围；无记录时沿用"在 Home 默认 Home，否则当前文件夹"。
        // 目录记录若与当前文件夹或 Home 相同，直接归并到对应现有档，不产生重复选项。
        let selected_scope = match &last_location {
            Some(LastSearchScope::Global) => SearchDirectoryScope::AllIndexedLocations,
            Some(LastSearchScope::Directory(directory))
                if directory.as_path() == current_folder.as_path() =>
            {
                SearchDirectoryScope::CurrentFolder
            }
            Some(LastSearchScope::Directory(directory))
                if directory.as_path() == home.as_path() =>
            {
                SearchDirectoryScope::Home
            }
            Some(_) => SearchDirectoryScope::LastLocation,
            None if current_folder == home => SearchDirectoryScope::Home,
            None => SearchDirectoryScope::CurrentFolder,
        };
        Self {
            current_folder,
            home,
            last_location,
            selected_scope,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => &self.current_folder,
            SearchDirectoryScope::Home | SearchDirectoryScope::AllIndexedLocations => &self.home,
            // 不变量：selected_scope == LastLocation 当且仅当 last_location 是
            // 一个不等于 current_folder/home 的目录记录。
            SearchDirectoryScope::LastLocation => {
                self.last_location_directory().unwrap_or(&self.home)
            }
        }
    }

    pub(crate) fn query_scope(&self) -> SearchScope {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => {
                SearchScope::Directory(self.current_folder.clone())
            }
            SearchDirectoryScope::Home => SearchScope::Directory(self.home.clone()),
            SearchDirectoryScope::AllIndexedLocations => SearchScope::Global,
            SearchDirectoryScope::LastLocation => match &self.last_location {
                Some(LastSearchScope::Directory(directory)) => {
                    SearchScope::Directory(directory.clone())
                }
                // 同 path()：LastLocation 档只在有目录记录时可用。
                _ => SearchScope::Global,
            },
        }
    }

    /// LastLocation 档指向的目录；仅在持有目录记录时存在。
    pub(crate) fn last_location_directory(&self) -> Option<&Path> {
        match &self.last_location {
            Some(LastSearchScope::Directory(directory)) => Some(directory.as_path()),
            _ => None,
        }
    }

    /// 当前选定范围的持久化事实，用于随用户偏好落盘。
    pub(crate) fn last_search_scope(&self) -> LastSearchScope {
        match self.selected_scope {
            SearchDirectoryScope::CurrentFolder => {
                LastSearchScope::Directory(self.current_folder.clone())
            }
            SearchDirectoryScope::Home => LastSearchScope::Directory(self.home.clone()),
            SearchDirectoryScope::AllIndexedLocations => LastSearchScope::Global,
            SearchDirectoryScope::LastLocation => match &self.last_location {
                Some(directory @ LastSearchScope::Directory(_)) => directory.clone(),
                _ => LastSearchScope::Global,
            },
        }
    }

    pub(crate) fn selected_scope(&self) -> SearchDirectoryScope {
        self.selected_scope
    }

    pub(crate) fn available_scopes(&self) -> Vec<SearchDirectoryScope> {
        let mut scopes = if self.current_folder == self.home {
            vec![
                SearchDirectoryScope::Home,
                SearchDirectoryScope::AllIndexedLocations,
            ]
        } else {
            vec![
                SearchDirectoryScope::CurrentFolder,
                SearchDirectoryScope::Home,
                SearchDirectoryScope::AllIndexedLocations,
            ]
        };
        // 上次位置只在"不与现有档重复"时出现：等于当前文件夹并入 CurrentFolder，
        // 等于 Home 并入 Home，全局并入 AllIndexedLocations。
        let last_directory = self.last_location_directory();
        if let Some(directory) = last_directory {
            if directory != self.current_folder && directory != self.home {
                let insert_at = if self.current_folder == self.home {
                    0
                } else {
                    1
                };
                scopes.insert(insert_at, SearchDirectoryScope::LastLocation);
            }
        }
        scopes
    }

    /// 起点文件夹被改名或移动后跟着走：关闭搜索回到它，按“当前文件夹”范围重搜也搜它。
    pub(crate) fn migrate_origin(&mut self, migrations: &[CompletedPathMigration]) {
        self.current_folder = path_after_completed_migrations(&self.current_folder, migrations);
    }

    pub(crate) fn select_scope(&mut self, scope: SearchDirectoryScope) -> bool {
        if self.selected_scope == scope || !self.available_scopes().contains(&scope) {
            return false;
        }
        self.selected_scope = scope;
        true
    }
}
