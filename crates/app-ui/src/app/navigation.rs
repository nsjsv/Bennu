use file_core::{
    sort_entries, DirectoryDiscovery, DirectoryDiscoveryBatch, DirectoryEntry, FileKind,
};
use iced::Task;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

use super::entry_changes::replay_changes_queued_during_current_scan;
use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::{load_directory_command, load_expanded_directory_command};
use crate::model::{
    empty_directory_entry_snapshot, pane_entry, BrowserLocation, BrowserPaneId, BrowserViewMode,
    DirectoryCollectionPhase, DirectoryEntrySnapshot, DirectoryExpansionLoadContext,
    DirectoryLoadRequest, DirectoryLoadingPlaceholder, DirectoryLoadingPlaceholderEntry,
    DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryLoadRequest, HistoryEntry,
    IconGridExpansionState, Message, NavigationMode,
};
use crate::startup_trace;
impl FileBrowser {
    /// 窗格开始一次新的目录加载，返回请求与它的取消令牌。
    pub(super) fn begin_directory_load(
        &mut self,
        pane_id: BrowserPaneId,
        path: PathBuf,
    ) -> (DirectoryLoadRequest, CancellationToken) {
        self.clear_directory_metadata_demands_for_pane(pane_id);
        self.pane_by_id_mut(pane_id)
            .expect("a directory load starts in an existing pane")
            .begin_directory_load(path)
    }

    #[cfg(test)]
    pub(super) fn next_directory_load_request(&mut self, path: PathBuf) -> DirectoryLoadRequest {
        self.begin_directory_load(self.active_pane_id(), path).0
    }

    pub(super) fn next_expanded_directory_load_request(
        context: DirectoryExpansionLoadContext,
        path: PathBuf,
        expanded: &mut ExpandedDirectory,
    ) -> (ExpandedDirectoryLoadRequest, CancellationToken) {
        if let Some(cancel) = expanded.load_cancel.take() {
            cancel.cancel();
        }
        expanded.load_generation = expanded.load_generation.wrapping_add(1);
        let cancellation = CancellationToken::new();
        expanded.load_context = Some(context.clone());
        expanded.load_cancel = Some(cancellation.clone());
        (
            ExpandedDirectoryLoadRequest {
                context,
                path,
                generation: expanded.load_generation,
            },
            cancellation,
        )
    }

    pub(super) fn cancel_expanded_directory_load(expanded: &mut ExpandedDirectory) {
        if let Some(cancel) = expanded.load_cancel.take() {
            cancel.cancel();
        }
        expanded.load_generation = expanded.load_generation.wrapping_add(1);
        expanded.load_context = None;
        expanded.status = expanded.status.scan_abandoned();
    }

    pub(super) fn accept_directory_discovery_batch(
        &mut self,
        request: DirectoryLoadRequest,
        batch: DirectoryDiscoveryBatch,
    ) -> Task<Message> {
        let mut entries = batch
            .entries
            .iter()
            .map(file_core::DiscoveredDirectoryEntry::display_entry)
            .collect::<Vec<_>>();
        sort_entries(&mut entries, &self.options);

        let Some(pane) = self.pane_by_id_mut(request.pane_id) else {
            return Task::none();
        };
        if !directory_load_request_matches_pane(
            &request,
            pane.tab().current_directory(),
            pane.directory_load_generation,
        ) {
            return Task::none();
        }
        // 分批只用来填从空开始的列表；刷新中的完整列表等结果落地整体替换，
        // 往上追加会让新旧条目重复出现、计数翻倍。
        if !pane.tab().directory_collection_phase.is_discovering() {
            return Task::none();
        }
        pane.tab_mut().entries.make_mut().extend(entries);
        pane.directory_loading_placeholder = None;
        Task::none()
    }

    pub(super) fn accept_directory_discovery(
        &mut self,
        request: DirectoryLoadRequest,
        prebuilt: crate::model::PrebuiltDirectoryDiscovery,
    ) -> Task<Message> {
        let path = prebuilt.discovery.path.clone();
        let entries = prebuilt.display_entries;
        self.accept_authoritative_directory_entries(
            request,
            path,
            entries,
            Some(prebuilt.discovery),
        )
    }

    #[cfg(test)]
    pub(super) fn accept_complete_directory_fixture(
        &mut self,
        request: DirectoryLoadRequest,
        scan: file_core::DirectoryScan,
    ) -> Task<Message> {
        self.accept_authoritative_directory_entries(request, scan.path, scan.entries.into(), None)
    }

    fn accept_authoritative_directory_entries(
        &mut self,
        request: DirectoryLoadRequest,
        path: PathBuf,
        entries: DirectoryEntrySnapshot,
        discovery: Option<DirectoryDiscovery>,
    ) -> Task<Message> {
        let pane_id = request.pane_id;
        let sort_options = self.listing_options(pane_id);
        let is_active = pane_id == self.active_pane_id();
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return Task::none();
        };
        if !directory_load_request_matches_pane(
            &request,
            pane.tab().current_directory(),
            pane.directory_load_generation,
        ) {
            return Task::none();
        }

        pane.directory_loading_placeholder = None;
        let tab = pane.tab_mut();
        tab.location = BrowserLocation::Directory(path.clone());
        tab.entries = entries.clone();
        tab.drop_selection_missing_from(&path);
        tab.directory_discovery = discovery;
        // 扫描读到的是启动那一刻的磁盘，期间到达的变化排在队里，落地后补上。
        replay_changes_queued_during_current_scan(tab, request.generation, &sort_options);
        // 图标视图的选择修正在活动窗格要结合展开状态，非活动窗格只剩直接条目。
        if tab.view_mode() == BrowserViewMode::Icons && !is_active {
            tab.retain_direct_entry_selection();
        }
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
        tab.directory_order_phase =
            directory_order_phase_after_collection(&sort_options, &tab.entries);
        let gone = self.interaction_targets_missing_from(&path, &entries);
        self.release_interactions_targeting(|target| gone.iter().any(|path| path == target));
        self.entries_replaced();
        if is_active {
            let entries = self.tab().entries.clone();
            self.reconcile_icon_grid_root_after_scan(&entries);
        }

        if !is_active {
            self.resort_size_sorted_list_panes();
            return Task::batch([
                self.clamp_pane_viewports(&[pane_id]),
                self.schedule_thumbnail_refresh_for_pane(pane_id),
                self.schedule_visible_list_directory_summaries_for_pane(pane_id),
                self.schedule_visible_directory_metadata(pane_id, None),
                self.reveal_address_bar_current_segment(pane_id),
            ]);
        }

        if self.tab().view_mode() == BrowserViewMode::Icons {
            self.retain_icon_grid_visible_selection();
        }
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);
        startup_trace::record_directory_collection_ready(self.tab().entries.len());
        if self.tab().directory_order_phase.is_ready() {
            startup_trace::mark_once("initial_directory_ready");
        }
        let command = self.focus_created_entry_for_rename();
        self.resort_size_sorted_list_panes();
        Task::batch([
            command,
            // 往上走时选中的子文件夹：列表到了才知道它在哪一行，这时滚到看得见。
            self.complete_pending_selection_reveal(&path),
            self.clamp_pane_viewports(&[pane_id]),
            self.schedule_thumbnail_refresh_for_pane(pane_id),
            self.schedule_visible_list_directory_summaries_for_pane(pane_id),
            self.schedule_visible_directory_metadata(pane_id, None),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(pane_id),
            self.remeasure_active_file_drop_layout(),
        ])
    }

    pub(crate) fn navigate_to(&mut self, path: PathBuf, mode: NavigationMode) -> Task<Message> {
        let came_out_of = self.came_out_of(&path);
        self.cancel_expansion_follow_plans();
        let cancel_address_editing = self.cancel_address_editing();
        let loading_placeholder = self.capture_directory_loading_placeholder();
        let tab = self.tab_mut();
        if mode == NavigationMode::RecordHistory {
            // 回收站不进历史：从回收站出来不记它。
            if let Some(previous) = tab
                .history_entry()
                .filter(|entry| !entry.is_directory(&path))
            {
                tab.back_stack.push(previous);
                tab.forward_stack.clear();
            }
        }
        self.forget_search_interaction();
        self.clear_icon_grid_expansion();
        // 拖拽栏链快照绑定换目录前的多栏链;拖拽中导航(spring 面包屑
        // 返回 / Alt+←)换目录后快照失效,清空让多栏渲染回到活链。
        if let Some(drag) = &mut self.file_drag {
            drag.column_directories_snapshot.clear();
        }
        let tab = self.tab_mut();
        tab.location = BrowserLocation::Directory(path.clone());
        tab.trash_entries.clear();
        tab.close_expanded_directories();
        tab.clear_selection();
        tab.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.set_entries(empty_directory_entry_snapshot());
        let pane = self.pane_mut();
        pane.reset_directory_view();
        pane.directory_loading_placeholder = loading_placeholder;
        self.clear_selection_context();
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);
        self.follow_terminal_panel_directory();
        let (request, cancellation) = self.begin_directory_load(self.active_pane_id(), path);
        // 目录已切换：按新存活树裁剪摘要缓存（内存上界机制）。
        self.prune_list_directory_summaries_to_live_roots();
        let navigated = Task::batch([
            cancel_address_editing,
            load_directory_command(
                request,
                self.listing_options(self.active_pane_id()),
                cancellation,
            ),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ]);
        match came_out_of {
            None => navigated,
            Some(CameOutOf::Columns { folder, selection }) => {
                Task::batch([navigated, self.reveal_in_columns(folder, selection)])
            }
            Some(CameOutOf::Folder(child)) => {
                self.select_path(child.clone());
                self.pending_selection_reveal = Some((self.active_pane_id(), child));
                navigated
            }
        }
    }

    /// 去的是现在所在文件夹的上级（上一级、后退、点面包屑、地址栏都算）时，刚从哪里出来。
    fn came_out_of(&self, destination: &Path) -> Option<CameOutOf> {
        let tab = self.tab();
        let current = tab.current_directory()?;
        if current == destination || !current.starts_with(destination) {
            return None;
        }
        // 刚出来的地方根本没列出来过（打的是文件、或者它已经没了）：没有栏可留，在上级里选中它。
        let was_shown = !tab.directory_collection_phase.is_discovering();
        Some(match tab.view_mode() {
            BrowserViewMode::Columns if was_shown => {
                let folder = tab
                    .deepest_open_column_directory
                    .clone()
                    .unwrap_or_else(|| current.to_path_buf());
                CameOutOf::Columns {
                    selection: tab.selected.clone().or_else(|| Some(folder.clone())),
                    folder,
                }
            }
            BrowserViewMode::Columns | BrowserViewMode::List | BrowserViewMode::Icons => {
                CameOutOf::Folder(
                    current
                        .ancestors()
                        .find(|ancestor| ancestor.parent() == Some(destination))?
                        .to_path_buf(),
                )
            }
        })
    }

    pub(super) fn open_trash_view(&mut self, mode: NavigationMode) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let cancel_address_editing = self.cancel_address_editing();
        let pane_id = self.active_pane_id();
        let cached_entries = self
            .trash_refresh
            .snapshot()
            .map(|snapshot| snapshot.entries.clone());
        let loading_placeholder = if cached_entries.is_some() {
            None
        } else {
            self.capture_directory_loading_placeholder()
        };
        let tab = self.tab_mut();
        if mode == NavigationMode::RecordHistory {
            if let Some(previous) = tab.history_entry() {
                tab.back_stack.push(previous);
                tab.forward_stack.clear();
            }
        }
        self.forget_search_interaction();
        self.clear_icon_grid_expansion();
        let has_cached_entries = cached_entries.is_some();
        let trash_entries = cached_entries.unwrap_or_default();
        let displayed_entries = trash_entries
            .iter()
            .map(|trash_entry| trash_entry.entry.clone())
            .collect::<Vec<_>>()
            .into();
        let tab = self.tab_mut();
        tab.location = BrowserLocation::Trash;
        tab.trash_entries = trash_entries;
        tab.directory_discovery = None;
        tab.close_expanded_directories();
        tab.clear_selection();
        tab.directory_collection_phase = if has_cached_entries {
            DirectoryCollectionPhase::Ready
        } else {
            DirectoryCollectionPhase::Discovering
        };
        self.set_entries(displayed_entries);
        let pane = self.pane_mut();
        pane.reset_directory_view();
        pane.directory_loading_placeholder = loading_placeholder;
        pane.cancel_directory_load();
        self.clear_selection_context();
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);
        self.follow_terminal_panel_directory();
        Task::batch([
            cancel_address_editing,
            self.request_trash_snapshot_refresh(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(pane_id),
        ])
    }

    /// 目录监视报来的变化：文件夹树和各窗格各按自己的规则用。
    pub(super) fn reload_observed_directory(
        &mut self,
        change: file_core::DirectoryEntryChanges,
    ) -> Task<Message> {
        let folder_tree = self.accept_folder_tree_changes(&change);
        Task::batch([folder_tree, self.reload_observed_directory_in_panes(change)])
    }

    fn reload_observed_directory_in_panes(
        &mut self,
        change: file_core::DirectoryEntryChanges,
    ) -> Task<Message> {
        let path = change.directory.clone();
        if self.has_trash_tab()
            && file_core::trash_bin::trash_watch_directories()
                .iter()
                .any(|watched| path.starts_with(watched))
        {
            // 批量回收站任务本身就是这些事件的来源,逐条全量重扫是平方级
            // 放大;挂起到任务终结后统一补刷一次。
            if self.operation_queue.has_running_trash_change_operation() {
                self.trash_batch_rescan_pending = true;
                return Task::none();
            }
            return self.refresh_trash_snapshot_after_change();
        }

        // 增量优先：结构化变更直接修正条目，列表不重置；RescanRequired 或
        // 无应用目标（事件丢失、批量溢出）时才退回全量重扫。
        if let Some(task) = self.apply_entry_changes(&path, &change.changes) {
            return task;
        }

        self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&path);
        let mut commands = Vec::new();
        if let Some(command) = self.reload_observed_icon_grid_directory(&path) {
            commands.push(command);
        }
        // 监听范围是所有可见窗格，重扫兜底也得覆盖它们，否则非活动窗格丢了
        // 事件就永远陈旧。
        for pane_id in self.pane_layout.visible_pane_ids() {
            commands.push(self.reload_observed_directory_in_pane(pane_id, &path));
        }
        Task::batch(commands)
    }

    pub(super) fn reload_observed_directory_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        path: &Path,
    ) -> Task<Message> {
        let Some(tab) = self.pane_by_id(pane_id).map(|pane| pane.tab()) else {
            return Task::none();
        };
        let Some(current_directory) = tab.current_directory() else {
            return Task::none();
        };
        if current_directory == path {
            // 磁盘上的变化不是用户操作：不收右键菜单、不清预览、不打断重命名。
            return self.reload_pane(pane_id);
        }
        let load_context = self.expanded_directory_load_context(pane_id, path);
        let options = self.listing_options(pane_id);
        let Some(expanded) = self
            .pane_by_id_mut(pane_id)
            .and_then(|pane| pane.tab_mut().expanded_directories.get_mut(path))
        else {
            // 这个窗格不显示该目录。
            return Task::none();
        };
        expanded.status = expanded.status.reloading();
        expanded.is_expanded = true;
        expanded.is_collapsing = false;
        expanded.animation_progress = 1.0;
        let (request, cancellation) =
            Self::next_expanded_directory_load_request(load_context, path.to_path_buf(), expanded);
        load_expanded_directory_command(request, options, cancellation)
    }

    pub(super) fn navigate_up(&mut self) -> Task<Message> {
        match self.tab().current_directory().and_then(Path::parent) {
            Some(parent) => self.navigate_to(parent.to_path_buf(), NavigationMode::RecordHistory),
            None => Task::none(),
        }
    }

    pub(super) fn navigate_back(&mut self) -> Task<Message> {
        let tab = self.tab_mut();
        let Some(entry) = tab.back_stack.pop() else {
            return Task::none();
        };
        // 回收站不进历史，从回收站后退时不把它记进前进栈。
        if let Some(leaving) = tab.history_entry() {
            tab.forward_stack.push(leaving);
        }
        self.go_to_history_entry(entry)
    }

    pub(super) fn navigate_forward(&mut self) -> Task<Message> {
        let tab = self.tab_mut();
        let Some(entry) = tab.forward_stack.pop() else {
            return Task::none();
        };
        if let Some(leaving) = tab.history_entry() {
            tab.back_stack.push(leaving);
        }
        self.go_to_history_entry(entry)
    }

    fn go_to_history_entry(&mut self, entry: HistoryEntry) -> Task<Message> {
        match entry {
            HistoryEntry::Directory(directory) => {
                self.navigate_to(directory, NavigationMode::KeepHistory)
            }
            HistoryEntry::Search(recall) => self.return_to_search(recall),
        }
    }

    // 活动标签主列表条目的写入边界：整体替换经这里。
    pub(super) fn set_entries(&mut self, entries: DirectoryEntrySnapshot) {
        self.tab_mut().entries = entries;
        self.entries_replaced();
    }

    // 主列表条目被整体换掉（或换了一个活动标签/窗格）之后的善后。
    pub(super) fn entries_replaced(&mut self) {
        // 分组索引栏随条目分组出现或消失，消失的栏发不出 exit，屏障必须在
        // 条目变化时就地作废，否则栏再出现前滚动补偿一直被它挡住。
        self.hovered_grouping_rail_pane = None;
        // 拖拽会话存续时条目集变化意味着落点布局快照内容级过期(不同于
        // 滚动的坐标级软失效);由 spring tick 消化重测。
        self.file_drop_layout_stale |= self.file_drop_session.is_some();
    }

    pub(crate) fn entry_for_path(&self, path: &Path) -> Option<&DirectoryEntry> {
        let tab = self.tab();
        pane_entry(
            &tab.entries,
            tab.view_mode(),
            &tab.expanded_directories,
            self.pane_icon_expansion(self.active_pane_id(), tab.current_directory()),
            path,
        )
    }

    /// 窗格的图标交互展开：展开的正是这个窗格的当前目录时才算它的。
    pub(crate) fn pane_icon_expansion(
        &self,
        pane_id: BrowserPaneId,
        current_directory: Option<&Path>,
    ) -> Option<&IconGridExpansionState> {
        self.icon_grid_expansion.as_ref().filter(|state| {
            state.context().pane_id == pane_id
                && current_directory == Some(state.context().current_dir.as_path())
        })
    }

    pub(super) fn entry_kind_recursive(&self, path: &Path) -> Option<FileKind> {
        self.entry_for_path(path).map(|entry| entry.kind())
    }

    fn capture_directory_loading_placeholder(&self) -> Option<DirectoryLoadingPlaceholder> {
        let tab = self.tab();
        if tab.view_mode() != BrowserViewMode::List || tab.entries.is_empty() {
            return None;
        }

        let row_height =
            crate::list_view::ListGeometry::for_level(self.user_config.list_view_density)
                .row_height;
        let pane = self.pane_view(self.active_pane_id())?;
        let layout = crate::transfer_placeholder_view::list_row_layout(self, pane, row_height);
        let range = pane
            .list_viewport
            .map(|viewport| {
                layout.range_for_viewport(
                    crate::list_view::LIST_HEADER_HEIGHT,
                    viewport.offset_y,
                    viewport.height,
                    crate::list_view::LIST_OVERSCAN_ROWS,
                )
            })
            .unwrap_or_else(|| {
                layout.initial_range(crate::list_view::list_initial_rows(
                    self.main_window_height,
                    self.user_config.list_view_density,
                ))
            });
        // 条纹照抄正在显示的那份行流，换目录时这几行的颜色不跳。
        let entries = (range.start..range.end)
            .filter_map(|row| match layout.row(pane, row)? {
                crate::transfer_placeholder_view::ListTransferRow::Entry {
                    visible,
                    stripe_index,
                    ..
                } => Some(DirectoryLoadingPlaceholderEntry {
                    entry: visible.entry.clone(),
                    depth: visible.depth,
                    animation_progress: visible.animation_progress,
                    stripe_index,
                    trailing_status_height: tab
                        .expanded_directories
                        .get(visible.entry.path())
                        .map(|expanded| {
                            crate::visible_entries::visible_entry_status_row_height(
                                expanded, row_height,
                            )
                        })
                        .unwrap_or(0.0),
                }),
                _ => None,
            })
            .collect();

        Some(DirectoryLoadingPlaceholder {
            before_height: range.before_height,
            entries,
            after_height: range.after_height,
        })
    }

    /// 换位置时清掉依附于旧选择的交互状态（拖拽、悬停、菜单、重命名……）。
    /// 选择本身属于标签，由调用方在标签上清。
    pub(super) fn clear_selection_context(&mut self) {
        self.drag_selection_anchor = None;
        self.column_resize_drag = None;
        // 不取消进行中的拖拽:spring 悬停导航是拖拽中导航的唯一来源,
        // 拖拽生命周期归拖拽协议(松手/Escape/离窗)管理;导航只负责
        // 清选择上下文,落点布局过期由 stale 标记驱动重测。
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.hovered_entry = None;
        self.hovered_sidebar = None;
        self.cursor_paste_directory = None;
        self.clear_column_interaction_context();
        self.last_activation_click = None;
        self.column_return_targets.clear();
        self.pending_keyboard_column_focus = None;
        self.pending_selection_reveal = None;
        self.preview_engine.clear_preview();
        self.context_menu = None;
        self.renaming = None;
        self.selection_marquee = None;
    }

    pub(super) fn clear_transient_interaction_state(&mut self) {
        self.drag_selection_anchor = None;
        self.column_resize_drag = None;
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.hovered_entry = None;
        self.hovered_sidebar = None;
        self.cursor_paste_directory = None;
        self.last_activation_click = None;
        self.pending_keyboard_column_focus = None;
        self.preview_engine.clear_preview();
        self.context_menu = None;
        self.renaming = None;
        self.selection_marquee = None;
    }
}

pub(super) fn directory_order_phase_after_collection(
    options: &file_core::ScanOptions,
    entries: &[DirectoryEntry],
) -> DirectoryOrderPhase {
    if options.sort_field.needs_filesystem_metadata()
        && entries.iter().any(|entry| {
            entry.metadata().filesystem_availability
                == file_core::DirectoryMetadataAvailability::Pending
        })
    {
        DirectoryOrderPhase::WaitingForMetadata {
            request_generation: 0,
            field: options.sort_field,
            direction: options.sort_direction,
        }
    } else {
        DirectoryOrderPhase::Ready {
            field: options.sort_field,
            direction: options.sort_direction,
        }
    }
}

fn directory_load_request_matches_pane(
    request: &DirectoryLoadRequest,
    current_directory: Option<&Path>,
    generation: u64,
) -> bool {
    request.generation == generation && current_directory == Some(request.path.as_path())
}

/// 往上级走时刚出来的地方：列表、图标里是途经的那个子文件夹，到了上级选中它；多栏里那串栏
/// 照旧开着、选中的照旧选中，只是左边多出上级的栏。
enum CameOutOf {
    Folder(PathBuf),
    Columns {
        folder: PathBuf,
        selection: Option<PathBuf>,
    },
}

#[cfg(test)]
#[path = "navigation_ancestor_tests.rs"]
mod ancestor_tests;

#[cfg(test)]
mod tests {
    use file_core::{
        discover_directory_with_progress, DirectoryMetadataAvailability, EntryMetadata, ScanOptions,
    };
    use std::sync::Arc;

    use super::*;
    use crate::model::ExpandedDirectoryStatus;

    #[tokio::test]
    async fn streamed_hints_are_replaced_by_the_authoritative_discovery() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..300 {
            std::fs::write(directory.path().join(format!("file-{index:03}.dat")), []).unwrap();
        }
        let mut batches = Vec::new();
        let discovery = discover_directory_with_progress(
            directory.path(),
            ScanOptions::default(),
            CancellationToken::new(),
            |batch| batches.push(batch),
        )
        .await
        .unwrap();
        let discovery_entries = Arc::clone(&discovery.entries);

        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(directory.path().to_path_buf());
        let request = browser.next_directory_load_request(directory.path().to_path_buf());
        for batch in batches {
            drop(browser.accept_directory_discovery_batch(request.clone(), batch));
        }

        // 流式批次直接落在标签自己的条目上，没有第二份可供「提交」的副本。
        assert_eq!(browser.tab().entries.len(), 300);

        drop(browser.accept_directory_discovery(
            request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        assert_eq!(browser.tab().entries.len(), 300);
        assert!(browser.tab().entries.iter().all(|entry| {
            entry.metadata().filesystem_availability == DirectoryMetadataAvailability::Pending
                && entry.metadata().identity_names_availability
                    == DirectoryMetadataAvailability::Pending
        }));
        let committed_discovery = browser
            .tab()
            .directory_discovery
            .as_ref()
            .expect("authoritative discovery");
        assert!(Arc::ptr_eq(
            &committed_discovery.entries,
            &discovery_entries
        ));
    }

    #[test]
    fn list_navigation_keeps_only_the_virtual_placeholder_range() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let current_directory = PathBuf::from("/workspace/large");
        let destination = PathBuf::from("/workspace");
        let tab = browser.tab_mut();
        tab.location = BrowserLocation::Directory(current_directory.clone());
        tab.choose_view_mode(BrowserViewMode::List);
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
        tab.entries = (0..5_000)
            .map(|index| {
                DirectoryEntry::new(
                    current_directory.join(format!("entry-{index:04}-{}", "界".repeat(64))),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )
            })
            .collect::<Vec<_>>()
            .into();

        let viewport = crate::thumbnail_cache::ColumnViewport {
            offset_y: crate::list_view::LIST_HEADER_HEIGHT
                + 2_500.0 * crate::list_view::LIST_ROW_HEIGHT,
            height: 10.0 * crate::list_view::LIST_ROW_HEIGHT,
        };
        browser.pane_mut().list_viewport = Some(viewport);
        let previous_entries = browser.tab().entries.clone();
        let expected_range = crate::transfer_placeholder_view::list_row_layout(
            &browser,
            browser.pane_view(browser.active_pane_id()).unwrap(),
            crate::list_view::LIST_ROW_HEIGHT,
        )
        .range_for_viewport(
            crate::list_view::LIST_HEADER_HEIGHT,
            viewport.offset_y,
            viewport.height,
            crate::list_view::LIST_OVERSCAN_ROWS,
        );

        drop(browser.navigate_to(destination, NavigationMode::RecordHistory));

        assert!(browser.tab().entries.is_empty());
        assert_eq!(browser.tab().entries.capacity(), 0);
        assert_eq!(previous_entries.len(), 5_000);
        let placeholder = browser
            .pane()
            .directory_loading_placeholder
            .as_ref()
            .expect("list navigation must retain visible placeholder rows");
        assert_eq!(
            placeholder.entries.len(),
            expected_range.end - expected_range.start
        );
        assert!(placeholder.entries.len() < previous_entries.len());
        assert_eq!(placeholder.before_height, expected_range.before_height);
        assert_eq!(placeholder.after_height, expected_range.after_height);
        assert_eq!(
            placeholder.entries.first().map(|entry| entry.stripe_index),
            Some(expected_range.start)
        );
        assert_eq!(
            placeholder.entries.last().map(|entry| entry.stripe_index),
            Some(expected_range.end - 1)
        );
        assert_eq!(
            placeholder.entries.first().map(|entry| entry.entry.path()),
            Some(previous_entries[expected_range.start].path())
        );
        let placeholder_height = placeholder.before_height
            + placeholder
                .entries
                .iter()
                .map(|entry| {
                    crate::list_view::LIST_ROW_HEIGHT * entry.animation_progress.clamp(0.0, 1.0)
                        + entry.trailing_status_height
                })
                .sum::<f32>()
            + placeholder.after_height;
        assert_eq!(
            placeholder_height,
            previous_entries.len() as f32 * crate::list_view::LIST_ROW_HEIGHT
        );
    }

    #[test]
    fn loading_placeholder_rows_keep_the_stripes_of_the_list() {
        // Pending 加载占位流照抄正常行流的条纹:展开状态行也占一个条纹序号,
        // 状态行本身在占位流里只是条目后的 trailing 高度 spacer。
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let abandoned = PathBuf::from("/workspace/leaving");
        let project = DirectoryEntry::new(
            abandoned.join("project"),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        );
        let tab = browser.tab_mut();
        tab.choose_view_mode(BrowserViewMode::List);
        tab.location = BrowserLocation::Directory(abandoned.clone());
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
        tab.entries = vec![
            project.clone(),
            DirectoryEntry::new(
                abandoned.join("f.txt"),
                FileKind::File,
                EntryMetadata::default(),
                false,
                false,
                false,
            ),
        ]
        .into();
        tab.expanded_directories.insert(
            project.path().to_path_buf(),
            ExpandedDirectory {
                entries: Default::default(),
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Error,
                is_expanded: true,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: DirectoryOrderPhase::Ready {
                    field: file_core::SortField::Name,
                    direction: file_core::SortDirection::Ascending,
                },
            },
        );

        drop(browser.navigate_to(PathBuf::from("/workspace"), NavigationMode::RecordHistory));

        let placeholder = browser
            .pane()
            .directory_loading_placeholder
            .as_ref()
            .expect("list navigation must retain visible placeholder rows");
        assert_eq!(placeholder.entries.len(), 2);
        assert_eq!(
            placeholder
                .entries
                .iter()
                .map(|entry| entry.stripe_index)
                .collect::<Vec<_>>(),
            [0, 2],
            "状态行占了 1 号条纹,f.txt 与正常列表一样是 2"
        );
        assert!(placeholder.entries[0].trailing_status_height > 0.0);
    }
}
