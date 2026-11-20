//! 点名路径：用户明确要去的地方（地址栏输入、定位找到的文件）在上一级列表里看得见，哪怕它是
//! 隐藏的。规则本身是 `ScanOptions::includes_entry`；这里管标签的点名怎么换、换了以后哪些已经
//! 列出来的目录要按新规则重读、文件监听要为哪些目录放行隐藏条目的变化。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::{revealed_hidden_entries, ScanOptions};
use iced::Task;

use super::FileBrowser;
use crate::model::{BrowserPaneId, Message};

impl FileBrowser {
    /// 窗格活动标签列目录的规则。扫描都从这里取，同一个目录不论哪条路径去读，列出来的都一样。
    pub(super) fn listing_options(&self, pane_id: BrowserPaneId) -> ScanOptions {
        self.pane_by_id(pane_id)
            .expect("a directory listing belongs to an existing pane")
            .tab()
            .listing_options(&self.options)
    }

    /// 活动标签改为点名 `path`。不显示隐藏文件时，新旧路径上的隐藏部分有出入的，所在目录的
    /// 列表会多一项或少一项：已经列着的按新规则重读，正在读的重新发起。
    pub(super) fn reveal_path(&mut self, path: PathBuf) -> Task<Message> {
        let previous = self.tab_mut().revealed_path.replace(path.clone());
        if self.options.include_hidden || previous.as_ref() == Some(&path) {
            return Task::none();
        }
        let before = previous
            .as_deref()
            .into_iter()
            .flat_map(revealed_hidden_entries)
            .collect::<HashSet<_>>();
        let after = revealed_hidden_entries(&path).collect::<HashSet<_>>();
        let stale_directories = before
            .symmetric_difference(&after)
            .filter_map(|entry| entry.parent())
            .map(Path::to_path_buf)
            .collect::<HashSet<_>>();
        let pane_id = self.active_pane_id();
        Task::batch(
            stale_directories
                .into_iter()
                .map(|directory| self.reload_observed_directory_in_pane(pane_id, &directory))
                .collect::<Vec<_>>(),
        )
    }

    /// 关掉显示隐藏文件时，各标签开着的那串栏照样看得见，不会剩下一串找不到上级的栏。
    pub(super) fn reveal_open_column_chains(&mut self) {
        for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            tab.revealed_path = tab.deepest_open_column_directory.clone();
        }
    }

    /// 列着点名隐藏条目的目录（只看可见窗格）。它们的监听要放行隐藏条目的变化，留给各标签
    /// 按自己的规则取舍：几个窗格在同一目录点名了不同的隐藏条目也不会漏。
    pub(super) fn directories_listing_revealed_entries(&self) -> HashSet<&Path> {
        self.pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter_map(|pane_id| self.pane_by_id(pane_id))
            .filter_map(|pane| pane.tab().revealed_path.as_deref())
            .flat_map(revealed_hidden_entries)
            .filter_map(Path::parent)
            .collect()
    }
}
