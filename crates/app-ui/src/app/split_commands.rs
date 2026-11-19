//! 分屏的键盘命令：开 / 合分屏、切到另一侧、把选中项复制或移动到另一侧。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::{BrowserPaneId, BrowserPaneLayout, Message, PendingOperation, SplitAxis};

impl FileBrowser {
    /// F3：只有一个窗格时，在右边再开一个窗格显示当前文件夹；已经分屏时，把另一侧的标签页
    /// 全部并回当前这一侧（一个都不丢），当前标签页不变。
    pub(super) fn toggle_split_view(&mut self) -> Task<Message> {
        let Some(other) = self.other_pane_id() else {
            let directory = self
                .tab()
                .current_directory()
                .map(PathBuf::from)
                .unwrap_or_else(file_core::app_dirs::home_dir);
            return self.open_directory_in_split(directory, SplitAxis::Horizontal);
        };
        let cancel_address_editing = self.cancel_address_editing();
        self.clear_icon_grid_expansion_for_context_change();
        if self.merge_pane_into(other, self.active_pane_id()).is_none() {
            return cancel_address_editing;
        }
        self.reset_active_pane_context();
        self.sync_tab_bar_visibility();
        Task::batch([cancel_address_editing, self.request_browser_session_save()])
    }

    /// Ctrl+F3：键盘焦点换到另一侧的窗格。
    pub(super) fn focus_other_pane(&mut self) -> Task<Message> {
        if let Some(other) = self.other_pane_id() {
            self.activate_pane(other);
        }
        Task::none()
    }

    /// Shift+F5 / Shift+F6：选中项复制 / 移动到另一侧当前显示的文件夹，和粘贴走同一个落地入口
    /// （冲突询问、包内只读都一样），但不碰剪切状态。没分屏或另一侧不是文件夹时不做事。
    pub(super) fn transfer_selection_to_other_pane(
        &mut self,
        operation: fn(Vec<PathBuf>) -> PendingOperation,
    ) -> Task<Message> {
        let Some(destination) = self
            .other_pane_id()
            .and_then(|other| self.pane_by_id(other))
            .and_then(|pane| pane.tab().current_directory())
            .map(PathBuf::from)
        else {
            return Task::none();
        };
        let sources = self.selected_paths_for_operation();
        if sources.is_empty() {
            return Task::none();
        }
        self.land_operation_in(destination, operation(sources))
            .unwrap_or_else(Task::none)
    }

    fn other_pane_id(&self) -> Option<BrowserPaneId> {
        match self.pane_layout {
            BrowserPaneLayout::Single { .. } => None,
            BrowserPaneLayout::Split {
                first,
                second,
                active,
                ..
            } => Some(if active == first { second } else { first }),
        }
    }
}

#[cfg(test)]
#[path = "split_commands_tests.rs"]
mod tests;
