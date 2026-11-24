//! 全选与反选：都作用在同一个“选择范围”上（回收站、图标展开树的当前目录、多栏的当前栏、
//! 列表的全部可见行）。

use std::collections::HashSet;
use std::path::PathBuf;

use iced::Task;

use super::super::FileBrowser;
use crate::model::{BrowserLocation, BrowserViewMode, Message};

impl FileBrowser {
    pub(in crate::app) fn select_all_in_file_selection_scope(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let paths = self.select_all_selection_scope_paths();
        self.select_scope_paths(paths)
    }

    /// 在全选的同一个范围里取反：选中的变成没选中，没选中的变成选中。
    pub(in crate::app) fn invert_selection_in_file_selection_scope(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let selected = &self.tab().selected_paths;
        let paths = self
            .select_all_selection_scope_paths()
            .into_iter()
            .filter(|path| !selected.contains(path))
            .collect();
        self.select_scope_paths(paths)
    }

    fn select_scope_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let tab = self.tab_mut();
        *tab.selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
        tab.selection_anchor = paths.first().cloned();
        if let Some(path) = paths.first().cloned() {
            self.focus_path(path);
        } else {
            self.tab_mut().selected = None;
            self.rename_input.clear();
        }
        self.preview_engine.clear_preview();
        self.context_menu = None;
        self.schedule_selected_metadata_for_all_panes()
    }

    fn select_all_selection_scope_paths(&self) -> Vec<PathBuf> {
        let tab = self.tab();
        if tab.location == BrowserLocation::Trash {
            return self.visible_entry_paths();
        }

        if tab.view_mode() == BrowserViewMode::Icons {
            let directory = self
                .icon_grid_expansion
                .as_ref()
                .map(|state| state.selection_directory())
                .or(tab.current_directory());
            return directory
                .map(|directory| self.entry_paths_in_directory(directory))
                .unwrap_or_default();
        }

        if tab.view_mode() == BrowserViewMode::Columns {
            return self
                .column_select_all_directory()
                .map(|directory| self.entry_paths_in_directory(&directory))
                .unwrap_or_default();
        }

        self.visible_entry_paths()
    }
}
