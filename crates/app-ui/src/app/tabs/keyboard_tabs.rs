//! 标签页的键盘命令：新建、切换、按序号切换、左移右移。都只在活动窗格里走，跳过正在关闭的标签页。

use iced::Task;

use super::super::FileBrowser;
use crate::model::{BrowserLocation, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDirection {
    Left,
    Right,
}

impl FileBrowser {
    /// Ctrl+T：新标签页打开当前位置；搜索里没有一个确定的文件夹，打开主目录。
    pub(in crate::app) fn open_new_tab_here(&mut self) -> Task<Message> {
        match &self.tab().location {
            BrowserLocation::Trash => self.open_trash_in_new_tab(),
            location => {
                let directory = location
                    .directory()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(file_core::app_dirs::home_dir);
                self.open_directory_in_new_tab(directory)
            }
        }
    }

    /// 切到左边 / 右边的标签页，到头绕回另一头。
    pub(in crate::app) fn select_neighbor_tab(&mut self, direction: TabDirection) -> Task<Message> {
        let open_tabs = self.open_tab_ids();
        let Some(active) = open_tabs
            .iter()
            .position(|id| *id == self.pane().active_tab_id)
        else {
            return Task::none();
        };
        let target = match direction {
            TabDirection::Left => (active + open_tabs.len() - 1) % open_tabs.len(),
            TabDirection::Right => (active + 1) % open_tabs.len(),
        };
        self.select_tab(open_tabs[target])
    }

    /// Alt+1…8 切到第几个标签页，Alt+9 是最后一个（浏览器的习惯）；没有那么多就不动。
    pub(in crate::app) fn select_tab_by_number(&mut self, number: u8) -> Task<Message> {
        let open_tabs = self.open_tab_ids();
        let target = if number == 9 {
            open_tabs.last()
        } else {
            open_tabs.get(usize::from(number).saturating_sub(1))
        };
        match target {
            Some(tab_id) => self.select_tab(*tab_id),
            None => Task::none(),
        }
    }

    /// 把活动标签页和左边 / 右边的标签页换位置，到头就不动。
    pub(in crate::app) fn move_active_tab(&mut self, direction: TabDirection) -> Task<Message> {
        let tabs = &self.pane().tabs;
        let active_id = self.pane().active_tab_id;
        let Some(from) = tabs.iter().position(|tab| tab.id == active_id) else {
            return Task::none();
        };
        let neighbor = match direction {
            TabDirection::Left => tabs[..from]
                .iter()
                .rposition(|tab| !self.tab_is_closing(tab.id)),
            TabDirection::Right => tabs[from + 1..]
                .iter()
                .position(|tab| !self.tab_is_closing(tab.id))
                .map(|offset| from + 1 + offset),
        };
        match neighbor {
            Some(to) => self.move_tab_within_pane(from, to),
            None => Task::none(),
        }
    }

    fn open_tab_ids(&self) -> Vec<usize> {
        self.pane()
            .tabs
            .iter()
            .filter(|tab| !self.tab_is_closing(tab.id))
            .map(|tab| tab.id)
            .collect()
    }
}
