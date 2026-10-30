use file_core::{
    sort_entries, DirectoryDiscovery, DirectoryDiscoveryBatch, DirectoryEntry, FileKind, SortField,
};
use iced::Task;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::entry_changes::replay_changes_queued_during_current_scan;
use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::{load_directory_command, load_expanded_directory_command};
use crate::model::{
    empty_directory_entry_snapshot, trash_location_path, BrowserPaneId, BrowserViewMode,
    DirectoryCollectionPhase, DirectoryEntrySnapshot, DirectoryExpansionLoadContext,
    DirectoryLoadRequest, DirectoryLoadingPlaceholder, DirectoryLoadingPlaceholderEntry,
    DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus,
    Message, NavigationMode,
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
            pane.tab().current_dir.as_path(),
            pane.directory_load_generation,
        ) {
            return Task::none();
        }
        Arc::make_mut(&mut pane.tab_mut().entries).extend(entries);
        pane.directory_loading_placeholder = None;
        // 原地扩展可能在共享指针内变更内容：主列表条目索引在此失效。
        self.entry_index = None;
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
        self.accept_authoritative_directory_entries(
            request,
            scan.path,
            Arc::new(scan.entries),
            None,
        )
    }

    fn accept_authoritative_directory_entries(
        &mut self,
        request: DirectoryLoadRequest,
        path: PathBuf,
        entries: Arc<Vec<DirectoryEntry>>,
        discovery: Option<DirectoryDiscovery>,
    ) -> Task<Message> {
        let sort_options = self.options.clone();
        let pane_id = request.pane_id;
        let is_active = pane_id == self.active_pane_id();
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return Task::none();
        };
        if !directory_load_request_matches_pane(
            &request,
            pane.tab().current_dir.as_path(),
            pane.directory_load_generation,
        ) {
            return Task::none();
        }

        pane.directory_loading_placeholder = None;
        let tab = pane.tab_mut();
        tab.current_dir = path;
        tab.entries = entries;
        tab.directory_discovery = discovery;
        // 扫描读到的是启动那一刻的磁盘，期间到达的变化排在队里，落地后补上。
        replay_changes_queued_during_current_scan(tab, request.generation, &sort_options);
        // 图标视图的选择修正在活动窗格要结合展开状态，非活动窗格只剩直接条目。
        if tab.view_mode == BrowserViewMode::Icons && !is_active {
            tab.retain_direct_entry_selection();
        }
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
        tab.directory_order_phase =
            directory_order_phase_after_collection(&sort_options, &tab.entries);
        self.entries_replaced();
        if is_active {
            let entries = Arc::clone(&self.tab().entries);
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

        if self.tab().view_mode == BrowserViewMode::Icons {
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
            self.clamp_pane_viewports(&[pane_id]),
            self.schedule_thumbnail_refresh_for_pane(pane_id),
            self.schedule_visible_list_directory_summaries_for_pane(pane_id),
            self.schedule_visible_directory_metadata(pane_id, None),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(pane_id),
            self.remeasure_active_file_drop_layout(),
        ])
    }

    pub(super) fn navigate_to(&mut self, path: PathBuf, mode: NavigationMode) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let cancel_address_editing = self.cancel_address_editing();
        let loading_placeholder = self.capture_directory_loading_placeholder();
        let tab = self.tab_mut();
        if mode == NavigationMode::RecordHistory && !tab.is_trash_view && path != tab.current_dir {
            let previous = tab.current_dir.clone();
            tab.back_stack.push(previous);
            tab.forward_stack.clear();
        }
        self.clear_icon_grid_expansion();
        // 拖拽栏链快照绑定换目录前的多栏链;拖拽中导航(spring 面包屑
        // 返回 / Alt+←)换目录后快照失效,清空让多栏渲染回到活链。
        if let Some(drag) = &mut self.file_drag {
            drag.column_directories_snapshot.clear();
        }
        let tab = self.tab_mut();
        tab.current_dir = path.clone();
        tab.is_trash_view = false;
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
        Task::batch([
            cancel_address_editing,
            load_directory_command(request, self.options.clone(), cancellation),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
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
        if mode == NavigationMode::RecordHistory && !tab.is_trash_view {
            let previous = tab.current_dir.clone();
            tab.back_stack.push(previous);
            tab.forward_stack.clear();
        }
        self.clear_icon_grid_expansion();
        let has_cached_entries = cached_entries.is_some();
        let trash_entries = cached_entries.unwrap_or_default();
        let displayed_entries = trash_entries
            .iter()
            .map(|trash_entry| trash_entry.entry.clone())
            .collect::<Vec<_>>()
            .into();
        let tab = self.tab_mut();
        tab.current_dir = trash_location_path();
        tab.is_trash_view = true;
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

    pub(super) fn reload_current(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_pane(self.active_pane_id());
        if self.tab().is_trash_view {
            self.invalidate_trash_snapshot_refresh();
        }
        self.reload_current_preserving_list_directory_summaries()
    }

    pub(in crate::app) fn reload_current_for_file_drop(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_pane(self.active_pane_id());
        if self.tab().is_trash_view {
            self.invalidate_trash_snapshot_refresh();
        }
        self.schedule_directory_reload(self.active_pane_id())
    }

    pub(super) fn reload_current_preserving_list_directory_summaries(&mut self) -> Task<Message> {
        self.clear_transient_interaction_state();
        self.schedule_directory_reload(self.active_pane_id())
    }

    /// 重新加载窗格活动标签的目录：回收站视图重拉快照，其余重扫目录并刷新
    /// 已展开的子目录。图标展开只属于活动窗格的交互上下文。
    fn schedule_directory_reload(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        let is_active = pane_id == self.active_pane_id();
        let has_trash_snapshot = self.trash_refresh.snapshot().is_some();
        let shows_trash = self
            .pane_by_id(pane_id)
            .is_some_and(|pane| pane.tab().is_trash_view);
        if is_active && shows_trash {
            self.clear_icon_grid_expansion();
        }
        let pane = self
            .pane_by_id_mut(pane_id)
            .expect("a reload targets an existing pane");
        pane.directory_loading_placeholder = None;
        let tab = pane.tab_mut();
        if shows_trash {
            tab.directory_collection_phase = if has_trash_snapshot {
                DirectoryCollectionPhase::Ready
            } else {
                DirectoryCollectionPhase::Discovering
            };
            tab.close_expanded_directories();
            pane.cancel_directory_load();
            self.retract_global_error(GlobalErrorSource::DirectoryLoad);
            self.prune_list_directory_summaries_to_live_roots();
            return self.request_trash_snapshot_refresh();
        }
        tab.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        let current_dir = tab.current_dir.clone();
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);

        let mut commands = self.refresh_expanded_directory_commands(pane_id);
        if is_active {
            commands.extend(self.refresh_icon_grid_expansion_commands());
        }
        let (request, cancellation) = self.begin_directory_load(pane_id, current_dir);
        commands.push(load_directory_command(
            request,
            self.options.clone(),
            cancellation,
        ));
        Task::batch(commands)
    }

    pub(super) fn reload_visible_panes(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_visible_panes();
        self.reload_visible_panes_preserving_list_directory_summaries()
    }

    pub(super) fn reload_visible_panes_after_file_operation(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_visible_panes();
        self.reload_visible_panes_after_file_operation_preserving_list_directory_summaries()
    }

    pub(super) fn reload_visible_panes_after_file_operation_preserving_list_directory_summaries(
        &mut self,
    ) -> Task<Message> {
        let mut commands = vec![self.refresh_trash_snapshot_for_visible_panes()];
        commands.extend(
            self.pane_layout
                .visible_pane_ids()
                .into_iter()
                .map(|pane_id| self.schedule_directory_reload(pane_id)),
        );
        Task::batch(commands)
    }

    pub(super) fn reload_visible_panes_preserving_list_directory_summaries(
        &mut self,
    ) -> Task<Message> {
        self.clear_transient_interaction_state();
        Task::batch(
            self.pane_layout
                .visible_pane_ids()
                .into_iter()
                .map(|pane_id| self.schedule_directory_reload(pane_id))
                .collect::<Vec<_>>(),
        )
    }

    pub(super) fn reload_observed_directory(
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
            return self.refresh_trash_snapshot_for_trash_tabs();
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

    fn reload_observed_directory_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        path: &Path,
    ) -> Task<Message> {
        let Some(tab) = self.pane_by_id(pane_id).map(|pane| pane.tab()) else {
            return Task::none();
        };
        if tab.is_trash_view {
            return Task::none();
        }
        if tab.current_dir == path {
            return if pane_id == self.active_pane_id() {
                self.reload_current_preserving_list_directory_summaries()
            } else {
                self.schedule_directory_reload(pane_id)
            };
        }
        let load_context = self.expanded_directory_load_context(pane_id, path);
        let options = self.options.clone();
        let Some(expanded) = self
            .pane_by_id_mut(pane_id)
            .and_then(|pane| pane.tab_mut().expanded_directories.get_mut(path))
        else {
            // 这个窗格不显示该目录。
            return Task::none();
        };
        expanded.status = ExpandedDirectoryStatus::Loading;
        expanded.is_expanded = true;
        expanded.is_collapsing = false;
        expanded.animation_progress = 1.0;
        let (request, cancellation) =
            Self::next_expanded_directory_load_request(load_context, path.to_path_buf(), expanded);
        load_expanded_directory_command(request, options, cancellation)
    }

    pub(super) fn navigate_up(&mut self) -> Task<Message> {
        if self.tab().is_trash_view {
            return Task::none();
        }

        if let Some(parent) = self.tab().current_dir.parent() {
            self.navigate_to(parent.to_path_buf(), NavigationMode::RecordHistory)
        } else {
            Task::none()
        }
    }

    pub(super) fn navigate_back(&mut self) -> Task<Message> {
        let tab = self.tab_mut();
        let Some(path) = tab.back_stack.pop() else {
            return Task::none();
        };
        if !tab.is_trash_view {
            let leaving = tab.current_dir.clone();
            tab.forward_stack.push(leaving);
        }
        self.navigate_to(path, NavigationMode::KeepHistory)
    }

    pub(super) fn navigate_forward(&mut self) -> Task<Message> {
        let tab = self.tab_mut();
        let Some(path) = tab.forward_stack.pop() else {
            return Task::none();
        };
        let leaving = tab.current_dir.clone();
        tab.back_stack.push(leaving);
        self.navigate_to(path, NavigationMode::KeepHistory)
    }

    // 活动标签主列表条目的写入边界：整体替换经这里，使 hover 热路径的
    // 条目索引随之失效（内存/正确性不变量）。
    pub(super) fn set_entries(&mut self, entries: DirectoryEntrySnapshot) {
        self.tab_mut().entries = entries;
        self.entries_replaced();
    }

    // 主列表条目被整体换掉（或换了一个活动标签/窗格）之后的善后。
    pub(super) fn entries_replaced(&mut self) {
        self.entry_index = None;
        // 分组索引栏随条目分组出现或消失，消失的栏发不出 exit，屏障必须在
        // 条目变化时就地作废，否则栏再出现前滚动补偿一直被它挡住。
        self.hovered_grouping_rail_pane = None;
        // 拖拽会话存续时条目集变化意味着落点布局快照内容级过期(不同于
        // 滚动的坐标级软失效);由 spring tick 消化重测。
        self.file_drop_layout_stale |= self.file_drop_session.is_some();
    }

    // hover 热路径的 O(1) 查找前提：按需惰性重建索引（导航级一次 O(n)）。
    pub(super) fn refresh_entry_index(&mut self) {
        let entries = Arc::clone(&self.tab().entries);
        let already_valid = self
            .entry_index
            .as_ref()
            .is_some_and(|(indexed, _)| Arc::ptr_eq(indexed, &entries));
        if already_valid {
            return;
        }
        let index = entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.path.clone(), position))
            .collect();
        self.entry_index = Some((entries, index));
    }

    pub(crate) fn entry_for_path(&self, path: &Path) -> Option<&DirectoryEntry> {
        let tab = self.tab();
        if let Some((snapshot, index)) = &self.entry_index {
            if Arc::ptr_eq(snapshot, &tab.entries) {
                if let Some(position) = index.get(path) {
                    if let Some(entry) = tab.entries.get(*position) {
                        return Some(entry);
                    }
                }
            }
        }
        // 索引过期或未命中主列表时退回线性查找，保证语义不变。
        tab.entries
            .iter()
            .find(|entry| entry.path == path)
            .or_else(|| match tab.view_mode {
                BrowserViewMode::Icons => self
                    .icon_grid_expansion
                    .as_ref()
                    .and_then(|state| state.entry(path)),
                BrowserViewMode::List | BrowserViewMode::Columns => tab
                    .expanded_directories
                    .values()
                    .flat_map(|expanded| expanded.entries.iter())
                    .find(|entry| entry.path == path),
            })
    }

    pub(super) fn entry_kind_recursive(&self, path: &Path) -> Option<FileKind> {
        self.entry_for_path(path).map(|entry| entry.kind)
    }

    fn capture_directory_loading_placeholder(&self) -> Option<DirectoryLoadingPlaceholder> {
        let tab = self.tab();
        if tab.view_mode != BrowserViewMode::List || tab.entries.is_empty() {
            return None;
        }

        let row_height =
            crate::list_view::ListGeometry::for_level(self.user_config.list_view_density)
                .row_height;
        let range = self
            .pane()
            .column_viewports
            .get(&tab.current_dir)
            .map(|viewport| {
                crate::visible_entries::list_entry_range_for_viewport(
                    &tab.entries,
                    &tab.expanded_directories,
                    row_height,
                    crate::list_view::LIST_HEADER_HEIGHT,
                    viewport.offset_y,
                    viewport.height,
                    crate::list_view::LIST_OVERSCAN_ROWS,
                )
            })
            .unwrap_or_else(|| {
                crate::visible_entries::initial_list_entry_range(
                    &tab.entries,
                    &tab.expanded_directories,
                    row_height,
                    crate::list_view::list_initial_rows(
                        self.main_window_height,
                        self.user_config.list_view_density,
                    ),
                )
            });
        let entries = crate::visible_entries::visible_entries_in_range(
            &tab.entries,
            &tab.expanded_directories,
            range.start,
            range.end,
        )
        .into_iter()
        .enumerate()
        .map(
            |(range_offset, visible_entry)| DirectoryLoadingPlaceholderEntry {
                entry: visible_entry.entry.clone(),
                depth: visible_entry.depth,
                animation_progress: visible_entry.animation_progress,
                row_index: range.start + range_offset,
                trailing_status_height: tab
                    .expanded_directories
                    .get(&visible_entry.entry.path)
                    .map(|expanded| {
                        crate::visible_entries::visible_entry_status_row_height(
                            expanded, row_height,
                        )
                    })
                    .unwrap_or(0.0),
            },
        )
        .collect();

        Some(DirectoryLoadingPlaceholder {
            before_height: range.before_height,
            entries,
            after_height: range.after_height,
        })
    }

    /// 换位置时清掉依附于旧选择的交互状态（拖拽、悬停、菜单、重命名……）。
    /// 选择本身属于标签，由调用方在标签上清。
    fn clear_selection_context(&mut self) {
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
        self.pending_view_switch_reveal = None;
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

    /// 把窗格活动标签里已展开的目录全部置为加载中并发起重新加载。
    fn refresh_expanded_directory_commands(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Vec<Task<Message>> {
        let expanded_directories = &mut self
            .pane_by_id_mut(pane_id)
            .expect("a reload targets an existing pane")
            .tab_mut()
            .expanded_directories;
        let paths = expanded_directories.keys().cloned().collect::<Vec<_>>();
        for expanded in expanded_directories.values_mut() {
            expanded.status = ExpandedDirectoryStatus::Loading;
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = 1.0;
        }

        paths
            .into_iter()
            .filter_map(|path| {
                let load_context = self.expanded_directory_load_context(pane_id, &path);
                let expanded = self
                    .pane_by_id_mut(pane_id)?
                    .tab_mut()
                    .expanded_directories
                    .get_mut(&path)?;
                let (request, cancellation) =
                    Self::next_expanded_directory_load_request(load_context, path, expanded);
                Some(load_expanded_directory_command(
                    request,
                    self.options.clone(),
                    cancellation,
                ))
            })
            .collect()
    }
}

pub(super) fn directory_order_phase_after_collection(
    options: &file_core::ScanOptions,
    entries: &[DirectoryEntry],
) -> DirectoryOrderPhase {
    if matches!(options.sort_field, SortField::Size | SortField::Modified)
        && entries.iter().any(|entry| {
            entry.metadata.filesystem_availability
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
    current_dir: &Path,
    generation: u64,
) -> bool {
    request.generation == generation && request.path.as_path() == current_dir
}

#[cfg(test)]
mod tests {
    use file_core::{
        discover_directory_with_progress, DirectoryMetadataAvailability, EntryMetadata, ScanOptions,
    };

    use super::*;

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
        browser.tab_mut().current_dir = directory.path().to_path_buf();
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
            entry.metadata.filesystem_availability == DirectoryMetadataAvailability::Pending
                && entry.metadata.identity_names_availability
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
        tab.current_dir = current_directory.clone();
        tab.view_mode = BrowserViewMode::List;
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
        browser
            .pane_mut()
            .column_viewports
            .insert(current_directory.clone(), viewport);
        let previous_entries = Arc::clone(&browser.tab().entries);
        let expected_range = crate::visible_entries::list_entry_range_for_viewport(
            &previous_entries,
            &browser.tab().expanded_directories,
            crate::list_view::LIST_ROW_HEIGHT,
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
            placeholder.entries.first().map(|entry| entry.row_index),
            Some(expected_range.start)
        );
        assert_eq!(
            placeholder.entries.last().map(|entry| entry.row_index),
            Some(expected_range.end - 1)
        );
        assert_eq!(
            placeholder
                .entries
                .first()
                .map(|entry| entry.entry.path.as_path()),
            Some(previous_entries[expected_range.start].path.as_path())
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
    fn loading_placeholder_row_indices_count_only_entry_rows() {
        // Pending 加载占位流与正常行流同一条纹规则:只有条目行计数,
        // 展开状态行只贡献 trailing 高度 spacer、不占条纹序号,行序号
        // 与条目序一一对应。
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
        tab.view_mode = BrowserViewMode::List;
        tab.current_dir = abandoned.clone();
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
            project.path.clone(),
            ExpandedDirectory {
                entries: Vec::new(),
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Error,
                is_expanded: true,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: DirectoryOrderPhase::Ready {
                    field: SortField::Name,
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
                .map(|entry| entry.row_index)
                .collect::<Vec<_>>(),
            [0, 1],
            "状态行 spacer 不占条纹序号,f.txt 的行序号仍是 1"
        );
        assert!(placeholder.entries[0].trailing_status_height > 0.0);
    }
}
