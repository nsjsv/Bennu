//! 快捷操作面板：右键菜单里对选中项动手的那几项和「动作」，跟着选中变。菜单状态跟右键菜单
//! 同一个函数算（`entry_menu_state` / `blank_menu_state`），算一次要把选中项逐个看一遍，所以
//! 只在选中、位置、视图或动作库变了时重算。

use std::path::PathBuf;

use super::FileBrowser;
use crate::model::{
    BrowserLocation, BrowserPaneId, BrowserViewMode, FileContextMenuState, LocationKey,
};
use crate::right_dock::DockPanel;

#[derive(Debug, Default)]
pub(crate) struct QuickActionsPanel {
    computed_for: Option<QuickActionsInputs>,
    menu: Option<FileContextMenuState>,
}

impl QuickActionsPanel {
    /// 回收站里、没有位置时为 None：那里没有能做的。
    pub(crate) fn menu(&self) -> Option<&FileContextMenuState> {
        self.menu.as_ref()
    }
}

/// 菜单状态取决于的东西，都是便宜的比较。条目的类型跟着列表版本号走；展开的子文件夹里换了
/// 类型（文件换成同名文件夹）不算——点的时候动作还会再判断一次适不适用。
#[derive(Debug, Clone, PartialEq, Eq)]
struct QuickActionsInputs {
    pane: BrowserPaneId,
    tab: usize,
    location: LocationKey,
    view_mode: BrowserViewMode,
    selected: Option<PathBuf>,
    selection_revision: u64,
    entries_revision: u64,
    actions_revision: u64,
}

impl FileBrowser {
    /// `update` 出口：面板显示着、输入变了才重算。
    pub(super) fn sync_quick_actions_panel(&mut self) {
        if !self.right_dock.layout().shows(DockPanel::QuickActions) {
            return;
        }
        let tab = self.tab();
        let inputs = QuickActionsInputs {
            pane: self.active_pane_id(),
            tab: tab.id,
            location: tab.location.key(),
            view_mode: tab.view_mode(),
            selected: tab.selected.clone(),
            selection_revision: tab.selected_paths.revision(),
            entries_revision: tab.entries.revision(),
            actions_revision: self.action_library.revision(),
        };
        if self.quick_actions.computed_for.as_ref() == Some(&inputs) {
            return;
        }
        self.quick_actions.menu = self.quick_actions_menu();
        self.quick_actions.computed_for = Some(inputs);
    }

    /// 有选中时是条目菜单（以焦点项为准，跟按菜单键打开的一样），没有时是当前文件夹的空白菜单。
    fn quick_actions_menu(&self) -> Option<FileContextMenuState> {
        let tab = self.tab();
        if tab.location == BrowserLocation::Trash {
            return None;
        }
        match tab.selected.clone() {
            Some(path) => Some(self.entry_menu_state(path, iced::Point::ORIGIN)),
            None => tab.current_directory().map(|directory| {
                self.blank_menu_state(directory.to_path_buf(), iced::Point::ORIGIN)
            }),
        }
    }
}

#[cfg(test)]
#[path = "quick_actions_panel_tests.rs"]
mod tests;
