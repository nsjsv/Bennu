use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    sort_discovered_entry_indices, DirectoryDiscovery, DirectoryEntry, DiscoveredDirectoryEntry,
    ResolvedEntryChange,
};
use iced::Task;

use super::FileBrowser;
use crate::model::{
    display_entries_in_discovery_order, BrowserPaneId, ExpandedDirectoryStatus, Message,
};

/// 目录增量在单个面板内的应用位置：root 是该面板当前目录的主列表，
/// expanded 是该面板内已加载的展开目录（列视图父列、侧栏树、图标展开共用）。
#[derive(Clone, Copy)]
enum PaneDirectoryRole {
    Root,
    Expanded,
}

/// 目录增量的应用目标 = 哪个面板的哪层数据。增量管道的作用域是「所有
/// 可见面板各自显示的目录集合」，而不是「活动面板显示的目录集合」：
/// 跨面板移动的源/目标与 watcher 事件都可能只落在非活动面板上，漏掉
/// 任何一个目标都会让该面板停留在陈旧列表上。
#[derive(Clone, Copy)]
enum EntryChangeApplicationTarget {
    /// 活动面板由顶层镜像字段代表（工作副本）。
    ActiveMirror(PaneDirectoryRole),
    /// 非活动面板的状态独立持有，按 pane id 定位以便回写与重调度缩略图。
    Pane(BrowserPaneId, PaneDirectoryRole),
}

/// 记录需要重调度缩略图的面板；同一面板可能同时命中 root 与 expanded，
/// 只调度一次。
fn remember_pane_for_refresh(pane_ids: &mut Vec<BrowserPaneId>, pane_id: BrowserPaneId) {
    if !pane_ids.contains(&pane_id) {
        pane_ids.push(pane_id);
    }
}

/// 目录条目增量应用唯一入口：watcher 事件与文件操作事件都汇入这里。
///
/// 应用目标是「所有可见面板各自显示该目录的位置」：活动面板镜像（当前
/// 目录与展开目录）加上每个非活动面板的对应位置；同一目录可能同时命中
/// 多个目标（例如两个面板显示同一目录），全部应用。应用只修正受影响
/// 条目并重建显示顺序，不进入 Discovering 占位、不重置未变化条目。
///
/// 返回 `None` 表示没有任何应用目标成功应用（含 RescanRequired），调用方
/// 退回全量重扫兜底；个别目标失败（如非活动面板 discovery 尚未加载到
/// 可应用状态）不阻塞其它目标，该面板由权威扫描回写与对账兜底纠正。
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

        let active_pane_id = self.active_pane_id();
        let targets = self.entry_change_application_targets(directory);
        // Pane 目标要按 id 独占借用 self.panes，排序参数提前克隆一份避免
        // 借用冲突；该结构只有几个枚举字段，克隆成本可忽略。
        let sort_options = self.options.clone();
        let mut applied_any = false;
        // summaries 需要的直接子条目数必须来自刚应用成功的目标：显示该
        // 目录但 discovery 尚未加载的面板，其列表是陈旧的，不能作为计数来源。
        // 多目标同时成功时取首个：目标按「镜像 root → 镜像 expanded → 各
        // 面板」优先级枚举，与单面板时代「root 优先于 expanded」的取数
        // 优先级一致，不因面板遍历顺序漂移。
        let mut applied_child_count = None;
        let mut thumbnail_pane_ids: Vec<BrowserPaneId> = Vec::new();

        for target in targets {
            match target {
                EntryChangeApplicationTarget::ActiveMirror(PaneDirectoryRole::Root) => {
                    // Discovering/Loading 期间同样应用：权威扫描回写的是「扫描
                    // 启动时刻」的快照，可能覆盖刚应用的增量，但下一个防抖窗口
                    // （≤250ms）会把列表纠正回来；反之跳过会让大目录扫描期间
                    // 的事件永久丢失。
                    if let Some(discovery) = apply_changes_to_discovery(
                        self.directory_discovery.as_mut(),
                        changes,
                        &sort_options,
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
                        if applied_child_count.is_none() {
                            applied_child_count = Some(display.len());
                        }
                        self.directory_discovery = Some(discovery);
                        self.set_entries(display);
                        self.sync_active_tab_state();
                        applied_any = true;
                        remember_pane_for_refresh(&mut thumbnail_pane_ids, active_pane_id);
                    }
                }
                EntryChangeApplicationTarget::ActiveMirror(PaneDirectoryRole::Expanded) => {
                    if let Some(expanded) = self.expanded_directories.get_mut(directory) {
                        if let Some(discovery) = apply_changes_to_discovery(
                            expanded.directory_discovery.as_mut(),
                            changes,
                            &sort_options,
                        ) {
                            expanded.entries = display_entries_in_discovery_order(&discovery);
                            expanded.directory_discovery = Some(discovery);
                            if applied_child_count.is_none() {
                                applied_child_count = Some(expanded.entries.len());
                            }
                            applied_any = true;
                            remember_pane_for_refresh(&mut thumbnail_pane_ids, active_pane_id);
                        }
                    }
                }
                EntryChangeApplicationTarget::Pane(pane_id, PaneDirectoryRole::Root) => {
                    // 非活动面板与活动镜像共用同一 discovery 应用路径，增量
                    // 语义必须一致，否则两个面板对同一事件会得出不同列表。
                    let Some(pane) = self.pane_by_id_mut(pane_id) else {
                        continue;
                    };
                    if let Some(discovery) = apply_changes_to_discovery(
                        pane.directory_discovery.as_mut(),
                        changes,
                        &sort_options,
                    ) {
                        // 被移走的源路径不能继续留在该面板的选中集合或锚点上，
                        // 与活动镜像的选中清理语义保持一致。
                        for path in &removed_paths {
                            pane.selected_paths.remove(path);
                            if pane.selected.as_ref() == Some(path) {
                                pane.selected = None;
                            }
                            if pane.selection_anchor.as_ref() == Some(path) {
                                pane.selection_anchor = None;
                            }
                        }
                        let display = Arc::new(display_entries_in_discovery_order(&discovery));
                        if applied_child_count.is_none() {
                            applied_child_count = Some(display.len());
                        }
                        pane.directory_discovery = Some(discovery);
                        pane.entries = display;
                        // 活动标签页快照必须同步，否则切走再切回会回到陈旧列表。
                        pane.sync_active_tab_state();
                        applied_any = true;
                        remember_pane_for_refresh(&mut thumbnail_pane_ids, pane_id);
                    }
                }
                EntryChangeApplicationTarget::Pane(pane_id, PaneDirectoryRole::Expanded) => {
                    let Some(pane) = self.pane_by_id_mut(pane_id) else {
                        continue;
                    };
                    if let Some(expanded) = pane.expanded_directories.get_mut(directory) {
                        if let Some(discovery) = apply_changes_to_discovery(
                            expanded.directory_discovery.as_mut(),
                            changes,
                            &sort_options,
                        ) {
                            expanded.entries = display_entries_in_discovery_order(&discovery);
                            expanded.directory_discovery = Some(discovery);
                            if applied_child_count.is_none() {
                                applied_child_count = Some(expanded.entries.len());
                            }
                            applied_any = true;
                            remember_pane_for_refresh(&mut thumbnail_pane_ids, pane_id);
                        }
                    }
                }
            }
        }

        if !applied_any {
            return None;
        }

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
        let mut tasks = vec![self.schedule_visible_list_directory_summaries()];
        tasks.extend(
            thumbnail_pane_ids
                .drain(..)
                .map(|pane_id| self.schedule_thumbnail_refresh_for_pane(pane_id)),
        );
        Some(Task::batch(tasks))
    }

    /// 枚举显示 `directory` 的全部应用目标（根列表与已加载展开目录是独立
    /// 数据，可分别命中）。活动面板读顶层镜像；`self.panes` 中与活动面板
    /// 同 id 的条目是镜像的过期快照，必须跳过，否则会把增量写进即将被
    /// 镜像覆盖的旧数据。
    fn entry_change_application_targets(
        &self,
        directory: &Path,
    ) -> Vec<EntryChangeApplicationTarget> {
        let mut targets = Vec::new();
        // 回收站视图的列表来自回收站快照而非文件系统 discovery，不走增量。
        if !self.is_trash_view && directory == self.current_dir {
            targets.push(EntryChangeApplicationTarget::ActiveMirror(
                PaneDirectoryRole::Root,
            ));
        }
        if self.expanded_directories.contains_key(directory) {
            targets.push(EntryChangeApplicationTarget::ActiveMirror(
                PaneDirectoryRole::Expanded,
            ));
        }
        for pane in &self.panes {
            if pane.id == self.active_pane_id() {
                continue;
            }
            if !pane.is_trash_view && pane.current_dir == directory {
                targets.push(EntryChangeApplicationTarget::Pane(
                    pane.id,
                    PaneDirectoryRole::Root,
                ));
            }
            if pane.expanded_directories.contains_key(directory) {
                targets.push(EntryChangeApplicationTarget::Pane(
                    pane.id,
                    PaneDirectoryRole::Expanded,
                ));
            }
        }
        targets
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

    pub(super) fn find_discovered_entry(&self, path: &Path) -> Option<DiscoveredDirectoryEntry> {
        if let Some(discovery) = &self.directory_discovery {
            if let Some(entry) = discovery.entries.iter().find(|entry| entry.path() == path) {
                return Some(entry.clone());
            }
        }
        if let Some(entry) = self
            .expanded_directories
            .values()
            .filter_map(|expanded| expanded.directory_discovery.as_ref())
            .find_map(|discovery| {
                discovery
                    .entries
                    .iter()
                    .find(|entry| entry.path() == path)
                    .cloned()
            })
        {
            return Some(entry);
        }
        // 活动镜像查不到时继续查非活动面板：跨面板移动的源条目可能只被
        // 非活动面板显示，查不到会导致整批移动跳过增量、两个面板一起陈旧。
        let active_pane_id = self.active_pane_id();
        for pane in &self.panes {
            if pane.id == active_pane_id {
                continue;
            }
            if let Some(discovery) = &pane.directory_discovery {
                if let Some(entry) = discovery.entries.iter().find(|entry| entry.path() == path) {
                    return Some(entry.clone());
                }
            }
            if let Some(entry) = pane
                .expanded_directories
                .values()
                .filter_map(|expanded| expanded.directory_discovery.as_ref())
                .find_map(|discovery| {
                    discovery
                        .entries
                        .iter()
                        .find(|entry| entry.path() == path)
                        .cloned()
                })
            {
                return Some(entry);
            }
        }
        None
    }

    /// 枚举显示 `directory` 的所有可见面板的条目列表。活动面板读镜像
    /// （root 优先于 expanded，沿用单面板时代的语义），非活动面板读 pane
    /// 字段；活动面板在 `self.panes` 里的过期快照同样必须跳过。
    fn displayed_entry_lists_across_panes<'a>(
        &'a self,
        directory: &Path,
    ) -> Vec<&'a [DirectoryEntry]> {
        let mut lists = Vec::new();
        if directory == self.current_dir {
            lists.push(self.entries.as_slice());
        } else if let Some(expanded) = self.expanded_directories.get(directory) {
            if matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
                lists.push(expanded.entries.as_slice());
            }
        }
        for pane in &self.panes {
            if pane.id == self.active_pane_id() {
                continue;
            }
            if pane.current_dir.as_path() == directory {
                lists.push(pane.entries.as_slice());
            } else if let Some(expanded) = pane.expanded_directories.get(directory) {
                if matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
                    lists.push(expanded.entries.as_slice());
                }
            }
        }
        lists
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
                .all(|entries| !entries.iter().any(|entry| entry.path == path))
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
                .all(|entries| entries.iter().any(|entry| entry.path == path))
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
