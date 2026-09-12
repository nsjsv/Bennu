use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    DirectoryEntry, DirectoryMetadataAvailability, EntryMetadata, FileKind, ScanOptions,
    SortDirection, SortField,
};
use iced::Task;

use super::FileBrowser;
use crate::commands::load_list_directory_summary_command;
use crate::formatting::format_file_size;
use crate::model::{
    displayed_address_directory, BrowserPaneId, BrowserViewMode, ExpandedDirectory,
    ExpandedDirectoryStatus, ListDirectorySizeDisplayMode, ListDirectorySummaryCache,
    ListDirectorySummaryLoadRequest, Message,
};
use crate::operation_queue::QueuedFileOperation;
use crate::thumbnail_cache::ColumnViewport;

#[derive(Debug, Clone)]
struct RenderedListDirectory {
    path: std::path::PathBuf,
    loaded_child_count: Option<usize>,
}

impl FileBrowser {
    pub(super) fn toggle_list_directory_size_display_mode(&mut self) -> Task<Message> {
        self.user_config.list_directory_size_display_mode =
            self.user_config.list_directory_size_display_mode.toggled();
        self.resort_size_sorted_list_panes();
        Task::batch([
            self.persist_user_preferences_command(),
            self.schedule_visible_list_directory_summaries(),
        ])
    }

    pub(super) fn resort_size_sorted_list_panes(&mut self) {
        if self.options.sort_field != SortField::Size {
            return;
        }

        let display_mode = self.user_config.list_directory_size_display_mode;
        let sort_direction = self.options.sort_direction;
        let active_pane_id = self.active_pane_id();
        let mut sorted_active_pane = false;

        if self.view_mode == BrowserViewMode::List && !self.is_trash_view {
            resort_list_state_by_displayed_size(
                Arc::make_mut(&mut self.entries),
                &mut self.expanded_directories,
                &self.list_directory_summary_cache,
                display_mode,
                sort_direction,
            );
            self.sync_active_tab_state();
            sorted_active_pane = true;
        }

        for pane in self
            .panes
            .iter_mut()
            .filter(|pane| pane.id != active_pane_id)
        {
            if pane.view_mode != BrowserViewMode::List || pane.is_trash_view {
                continue;
            }
            resort_list_state_by_displayed_size(
                Arc::make_mut(&mut pane.entries),
                &mut pane.expanded_directories,
                &self.list_directory_summary_cache,
                display_mode,
                sort_direction,
            );
            pane.sync_active_tab_state();
        }

        if sorted_active_pane {
            self.sync_active_pane_state();
        }
    }

    pub(super) fn schedule_visible_list_directory_summaries(&mut self) -> Task<Message> {
        let commands = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .map(|pane_id| self.schedule_visible_list_directory_summaries_for_pane(pane_id))
            .collect::<Vec<_>>();
        Task::batch(commands)
    }

    pub(super) fn schedule_visible_list_directory_summaries_for_pane(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        self.schedule_visible_list_directory_summary_range_for_pane(pane_id, None)
    }

    pub(super) fn schedule_visible_list_directory_summary_range_for_pane(
        &mut self,
        pane_id: BrowserPaneId,
        viewport_override: Option<ColumnViewport>,
    ) -> Task<Message> {
        let include_recursive_total_size = self
            .user_config
            .list_directory_size_display_mode
            .uses_recursive_total_size();
        let mut commands = Vec::new();

        // 底部工具栏常显当前目录文件总大小：当前目录摘要与列表条目摘要共用
        // 本漏斗调度，导航/增量失效/文件操作后的重算自动覆盖，无需新触发点。
        // "当前目录"与地址栏同源：多栏模式取最深打开的栏，而非栏浏览根。
        // 递归量对底栏无意义，显式关闭。
        let displayed_dir = self.pane_view(pane_id).map(|pane| {
            displayed_address_directory(
                pane.current_dir,
                pane.view_mode,
                pane.deepest_open_column_directory,
            )
            .to_path_buf()
        });
        if let Some(displayed_dir) = displayed_dir {
            if let Some(request) = self
                .list_directory_summary_cache
                .start_request(displayed_dir, false)
            {
                commands.push(load_list_directory_summary_command(request));
            }
        }

        for directory in self.rendered_list_directories_for_pane(pane_id, viewport_override) {
            if let Some(loaded_child_count) = directory.loaded_child_count {
                self.list_directory_summary_cache
                    .remember_direct_child_count(directory.path.clone(), loaded_child_count);
            }
            if let Some(request) = self
                .list_directory_summary_cache
                .start_request(directory.path, include_recursive_total_size)
            {
                commands.push(load_list_directory_summary_command(request));
            }
        }

        if commands.is_empty() {
            Task::none()
        } else {
            Task::batch(commands)
        }
    }

    pub(super) fn accept_list_directory_summary(
        &mut self,
        request: ListDirectorySummaryLoadRequest,
        outcome: Result<crate::model::ListDirectorySummary, String>,
    ) -> Task<Message> {
        let changed = match outcome {
            Ok(summary) => self
                .list_directory_summary_cache
                .store_summary(&request, summary),
            Err(_) => self.list_directory_summary_cache.store_failure(&request),
        };
        if changed {
            self.resort_size_sorted_list_panes();
        }
        Task::none()
    }

    pub(super) fn remember_loaded_list_directory_children(
        &mut self,
        path: &Path,
        direct_child_count: usize,
    ) {
        self.list_directory_summary_cache
            .remember_direct_child_count(path.to_path_buf(), direct_child_count);
    }

    /// 底部工具栏常显统计：按窗格顺序，每窗格给出选中统计与当前目录
    /// 可见文件总大小。"当前目录"与地址栏同源（多栏取最深打开的栏）。
    /// 可见口径 = 文件系统事实按 show_hidden_files 推导，缓存条目保持不含配置。
    /// 隐藏条目数取原样事实，两种开关状态都原样呈现。
    pub(crate) fn pane_status_strip_entries(
        &self,
    ) -> Vec<crate::selection_summary::PaneStatusStripEntry> {
        self.panes
            .iter()
            .filter_map(|pane| {
                let pane_view = self.pane_view(pane.id)?;
                let displayed_dir = displayed_address_directory(
                    pane_view.current_dir,
                    pane_view.view_mode,
                    pane_view.deepest_open_column_directory,
                );
                let summary = self
                    .list_directory_summary_cache
                    .summary_for_path(displayed_dir);
                Some(crate::selection_summary::PaneStatusStripEntry {
                    selection: self.pane_selection_summary(pane_view),
                    visible_files_total_size_bytes: summary
                        .and_then(|summary| self.visible_files_total_size_for_summary(summary)),
                    hidden_entry_count: summary.and_then(|summary| summary.hidden_entry_count),
                })
            })
            .collect()
    }

    fn visible_files_total_size_for_summary(
        &self,
        summary: crate::model::ListDirectorySummary,
    ) -> Option<u64> {
        let files_total = summary.files_total_size_bytes?;
        if self.user_config.show_hidden_files {
            return Some(files_total);
        }
        Some(files_total.saturating_sub(summary.hidden_files_total_size_bytes.unwrap_or(0)))
    }

    pub(super) fn invalidate_list_directory_summary(&mut self, path: &Path) {
        self.list_directory_summary_cache.invalidate_path(path);
    }

    pub(super) fn invalidate_list_directory_summary_chain(&mut self, path: &Path) {
        for ancestor in path.ancestors() {
            self.invalidate_list_directory_summary(ancestor);
        }
    }

    pub(super) fn invalidate_list_directory_summary_subtree_and_ancestor_chain(
        &mut self,
        path: &Path,
    ) {
        self.list_directory_summary_cache
            .invalidate_path_subtree(path);
        if let Some(parent) = path.parent() {
            self.invalidate_list_directory_summary_chain(parent);
        }
    }

    // 摘要缓存按存活树裁剪：收集全部存活根（每个面板的当前目录、各标签目录、
    // 各级已展开目录），删除不在任何根之下的键。在导航级时机调用，
    // 保证缓存键集合不随浏览过的目录数量无限增长（内存上界机制）。
    pub(super) fn prune_list_directory_summaries_to_live_roots(&mut self) {
        let active_pane_id = self.active_pane_id();
        let mut roots: Vec<PathBuf> = Vec::new();
        for pane in &self.panes {
            let (current_dir, tabs, expanded_directories) = if pane.id == active_pane_id {
                (&self.current_dir, &self.tabs, &self.expanded_directories)
            } else {
                (&pane.current_dir, &pane.tabs, &pane.expanded_directories)
            };
            roots.push(current_dir.clone());
            roots.extend(tabs.iter().map(|tab| tab.directory.clone()));
            roots.extend(expanded_directories.keys().cloned());
        }
        self.list_directory_summary_cache.retain_roots(&roots);
    }

    pub(super) fn invalidate_list_directory_summaries_for_pane(&mut self, pane_id: BrowserPaneId) {
        for path in self.list_directory_paths_for_pane(pane_id) {
            self.invalidate_list_directory_summary(&path);
        }
    }

    pub(super) fn invalidate_list_directory_summaries_for_visible_panes(&mut self) {
        for pane_id in self.pane_layout.visible_pane_ids() {
            self.invalidate_list_directory_summaries_for_pane(pane_id);
        }
    }

    pub(super) fn invalidate_list_directory_summaries_for_file_operation(
        &mut self,
        operation: &QueuedFileOperation,
    ) {
        match operation {
            QueuedFileOperation::Rename { path, .. } => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(path);
            }
            QueuedFileOperation::BatchRename { items } => {
                for item in items {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&item.from);
                }
            }
            QueuedFileOperation::CreateDirectory { .. }
            | QueuedFileOperation::CreateEmptyFile { .. } => {
                if let Some(path) = operation.created_path() {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&path);
                }
            }
            QueuedFileOperation::Trash { paths }
            | QueuedFileOperation::DeletePermanently { paths } => {
                for path in paths {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(path);
                }
            }
            QueuedFileOperation::Restore { entries } => {
                for entry in entries {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &entry.original_path,
                    );
                }
            }
            QueuedFileOperation::Copy { transfers, .. } | QueuedFileOperation::Duplicate { transfers, .. } => {
                for transfer in transfers {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &transfer.target,
                    );
                }
            }
            QueuedFileOperation::Move { transfers, .. } => {
                for transfer in transfers {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &transfer.source,
                    );
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &transfer.target,
                    );
                }
            }
            QueuedFileOperation::CreateArchive { target, .. } => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(target);
            }
            QueuedFileOperation::ExtractArchive { request } => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                    &request.destination,
                );
            }
            QueuedFileOperation::Convert { requests } => {
                for request in requests {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &request.source,
                    );
                }
            }
            QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources } => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(directory);
                for source in sources {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(source);
                }
            }
            QueuedFileOperation::UngatherNewFolder {
                directory,
                restore_targets,
            } => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(directory);
                for restore_target in restore_targets {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        restore_target,
                    );
                }
            }
            QueuedFileOperation::CreateSymbolicLinks { links } => {
                for link in links {
                    self.invalidate_list_directory_summary_subtree_and_ancestor_chain(
                        &link.link_path,
                    );
                }
            }
            QueuedFileOperation::DeleteTrashEntries { .. } | QueuedFileOperation::EmptyTrash => {}
        }
    }

    pub(crate) fn list_directory_size_text(
        &self,
        entry: &DirectoryEntry,
        metadata: &EntryMetadata,
    ) -> String {
        if entry.kind != FileKind::Directory {
            if metadata.filesystem_availability != DirectoryMetadataAvailability::Complete {
                return "-".to_owned();
            }
            return format_file_size(metadata.len);
        }

        let Some(summary) = self
            .list_directory_summary_cache
            .summary_for_path(&entry.path)
        else {
            return "-".to_owned();
        };

        match self.user_config.list_directory_size_display_mode {
            ListDirectorySizeDisplayMode::ItemCount => {
                list_item_count_text(summary.direct_child_count)
            }
            ListDirectorySizeDisplayMode::RecursiveTotalSize => summary
                .recursive_total_size_bytes
                .map(format_file_size)
                .unwrap_or_else(|| "-".to_owned()),
        }
    }

    fn rendered_list_directories_for_pane(
        &self,
        pane_id: BrowserPaneId,
        viewport_override: Option<ColumnViewport>,
    ) -> Vec<RenderedListDirectory> {
        if !self.pane_layout.visible_pane_ids().contains(&pane_id) {
            return Vec::new();
        }

        let window_height = self.main_window_height;
        let Some(pane) = self.pane_view(pane_id) else {
            return Vec::new();
        };
        if pane.view_mode != BrowserViewMode::List
            || pane.is_trash_view
            || pane.directory_collection_phase.is_discovering()
        {
            return Vec::new();
        }

        let list_density = self.user_config.list_view_density;
        let row_height = crate::list_view::ListGeometry::for_level(list_density).row_height;
        let range = viewport_override
            .or_else(|| pane.column_viewports.get(pane.current_dir).copied())
            .map(|viewport| {
                crate::visible_entries::list_entry_range_for_viewport(
                    pane.entries,
                    pane.expanded_directories,
                    row_height,
                    crate::list_view::LIST_HEADER_HEIGHT,
                    viewport.offset_y,
                    viewport.height,
                    crate::list_view::LIST_OVERSCAN_ROWS,
                )
            })
            .unwrap_or_else(|| {
                crate::visible_entries::initial_list_entry_range(
                    pane.entries,
                    pane.expanded_directories,
                    row_height,
                    crate::list_view::list_initial_rows(window_height, list_density),
                )
            });

        crate::visible_entries::visible_entries_in_range(
            pane.entries,
            pane.expanded_directories,
            range.start,
            range.end,
        )
        .into_iter()
        .filter_map(|visible_entry| {
            (visible_entry.entry.kind == FileKind::Directory).then(|| RenderedListDirectory {
                path: visible_entry.entry.path.clone(),
                loaded_child_count: pane
                    .expanded_directories
                    .get(&visible_entry.entry.path)
                    .filter(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loaded))
                    .map(|expanded| expanded.entries.len()),
            })
        })
        .collect()
    }

    fn list_directory_paths_for_pane(&self, pane_id: BrowserPaneId) -> Vec<std::path::PathBuf> {
        let Some(pane) = self.pane_view(pane_id) else {
            return Vec::new();
        };
        if pane.view_mode != BrowserViewMode::List || pane.is_trash_view {
            return Vec::new();
        }

        let mut paths = Vec::new();
        collect_list_directory_paths(pane.entries, pane.expanded_directories, &mut paths);
        paths
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryDisplayedSizeValue {
    Known(u64),
    Unknown,
}

fn resort_list_state_by_displayed_size(
    entries: &mut Vec<DirectoryEntry>,
    expanded_directories: &mut HashMap<PathBuf, ExpandedDirectory>,
    summary_cache: &ListDirectorySummaryCache,
    display_mode: ListDirectorySizeDisplayMode,
    sort_direction: SortDirection,
) {
    let loaded_child_counts = loaded_child_counts_for_expanded_directories(expanded_directories);
    resort_entry_tree_by_displayed_size(
        entries,
        expanded_directories,
        &loaded_child_counts,
        summary_cache,
        display_mode,
        sort_direction,
    );
}

fn resort_entry_tree_by_displayed_size(
    entries: &mut Vec<DirectoryEntry>,
    expanded_directories: &mut HashMap<PathBuf, ExpandedDirectory>,
    loaded_child_counts: &HashMap<PathBuf, usize>,
    summary_cache: &ListDirectorySummaryCache,
    display_mode: ListDirectorySizeDisplayMode,
    sort_direction: SortDirection,
) {
    entries.sort_unstable_by(|left, right| {
        compare_entries_by_displayed_size(
            left,
            right,
            loaded_child_counts,
            summary_cache,
            display_mode,
            sort_direction,
        )
    });

    let expanded_paths = entries
        .iter()
        .filter(|entry| entry.kind == FileKind::Directory)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    for expanded_path in expanded_paths {
        let Some(mut expanded) = expanded_directories.remove(&expanded_path) else {
            continue;
        };
        resort_entry_tree_by_displayed_size(
            &mut expanded.entries,
            expanded_directories,
            loaded_child_counts,
            summary_cache,
            display_mode,
            sort_direction,
        );
        expanded_directories.insert(expanded_path, expanded);
    }
}

fn loaded_child_counts_for_expanded_directories(
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> HashMap<PathBuf, usize> {
    expanded_directories
        .iter()
        .filter(|(_, expanded)| matches!(expanded.status, ExpandedDirectoryStatus::Loaded))
        .map(|(path, expanded)| (path.clone(), expanded.entries.len()))
        .collect()
}

fn compare_entries_by_displayed_size(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    loaded_child_counts: &HashMap<PathBuf, usize>,
    summary_cache: &ListDirectorySummaryCache,
    display_mode: ListDirectorySizeDisplayMode,
    sort_direction: SortDirection,
) -> Ordering {
    match (
        left.kind == FileKind::Directory,
        right.kind == FileKind::Directory,
    ) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => compare_file_sizes(left, right, sort_direction),
        (true, true) => compare_directory_displayed_sizes(
            left,
            right,
            loaded_child_counts,
            summary_cache,
            display_mode,
            sort_direction,
        ),
    }
}

fn compare_file_sizes(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    sort_direction: SortDirection,
) -> Ordering {
    apply_sort_direction(
        left.metadata
            .len
            .cmp(&right.metadata.len)
            .then_with(|| compare_entry_names(left, right)),
        sort_direction,
    )
}

fn compare_directory_displayed_sizes(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    loaded_child_counts: &HashMap<PathBuf, usize>,
    summary_cache: &ListDirectorySummaryCache,
    display_mode: ListDirectorySizeDisplayMode,
    sort_direction: SortDirection,
) -> Ordering {
    let left_value =
        directory_displayed_size_value(left, loaded_child_counts, summary_cache, display_mode);
    let right_value =
        directory_displayed_size_value(right, loaded_child_counts, summary_cache, display_mode);
    match (left_value, right_value) {
        (
            DirectoryDisplayedSizeValue::Known(left_value),
            DirectoryDisplayedSizeValue::Known(right_value),
        ) => apply_sort_direction(
            left_value
                .cmp(&right_value)
                .then_with(|| compare_entry_names(left, right)),
            sort_direction,
        ),
        (DirectoryDisplayedSizeValue::Known(_), DirectoryDisplayedSizeValue::Unknown) => {
            Ordering::Less
        }
        (DirectoryDisplayedSizeValue::Unknown, DirectoryDisplayedSizeValue::Known(_)) => {
            Ordering::Greater
        }
        (DirectoryDisplayedSizeValue::Unknown, DirectoryDisplayedSizeValue::Unknown) => {
            apply_sort_direction(compare_entry_names(left, right), sort_direction)
        }
    }
}

fn directory_displayed_size_value(
    entry: &DirectoryEntry,
    loaded_child_counts: &HashMap<PathBuf, usize>,
    summary_cache: &ListDirectorySummaryCache,
    display_mode: ListDirectorySizeDisplayMode,
) -> DirectoryDisplayedSizeValue {
    let summary = summary_cache.summary_for_path(&entry.path);
    match display_mode {
        ListDirectorySizeDisplayMode::ItemCount => summary
            .map(|summary| summary.direct_child_count as u64)
            .or_else(|| {
                loaded_child_counts
                    .get(&entry.path)
                    .copied()
                    .map(|count| count as u64)
            })
            .map(DirectoryDisplayedSizeValue::Known)
            .unwrap_or(DirectoryDisplayedSizeValue::Unknown),
        ListDirectorySizeDisplayMode::RecursiveTotalSize => summary
            .and_then(|summary| summary.recursive_total_size_bytes)
            .map(DirectoryDisplayedSizeValue::Known)
            .unwrap_or(DirectoryDisplayedSizeValue::Unknown),
    }
}

fn compare_entry_names(left: &DirectoryEntry, right: &DirectoryEntry) -> Ordering {
    file_core::compare_entries(
        left,
        right,
        &ScanOptions {
            include_hidden: true,
            sort_field: SortField::Name,
            sort_direction: SortDirection::Ascending,
            directories_first: false,
        },
    )
}

fn apply_sort_direction(ordering: Ordering, sort_direction: SortDirection) -> Ordering {
    match sort_direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn list_item_count_text(count: usize) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

fn collect_list_directory_paths(
    entries: &[DirectoryEntry],
    expanded_directories: &std::collections::HashMap<
        std::path::PathBuf,
        crate::model::ExpandedDirectory,
    >,
    paths: &mut Vec<std::path::PathBuf>,
) {
    for entry in entries {
        if entry.kind != FileKind::Directory {
            continue;
        }

        paths.push(entry.path.clone());
        if let Some(expanded) = expanded_directories.get(&entry.path) {
            collect_list_directory_paths(&expanded.entries, expanded_directories, paths);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{EntryMetadata, FileKind, SortDirection, SortField};

    use super::*;
    use crate::config;
    use crate::model::ListDirectorySummary;
    use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

    fn test_entry(path: PathBuf, kind: FileKind, len: u64) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            kind,
            EntryMetadata {
                len,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    fn remember_summary(
        browser: &mut FileBrowser,
        path: &std::path::Path,
        count: usize,
        size: u64,
    ) {
        browser
            .list_directory_summary_cache
            .remember_direct_child_count(path.to_path_buf(), count);
        let request = browser
            .list_directory_summary_cache
            .start_request(path.to_path_buf(), true)
            .expect("recursive request");
        assert!(browser.list_directory_summary_cache.store_summary(
            &request,
            ListDirectorySummary {
                direct_child_count: count,
                recursive_total_size_bytes: Some(size),
                files_total_size_bytes: Some(size),
                hidden_files_total_size_bytes: Some(0),
                hidden_entry_count: Some(0),
            }
        ));
    }

    #[test]
    fn toggling_list_directory_size_display_mode_updates_preference() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        assert_eq!(
            browser.user_config.list_directory_size_display_mode,
            ListDirectorySizeDisplayMode::ItemCount
        );

        drop(browser.toggle_list_directory_size_display_mode());

        assert_eq!(
            browser.user_config.list_directory_size_display_mode,
            ListDirectorySizeDisplayMode::RecursiveTotalSize
        );
    }

    #[test]
    fn status_strip_reports_deepest_open_column_in_columns_mode() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.view_mode = BrowserViewMode::Columns;
        let root = PathBuf::from("/workspace");
        let column = root.join("sub");
        browser.current_dir = root.clone();
        browser.deepest_open_column_directory = Some(column.clone());
        remember_summary(&mut browser, &root, 10, 1000);
        remember_summary(&mut browser, &column, 4, 256);

        let entries = browser.pane_status_strip_entries();

        assert_eq!(entries.len(), 1);
        // 多栏模式下"当前目录"与地址栏同源：最深打开的栏，而非栏浏览根。
        assert_eq!(entries[0].visible_files_total_size_bytes, Some(256));
    }

    #[test]
    fn size_sort_orders_directories_by_item_count() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = PathBuf::from("/workspace");
        let smaller = root.join("a");
        let larger = root.join("b");

        browser.current_dir = root;
        browser.view_mode = BrowserViewMode::List;
        browser.options.sort_field = SortField::Size;
        browser.options.sort_direction = SortDirection::Ascending;
        browser.entries = vec![
            test_entry(larger.clone(), FileKind::Directory, 0),
            test_entry(smaller.clone(), FileKind::Directory, 0),
            test_entry(PathBuf::from("/workspace/file.bin"), FileKind::File, 9),
        ]
        .into();
        remember_summary(&mut browser, &smaller, 1, 10);
        remember_summary(&mut browser, &larger, 3, 30);

        browser.resort_size_sorted_list_panes();

        assert_eq!(
            browser
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![smaller, larger, PathBuf::from("/workspace/file.bin")]
        );
    }

    #[test]
    fn invalidating_directory_summary_chain_clears_ancestors_only() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = std::path::PathBuf::from("/workspace");
        let project = root.join("project");
        let src = project.join("src");
        let unrelated = root.join("notes");

        for (path, count, size) in [
            (project.clone(), 2usize, 2048u64),
            (src.clone(), 3usize, 1024u64),
            (unrelated.clone(), 1usize, 512u64),
        ] {
            remember_summary(&mut browser, &path, count, size);
        }

        browser.invalidate_list_directory_summary_chain(&src);

        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&src)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&project)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&unrelated)
            .is_some());
    }

    #[test]
    fn delete_operation_invalidation_keeps_unrelated_directory_summaries() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = std::path::PathBuf::from("/workspace");
        let current_dir = root.join("project");
        let deleted_child = current_dir.join("todo.txt");
        let unrelated = root.join("archive");

        remember_summary(&mut browser, &root, 3, 4096);
        remember_summary(&mut browser, &current_dir, 2, 2048);
        remember_summary(&mut browser, &unrelated, 1, 512);

        browser.invalidate_list_directory_summaries_for_file_operation(
            &QueuedFileOperation::DeletePermanently {
                paths: vec![deleted_child],
            },
        );

        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&current_dir)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&root)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&unrelated)
            .is_some());
    }

    #[test]
    fn move_operation_invalidation_clears_source_and_target_chains_only() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root = std::path::PathBuf::from("/workspace");
        let source_parent = root.join("source");
        let moved_directory = source_parent.join("entry");
        let source_child = moved_directory.join("child");
        let target_dir = root.join("target");
        let unrelated = root.join("notes");

        remember_summary(&mut browser, &source_parent, 2, 2048);
        remember_summary(&mut browser, &moved_directory, 2, 1536);
        remember_summary(&mut browser, &source_child, 1, 512);
        remember_summary(&mut browser, &target_dir, 1, 1024);
        remember_summary(&mut browser, &unrelated, 4, 512);

        browser.invalidate_list_directory_summaries_for_file_operation(
            &QueuedFileOperation::Move {
                transfers: vec![QueuedTransfer::new(
                    moved_directory,
                    target_dir.join("entry"),
                )],
                verification: file_core::FileOperationVerification::default(),
            },
        );

        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&source_parent)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&source_child)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&target_dir)
            .is_none());
        assert!(browser
            .list_directory_summary_cache
            .summary_for_path(&unrelated)
            .is_some());
    }
}
