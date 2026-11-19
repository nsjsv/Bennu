//! 只有键盘入口的命令。鼠标入口（工具栏、侧栏、右键菜单）各自已有函数，这里只补上
//! “作用在哪个文件夹 / 哪个选中项”的键盘语义。

use std::path::PathBuf;

use file_core::FileKind;
use iced::Task;

use super::FileBrowser;
use crate::model::{Message, NavigationMode, SearchInputFocusCheckOrigin};

impl FileBrowser {
    pub(super) fn navigate_home(&mut self) -> Task<Message> {
        Task::batch([
            self.commit_rename_if_active(),
            self.navigate_to(
                file_core::app_dirs::home_dir(),
                NavigationMode::RecordHistory,
            ),
        ])
    }

    /// Ctrl+F：光标放进搜索框并选中里面的字，接着打的就是新的搜索词。
    pub(super) fn focus_search_input(&mut self) -> Task<Message> {
        let input = crate::view::search_input_id();
        iced::widget::operation::focus(input.clone())
            .chain(iced::widget::operation::select_all(input))
            .chain(
                self.request_search_input_focus_check(
                    SearchInputFocusCheckOrigin::KeyboardTraversal,
                ),
            )
    }

    /// Ctrl+Enter：选中的文件夹在新标签页打开；选中的是文件就不做事。
    pub(super) fn open_selected_in_new_tab(&mut self) -> Task<Message> {
        let Some(path) = self.tab().selected.clone() else {
            return Task::none();
        };
        if !self.entry_acts_as_directory(&path) {
            return Task::none();
        }
        Task::batch([
            self.commit_rename_if_active(),
            self.open_directory_in_new_tab(path),
        ])
    }

    /// Ctrl+B：只选中了一个文件夹时收它，否则收当前文件夹。
    pub(super) fn add_keyboard_target_to_favorites(&mut self) -> Task<Message> {
        let tab = self.tab();
        let selected_folder = tab
            .selected
            .clone()
            .filter(|_| tab.selected_paths.len() <= 1)
            .filter(|path| self.entry_kind(path) == Some(FileKind::Directory));
        let Some(folder) = selected_folder.or_else(|| tab.current_directory().map(PathBuf::from))
        else {
            return Task::none();
        };
        self.add_sidebar_bookmark(folder)
    }

    pub(super) fn create_folder_from_keyboard(&mut self) -> Task<Message> {
        match self.keyboard_target_directory() {
            Some(directory) => self.create_directory_in(directory),
            None => Task::none(),
        }
    }

    pub(super) fn create_file_from_keyboard(&mut self) -> Task<Message> {
        match self.keyboard_target_directory() {
            Some(directory) => self.create_empty_file_in(directory),
            None => Task::none(),
        }
    }

    pub(super) fn open_external_terminal_from_keyboard(&mut self) -> Task<Message> {
        match self.keyboard_target_directory() {
            Some(directory) => self.open_terminal_here(directory),
            None => Task::none(),
        }
    }
}

#[cfg(test)]
#[path = "shortcut_commands_tests.rs"]
mod tests;
