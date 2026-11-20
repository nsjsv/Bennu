use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    sort_discovered_entry_indices, DirectoryDiscovery, DirectoryEntry, DiscoveredDirectoryEntry,
    ResolvedEntryChange,
};
use iced::Task;

use super::FileBrowser;
use crate::model::{
    display_entries_in_discovery_order, BrowserPane, BrowserPaneId, BrowserTab, ContextMenuState,
    DirectoryEntrySnapshot, ExpandedDirectory, ExpandedDirectoryStatus, Message,
};

/// 目录条目增量应用唯一入口：watcher 事件与文件操作事件都汇入这里。
///
/// 增量的作用域是「所有窗格的所有标签里显示该目录的位置」：标签的当前
/// 目录与它已加载的展开目录各自独立，凡命中的都应用一次——同一目录被
/// 多个窗格显示时每份都要修正，后台标签也一样，切回时才不会看到陈旧列表。
/// 应用只修正受影响条目并重建显示顺序，不进入 Discovering 占位、不重置
/// 未变化条目。目标正在被扫描时变化不碰旧列表，排进该标签的
/// `pending_changes`，扫描落地后按到达顺序重放（见 `replay_changes_*`）。
///
/// 返回 `None` 表示没有任何位置显示着这个目录（含 RescanRequired、discovery
/// 尚未加载），调用方退回全量重扫兜底；个别位置失败不阻塞其它位置。显示着
/// 但变化看不见（被隐藏规则滤掉、删的本来就不在列表里）是 `Unchanged`，
/// 不能当成「没法应用」去重扫——那会让一个一直被写的隐藏文件不停地整目录重扫。
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

        let removed_paths = removed_paths(changes);
        // 全局选项提前克隆一份：遍历窗格要独占借用 self.panes。
        let base_options = self.options.clone();
        let mut placed_any = false;
        let mut affected_any = false;
        // summaries 需要的直接子条目数必须来自刚应用成功的位置：显示该目录
        // 但 discovery 尚未加载的标签，其列表是陈旧的，不能作为计数来源。
        let mut applied_child_count = None;
        // 只有窗格的活动标签可见，缩略图只为它们重排。
        let mut thumbnail_pane_ids: Vec<BrowserPaneId> = Vec::new();

        for pane in &mut self.panes {
            let active_tab_id = pane.active_tab_id;
            let load_generation = pane.directory_load_generation;
            for tab in &mut pane.tabs {
                // 每个标签按自己的点名路径决定哪些隐藏条目在列表里。
                let sort_options = tab.listing_options(&base_options);
                // 窗格同一时刻只扫描活动标签；后台标签的 Discovering 只是还没
                // 加载过，切回时会整个重扫，没有扫描可等。
                let scan_generation = (tab.id == active_tab_id
                    && tab.directory_collection_phase.scan_in_flight())
                .then_some(load_generation);
                let placements = [
                    place_in_current_directory(
                        tab,
                        directory,
                        changes,
                        &removed_paths,
                        scan_generation,
                        &sort_options,
                    ),
                    place_in_expanded_directory(tab, directory, changes, &sort_options),
                ];
                for placement in placements.into_iter().flatten() {
                    placed_any = true;
                    if !matches!(placement, Placement::Unchanged) {
                        affected_any = true;
                    }
                    let Placement::Applied { child_count } = placement else {
                        continue;
                    };
                    applied_child_count.get_or_insert(child_count);
                    if tab.id == active_tab_id && !thumbnail_pane_ids.contains(&pane.id) {
                        thumbnail_pane_ids.push(pane.id);
                    }
                }
            }
        }

        if !placed_any {
            return None;
        }
        if !affected_any {
            return Some(Task::none());
        }
        self.release_interactions_targeting(|path| removed_paths.iter().any(|gone| gone == path));
        self.entries_replaced();

        // summaries：递归量失效重算（异步），直接子条目数用精确新值立即回填。
        self.invalidate_list_directory_summary(directory);
        if let Some(count) = applied_child_count {
            self.list_directory_summary_cache
                .remember_direct_child_count(directory.to_path_buf(), count);
        }
        // 增量重建条目会带上 discovery 已落地的元数据，缩略图缓存 key 随之
        // 漂移；重调度每个受影响面板的可见范围，否则这些面板的缩略图要等
        // hover/滚动才出现。
        tracing::info!(
            target: "app_ui::entry_changes",
            directory = %directory.display(),
            changes = changes.len(),
            panes = thumbnail_pane_ids.len(),
            "entry changes applied; rescheduling visible thumbnails"
        );
        let visible_pane_ids = self.pane_layout.visible_pane_ids();
        let mut tasks = vec![
            self.clamp_pane_viewports(&visible_pane_ids),
            self.schedule_visible_list_directory_summaries(),
        ];
        tasks.extend(
            thumbnail_pane_ids
                .drain(..)
                .map(|pane_id| self.schedule_thumbnail_refresh_for_pane(pane_id)),
        );
        Some(Task::batch(tasks))
    }

    /// 需要监听的目录：每个可见窗格活动标签的当前目录与展开目录，加上图标展开
    /// 的目录。扫描期间也监听——变化会排队等扫描落地，拆掉订阅只会丢事件。
    /// 回收站视图不走文件监听。
    pub(super) fn watched_directories(&self) -> HashSet<PathBuf> {
        let visible_tabs = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter_map(|pane_id| self.pane_by_id(pane_id))
            .map(BrowserPane::tab);
        visible_tabs
            .flat_map(|tab| {
                tab.current_directory()
                    .map(Path::to_path_buf)
                    .into_iter()
                    .chain(tab.expanded_directories.keys().cloned())
            })
            .chain(self.icon_grid_expansion_watch_directories())
            .collect()
    }

    /// 操作完成时的对账：对每个可见面板，凡显示某转移父目录的面板都必须
    /// 已反映该转移（源条目消失、目标条目可见）；没有任何面板显示该父
    /// 目录时无从核对，视为通过。任一面板对不上即要求全量兜底。
    pub(super) fn transfers_are_reflected_in_visible_directories(
        &self,
        transfers: &[(PathBuf, PathBuf)],
    ) -> bool {
        transfers.iter().all(|(source, target)| {
            self.path_hidden_in_all_displaying_panes(source)
                && self.path_shown_in_all_displaying_panes(target)
        })
    }

    /// 在任意窗格的任意标签里找路径对应的 discovery 条目：跨面板移动的
    /// 源条目可能只被某个非活动窗格或后台标签显示，查不到会导致整批移动
    /// 跳过增量、各处一起陈旧。
    pub(super) fn find_discovered_entry(&self, path: &Path) -> Option<DiscoveredDirectoryEntry> {
        let find_in = |discovery: &DirectoryDiscovery| {
            discovery
                .entries
                .iter()
                .find(|entry| entry.path() == path)
                .cloned()
        };
        self.panes
            .iter()
            .flat_map(|pane| &pane.tabs)
            .find_map(|tab| {
                tab.directory_discovery
                    .as_ref()
                    .and_then(find_in)
                    .or_else(|| {
                        tab.expanded_directories
                            .values()
                            .filter_map(|expanded| expanded.directory_discovery.as_ref())
                            .find_map(find_in)
                    })
            })
    }

    /// 枚举显示 `directory` 的所有可见面板的条目列表（各取活动标签；
    /// 当前目录优先于展开目录，沿用单面板时代的语义）。
    fn displayed_entry_lists_across_panes<'a>(
        &'a self,
        directory: &Path,
    ) -> Vec<&'a [DirectoryEntry]> {
        self.pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter_map(|pane_id| self.pane_by_id(pane_id))
            .filter_map(|pane| {
                let tab = pane.tab();
                if tab.current_directory() == Some(directory) {
                    Some(tab.entries.as_slice())
                } else {
                    tab.expanded_directories
                        .get(directory)
                        .filter(|expanded| {
                            matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
                        })
                        .map(|expanded| expanded.entries.as_slice())
                }
            })
            .collect()
    }

    /// 「源条目必须消失」逐面板核对；空列表 = 没有任何面板显示该目录，
    /// 无从核对视为通过，避免误判触发全量。
    fn path_hidden_in_all_displaying_panes(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let lists = self.displayed_entry_lists_across_panes(parent);
        lists.is_empty()
            || lists
                .iter()
                .all(|entries| !entries.iter().any(|entry| entry.path() == path))
    }

    /// 「目标条目必须可见」逐面板核对；空列表同上视为通过。
    fn path_shown_in_all_displaying_panes(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let lists = self.displayed_entry_lists_across_panes(parent);
        lists.is_empty()
            || lists
                .iter()
                .all(|entries| entries.iter().any(|entry| entry.path() == path))
    }
}

fn removed_paths(changes: &[ResolvedEntryChange]) -> Vec<PathBuf> {
    changes
        .iter()
        .filter_map(|change| match change {
            ResolvedEntryChange::Removed { path } => Some(path.clone()),
            ResolvedEntryChange::Renamed { from, .. } => Some(from.clone()),
            _ => None,
        })
        .collect()
}

/// 一份变化在某个标签里的去向。
impl FileBrowser {
    /// 右键菜单、重命名所指的条目没了就收起它们，别的交互不动。磁盘上的变化
    /// 不是用户操作，不能像手动刷新那样把手上的东西全清掉。
    pub(super) fn release_interactions_targeting(&mut self, vanished: impl Fn(&Path) -> bool) {
        if matches!(
            &self.context_menu,
            Some(ContextMenuState::FileArea(menu)) if menu.target.as_deref().is_some_and(&vanished)
        ) {
            self.context_menu = None;
        }
        if self.renaming.as_deref().is_some_and(&vanished) {
            self.renaming = None;
        }
    }

    /// 一次扫描结果整体替换了 `directory` 的列表：右键菜单、重命名所指的条目里，
    /// 位于该目录却不在新列表里的就是没了；其它目录里的目标与这次结果无关。
    pub(super) fn interaction_targets_missing_from(
        &self,
        directory: &Path,
        listed: &[DirectoryEntry],
    ) -> Vec<PathBuf> {
        let menu_target = match &self.context_menu {
            Some(ContextMenuState::FileArea(menu)) => menu.target.as_deref(),
            _ => None,
        };
        [menu_target, self.renaming.as_deref()]
            .into_iter()
            .flatten()
            .filter(|target| {
                target.parent() == Some(directory)
                    && !listed.iter().any(|entry| entry.path() == *target)
            })
            .map(Path::to_path_buf)
            .collect()
    }
}

pub(super) enum Placement {
    /// 已落到列表上；`child_count` 是应用后的直接子条目数（供摘要缓存回填）。
    Applied { child_count: usize },
    /// 这里显示着该目录，但这批变化在列表上看不见。
    Unchanged,
    /// 目标正在扫描，已排队，落地后重放。
    Queued,
}

/// `scan_generation` 为 `Some` 表示该标签的当前目录正在被扫描：扫描读到的
/// 是启动那一刻的磁盘状态，此时直接改旧列表会被落地的结果覆盖，所以排队。
/// 回收站视图的列表来自回收站快照而非文件系统 discovery，不走增量。
fn place_in_current_directory(
    tab: &mut BrowserTab,
    directory: &Path,
    changes: &[ResolvedEntryChange],
    removed_paths: &[PathBuf],
    scan_generation: Option<u64>,
    sort_options: &file_core::ScanOptions,
) -> Option<Placement> {
    if tab.current_directory() != Some(directory) {
        return None;
    }
    if let Some(scan_generation) = scan_generation {
        tab.queue_changes_during_scan(directory, scan_generation, changes);
        return Some(Placement::Queued);
    }
    apply_to_current_directory(tab, changes, removed_paths, sort_options)
}

/// 标签里已加载的展开目录（列视图父列、侧栏树、图标展开共用）；重新加载
/// 中的展开目录同样排队。
fn place_in_expanded_directory(
    tab: &mut BrowserTab,
    directory: &Path,
    changes: &[ResolvedEntryChange],
    sort_options: &file_core::ScanOptions,
) -> Option<Placement> {
    let expanded = tab.expanded_directories.get(directory)?;
    if expanded.status.scan_in_flight() {
        let scan_generation = expanded.load_generation;
        tab.queue_changes_during_scan(directory, scan_generation, changes);
        return Some(Placement::Queued);
    }
    apply_changes_to_expanded(
        tab.expanded_directories.get_mut(directory)?,
        changes,
        sort_options,
    )
}

/// 当前目录扫描落地后，把它进行期间排队的变化按到达顺序重放到结果上。
/// 与直接应用走同一个函数，增量语义完全一致。
pub(super) fn replay_changes_queued_during_current_scan(
    tab: &mut BrowserTab,
    scan_generation: u64,
    sort_options: &file_core::ScanOptions,
) {
    let Some(directory) = tab.current_directory().map(Path::to_path_buf) else {
        return;
    };
    let changes = tab.take_changes_queued_during_scan(&directory, scan_generation);
    if !changes.is_empty() {
        apply_to_current_directory(tab, &changes, &removed_paths(&changes), sort_options);
    }
}

/// 标签当前目录的主列表；没有 discovery（还没加载到可应用状态）时为 `None`。
fn apply_to_current_directory(
    tab: &mut BrowserTab,
    changes: &[ResolvedEntryChange],
    removed_paths: &[PathBuf],
    sort_options: &file_core::ScanOptions,
) -> Option<Placement> {
    let discovery = tab.directory_discovery.as_mut()?;
    if !apply_changes_to_discovery(discovery, changes, sort_options) {
        return Some(Placement::Unchanged);
    }
    let display = DirectoryEntrySnapshot::from(display_entries_in_discovery_order(discovery));
    // 被移走的源路径不能继续留在选中集合或锚点上。
    for path in removed_paths {
        tab.selected_paths.remove(path);
        if tab.selected.as_ref() == Some(path) {
            tab.selected = None;
        }
        if tab.selection_anchor.as_ref() == Some(path) {
            tab.selection_anchor = None;
        }
    }
    let child_count = display.len();
    tab.entries = display;
    Some(Placement::Applied { child_count })
}

/// 展开目录的扫描落地后同样用它重放排队的变化。
pub(super) fn apply_changes_to_expanded(
    expanded: &mut ExpandedDirectory,
    changes: &[ResolvedEntryChange],
    sort_options: &file_core::ScanOptions,
) -> Option<Placement> {
    let discovery = expanded.directory_discovery.as_mut()?;
    if !apply_changes_to_discovery(discovery, changes, sort_options) {
        return Some(Placement::Unchanged);
    }
    expanded.entries = display_entries_in_discovery_order(discovery).into();
    Some(Placement::Applied {
        child_count: expanded.entries.len(),
    })
}

/// 把一批变更原地应用到单个 discovery：增删改 `entries` 后整体重算 order，
/// 返回列表上是否真有变化。整体重排是纯内存比较（与全量扫描同一排序函数），
/// 保证与权威扫描顺序一致。
fn apply_changes_to_discovery(
    discovery: &mut DirectoryDiscovery,
    changes: &[ResolvedEntryChange],
    sort_options: &file_core::ScanOptions,
) -> bool {
    let mut changed = false;
    for change in changes {
        match change {
            ResolvedEntryChange::Added(entry) | ResolvedEntryChange::ContentChanged(entry) => {
                if upsert_discovered_entry(discovery, entry, sort_options) {
                    changed = true;
                }
            }
            ResolvedEntryChange::Renamed { from, to } => {
                if remove_discovered_entry(discovery, from) {
                    changed = true;
                }
                if upsert_discovered_entry(discovery, to, sort_options) {
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
    if changed {
        discovery.order = Arc::new(sort_discovered_entry_indices(
            &discovery.entries,
            sort_options,
        ));
    }
    changed
}

fn upsert_discovered_entry(
    discovery: &mut DirectoryDiscovery,
    entry: &DiscoveredDirectoryEntry,
    options: &file_core::ScanOptions,
) -> bool {
    if !options.includes_path(entry.path()) {
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
