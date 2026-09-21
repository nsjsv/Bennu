use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    sort_discovered_entry_indices, DirectoryDiscovery, DiscoveredDirectoryEntry,
    ResolvedEntryChange,
};
use iced::Task;

use super::FileBrowser;
use crate::model::{display_entries_in_discovery_order, ExpandedDirectoryStatus, Message};

/// 目录条目增量应用唯一入口：watcher 事件与文件操作事件都汇入这里。
///
/// 应用目标（互斥）：活动窗格当前目录（root）或 `expanded_directories` 中的已加载目录
/// （列视图父列、侧栏展开树、图标展开共用同一份数据）。应用只修正受影响条目并重建
/// 显示顺序，不进入 Discovering 占位、不重置未变化条目。
///
/// 返回 `None` 表示没有可应用目标或收到 `RescanRequired`，调用方退回全量重扫兜底。
impl FileBrowser {
    pub(super) fn apply_entry_changes(
        &mut self,
        directory: &Path,
        changes: &[ResolvedEntryChange],
    ) -> Option<Task<Message>> {
        if changes.is_empty() {
            return Some(Task::none());
        }
        if changes
            .iter()
            .any(|change| matches!(change, ResolvedEntryChange::RescanRequired))
        {
            return None;
        }

        let mut removed_paths = Vec::new();
        for change in changes {
            match change {
                ResolvedEntryChange::Removed { path } => removed_paths.push(path.clone()),
                ResolvedEntryChange::Renamed { from, .. } => removed_paths.push(from.clone()),
                _ => {}
            }
        }

        let mut applied = false;
        // Discovering/Loading 期间同样应用：权威扫描回写的是「扫描启动时刻」的快照，
        // 可能覆盖刚应用的增量，但下一个防抖窗口（≤250ms）会把列表纠正回来；
        // 反之跳过会让大目录扫描期间的事件永久丢失。
        if !self.is_trash_view && directory == self.current_dir {
            if let Some(discovery) = apply_changes_to_discovery(
                self.directory_discovery.as_mut(),
                changes,
                &self.options,
            ) {
                for path in &removed_paths {
                    self.selected_paths.remove(path);
                    if self.selected.as_ref() == Some(path) {
                        self.selected = None;
                    }
                    if self.selection_anchor.as_ref() == Some(path) {
                        self.selection_anchor = None;
                    }
                }
                let display = Arc::new(display_entries_in_discovery_order(&discovery));
                self.directory_discovery = Some(discovery);
                self.set_entries(display);
                self.sync_active_tab_state();
                applied = true;
            }
        }

        if let Some(expanded) = self.expanded_directories.get_mut(directory) {
            if let Some(discovery) = apply_changes_to_discovery(
                expanded.directory_discovery.as_mut(),
                changes,
                &self.options,
            ) {
                expanded.entries = display_entries_in_discovery_order(&discovery);
                expanded.directory_discovery = Some(discovery);
                applied = true;
            }
        }

        if !applied {
            return None;
        }

        // summaries：递归量失效重算（异步），直接子条目数用精确新值立即回填。
        let new_child_count = self.displayed_child_count_for(directory);
        self.invalidate_list_directory_summary(directory);
        if let Some(count) = new_child_count {
            self.list_directory_summary_cache
                .remember_direct_child_count(directory.to_path_buf(), count);
        }
        // 增量重建条目会带上 discovery 已落地的元数据，缩略图缓存 key 随之漂移；
        // 重调度可见范围，否则缩略图要等 hover/滚动才出现。
        tracing::info!(
            target: "app_ui::entry_changes",
            directory = %directory.display(),
            changes = changes.len(),
            "entry changes applied; rescheduling visible thumbnails"
        );
        Some(Task::batch([
            self.schedule_visible_list_directory_summaries(),
            self.schedule_thumbnail_refresh_for_pane(self.active_pane_id()),
        ]))
    }

    /// 操作完成时的对账：被显示目录（活动窗格当前目录 + 已加载展开目录）中，
    /// 源条目必须已消失、目标条目必须可见。任一可见目录对不上即要求全量兜底。
    pub(super) fn transfers_are_reflected_in_visible_directories(
        &self,
        transfers: &[(PathBuf, PathBuf)],
    ) -> bool {
        transfers.iter().all(|(source, target)| {
            self.directory_hides_path(source) && self.directory_shows_path(target)
        })
    }

    pub(super) fn find_discovered_entry(&self, path: &Path) -> Option<DiscoveredDirectoryEntry> {
        if let Some(discovery) = &self.directory_discovery {
            if let Some(entry) = discovery.entries.iter().find(|entry| entry.path() == path) {
                return Some(entry.clone());
            }
        }
        self.expanded_directories
            .values()
            .filter_map(|expanded| expanded.directory_discovery.as_ref())
            .find_map(|discovery| {
                discovery
                    .entries
                    .iter()
                    .find(|entry| entry.path() == path)
                    .cloned()
            })
    }

    fn directory_shows_path(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        // 目录未显示（无增量数据）时无从核对，视为通过，避免误判触发全量。
        self.displayed_entries_for(parent)
            .is_none_or(|entries| entries.iter().any(|entry| entry.path == path))
    }

    fn directory_hides_path(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        // 目录未显示（无增量数据）时无从核对，视为通过，避免误判触发全量。
        self.displayed_entries_for(parent)
            .is_none_or(|entries| !entries.iter().any(|entry| entry.path == path))
    }

    fn displayed_entries_for(&self, directory: &Path) -> Option<&[file_core::DirectoryEntry]> {
        if directory == self.current_dir {
            return Some(self.entries.as_slice());
        }
        self.expanded_directories
            .get(directory)
            .filter(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loaded))
            .map(|expanded| expanded.entries.as_slice())
    }

    fn displayed_child_count_for(&self, directory: &Path) -> Option<usize> {
        self.displayed_entries_for(directory)
            .map(<[file_core::DirectoryEntry]>::len)
    }
}

/// 把一批变更应用到单个 discovery：增删改 `entries` 后整体重算 order。
/// 整体重排是纯内存比较（与全量扫描同一排序函数），保证与权威扫描顺序一致。
fn apply_changes_to_discovery(
    discovery: Option<&mut DirectoryDiscovery>,
    changes: &[ResolvedEntryChange],
    sort_options: &file_core::ScanOptions,
) -> Option<DirectoryDiscovery> {
    let discovery = discovery?;
    let mut changed = false;
    for change in changes {
        match change {
            ResolvedEntryChange::Added(entry) | ResolvedEntryChange::ContentChanged(entry) => {
                if upsert_discovered_entry(discovery, entry) {
                    changed = true;
                }
            }
            ResolvedEntryChange::Renamed { from, to } => {
                if remove_discovered_entry(discovery, from) {
                    changed = true;
                }
                if upsert_discovered_entry(discovery, to) {
                    changed = true;
                }
            }
            ResolvedEntryChange::Removed { path } => {
                if remove_discovered_entry(discovery, path) {
                    changed = true;
                }
            }
            ResolvedEntryChange::RescanRequired => {}
        }
    }
    if !changed {
        return None;
    }
    discovery.order = Arc::new(sort_discovered_entry_indices(
        &discovery.entries,
        sort_options,
    ));
    Some(discovery.clone())
}

fn upsert_discovered_entry(
    discovery: &mut DirectoryDiscovery,
    entry: &DiscoveredDirectoryEntry,
) -> bool {
    // 隐藏过滤与全量扫描一致：不显示隐藏条目的视图不接受隐藏条目的增量。
    if entry.is_hidden() {
        return false;
    }
    let entries = Arc::make_mut(&mut discovery.entries);
    match entries
        .iter()
        .position(|existing| existing.path() == entry.path())
    {
        Some(index) => {
            entries[index] = entry.clone();
        }
        None => entries.push(entry.clone()),
    }
    true
}

fn remove_discovered_entry(discovery: &mut DirectoryDiscovery, path: &Path) -> bool {
    let entries = Arc::make_mut(&mut discovery.entries);
    match entries.iter().position(|existing| existing.path() == path) {
        Some(index) => {
            entries.remove(index);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests;
