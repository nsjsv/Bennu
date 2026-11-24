//! 文件区菜单要的状态：右键菜单打开时算一次，快捷操作面板在选中变了时算一次，同一份。

use std::path::{Path, PathBuf};

use file_core::FileKind;

use crate::app::actions::ActionSubject;
use crate::app::FileBrowser;
use crate::model::{
    file_grouping_entry_visible, BrowserLocation, FileContextMenuExpansion, FileContextMenuState,
    FileDeleteAction, FileMenuPlace, FileMenuTargetKind,
};

impl FileBrowser {
    /// 对选中项的菜单（`path` 是点中的那个，选中已经定好）。右键条目菜单和快捷操作面板都从这里
    /// 拿，两边列出的动作才一定一样。
    pub(in crate::app) fn entry_menu_state(
        &self,
        path: PathBuf,
        position: iced::Point,
    ) -> FileContextMenuState {
        FileContextMenuState {
            place: self.file_menu_place(),
            target_kind: self.file_menu_target_kind(&path),
            paste_directory: self.entry_parent_directory(&path),
            target: Some(path),
            can_batch_rename: self.batch_rename_available_for_selection(),
            can_create_symlink: self.selection_all_paths_local(),
            delete_action: self.file_delete_action_for_selection(),
            position,
            expansion: FileContextMenuExpansion::None,
            // 分组方式入口只属于空白菜单,条目菜单一律不带。
            grouping_entry_visible: false,
            // 解压菜单的消费数据:选中集合里的归档与「是否在包内」。
            selection_archives: self.archive_selection_paths(),
            inside_archive: self.current_directory_is_inside_archive(),
            keyboard_highlight: None,
            actions: self.applicable_actions(&ActionSubject::Selection),
        }
    }

    /// 对文件夹本身的菜单（空白处右键、没选中时的快捷操作面板）。
    pub(in crate::app) fn blank_menu_state(
        &self,
        directory: PathBuf,
        position: iced::Point,
    ) -> FileContextMenuState {
        FileContextMenuState {
            place: FileMenuPlace::Folder,
            target: None,
            target_kind: FileMenuTargetKind::File,
            can_batch_rename: false,
            can_create_symlink: false,
            delete_action: FileDeleteAction::MoveToTrash,
            position,
            expansion: FileContextMenuExpansion::None,
            // 按打开菜单那一刻的视图模式定死入口可见性,后续视图
            // 切换不会让已打开的菜单中途长出入口。
            grouping_entry_visible: file_grouping_entry_visible(self.tab().view_mode()),
            selection_archives: Vec::new(),
            inside_archive: self.current_directory_is_inside_archive(),
            keyboard_highlight: None,
            actions: self.applicable_actions(&ActionSubject::Folder(directory.clone())),
            paste_directory: directory,
        }
    }

    fn file_menu_target_kind(&self, path: &Path) -> FileMenuTargetKind {
        if self.entry_kind(path) != Some(FileKind::Directory) {
            FileMenuTargetKind::File
        } else if self.sidebar_favorite_exists(path) {
            FileMenuTargetKind::FavoriteFolder
        } else {
            FileMenuTargetKind::Folder
        }
    }

    fn file_menu_place(&self) -> FileMenuPlace {
        match self.tab().location {
            BrowserLocation::Search(_) => FileMenuPlace::SearchResults,
            BrowserLocation::Directory(_) | BrowserLocation::Trash => FileMenuPlace::Folder,
        }
    }
}
