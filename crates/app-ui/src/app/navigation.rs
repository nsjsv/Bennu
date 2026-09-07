use file_core::{
    sort_entries, DirectoryDiscovery, DirectoryDiscoveryBatch, DirectoryEntry, FileKind, SortField,
};
use iced::Task;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::commands::{
    delayed_thumbnail_refresh_command, load_directory_command, load_expanded_directory_command,
};
use crate::model::{
    empty_directory_entry_snapshot, trash_location_path, BrowserPaneId, BrowserViewMode,
    DirectoryCollectionPhase, DirectoryEntrySnapshot, DirectoryExpansionLoadContext,
    DirectoryLoadRequest, DirectoryLoadingPlaceholder, DirectoryLoadingPlaceholderEntry,
    DirectoryOrderPhase, ExpandedDirectory, ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus,
    Message, NavigationMode,
};
use crate::startup_trace;
impl FileBrowser {
    pub(super) fn next_directory_load_request(&mut self, path: PathBuf) -> DirectoryLoadRequest {
        if let Some(cancel) = self.directory_load_cancel.take() {
            cancel.cancel();
        }
        self.directory_load_generation = self.directory_load_generation.wrapping_add(1);
        self.clear_directory_metadata_demands_for_pane(self.active_pane_id());
        self.directory_discovery = None;
        self.directory_load_cancel = Some(CancellationToken::new());
        DirectoryLoadRequest {
            pane_id: self.active_pane_id(),
            path,
            generation: self.directory_load_generation,
        }
    }

    fn cancel_active_expanded_directory_loads(&mut self) {
        for expanded in self.expanded_directories.values_mut() {
            Self::cancel_expanded_directory_load(expanded);
        }
    }

    fn cancel_active_directory_load(&mut self) {
        if let Some(cancel) = self.directory_load_cancel.take() {
            cancel.cancel();
        }
        self.directory_load_generation = self.directory_load_generation.wrapping_add(1);
        self.directory_discovery = None;
    }

    pub(super) fn directory_load_cancellation(
        &self,
        request: &DirectoryLoadRequest,
    ) -> CancellationToken {
        if request.pane_id == self.active_pane_id() {
            return self
                .directory_load_cancel
                .clone()
                .unwrap_or_else(CancellationToken::new);
        }

        self.pane_by_id(request.pane_id)
            .and_then(|pane| pane.directory_load_cancel.clone())
            .unwrap_or_else(CancellationToken::new)
    }

    pub(super) fn next_inactive_directory_load_request(
        pane: &mut crate::model::BrowserPane,
        path: PathBuf,
    ) -> (DirectoryLoadRequest, CancellationToken) {
        if let Some(cancel) = pane.directory_load_cancel.take() {
            cancel.cancel();
        }
        pane.directory_load_generation = pane.directory_load_generation.wrapping_add(1);
        pane.directory_discovery = None;
        let cancellation = CancellationToken::new();
        pane.directory_load_cancel = Some(cancellation.clone());
        (
            DirectoryLoadRequest {
                pane_id: pane.id,
                path,
                generation: pane.directory_load_generation,
            },
            cancellation,
        )
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

        if request.pane_id != self.active_pane_id() {
            let Some(pane) = self.pane_by_id_mut(request.pane_id) else {
                return Task::none();
            };
            if !directory_load_request_matches_pane(
                &request,
                pane.current_dir.as_path(),
                pane.directory_load_generation,
            ) {
                return Task::none();
            }
            Arc::make_mut(&mut pane.entries).extend(entries);
            pane.directory_loading_placeholder = None;
            return Task::none();
        }

        if !directory_load_request_matches_pane(
            &request,
            self.current_dir.as_path(),
            self.directory_load_generation,
        ) {
            return Task::none();
        }
        Arc::make_mut(&mut self.entries).extend(entries);
        // 原地扩展可能在共享指针内变更内容：主列表条目索引在此失效。
        self.entry_index = None;
        self.directory_loading_placeholder = None;
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
        let order_phase = directory_order_phase_after_collection(&self.options, &entries);
        if request.pane_id != self.active_pane_id() {
            let current_dir = {
                let Some(pane) = self.pane_by_id_mut(request.pane_id) else {
                    return Task::none();
                };
                if !directory_load_request_matches_pane(
                    &request,
                    pane.current_dir.as_path(),
                    pane.directory_load_generation,
                ) {
                    return Task::none();
                }

                pane.current_dir = path;
                pane.entries = entries;
                pane.directory_discovery = discovery;
                if pane.view_mode == crate::model::BrowserViewMode::Icons {
                    crate::model::retain_direct_entry_selection(
                        &pane.entries,
                        &mut pane.selected,
                        &mut pane.selected_paths,
                        &mut pane.selection_anchor,
                    );
                }
                pane.directory_loading_placeholder = None;
                pane.directory_collection_phase = DirectoryCollectionPhase::Ready;
                pane.directory_order_phase = order_phase;
                pane.sync_active_tab_state();
                pane.current_dir.clone()
            };
            self.resort_size_sorted_list_panes();
            return Task::batch([
                delayed_thumbnail_refresh_command(request.pane_id, current_dir),
                self.schedule_visible_list_directory_summaries_for_pane(request.pane_id),
                self.schedule_visible_directory_metadata(request.pane_id, None),
                self.reveal_address_bar_current_segment(request.pane_id),
            ]);
        }

        if !directory_load_request_matches_pane(
            &request,
            self.current_dir.as_path(),
            self.directory_load_generation,
        ) {
            return Task::none();
        }

        self.reconcile_icon_grid_root_after_scan(&entries);
        self.current_dir = path;
        self.set_entries(entries);
        self.directory_discovery = discovery;
        if self.view_mode == crate::model::BrowserViewMode::Icons {
            self.retain_icon_grid_visible_selection();
        }
        self.directory_loading_placeholder = None;
        self.directory_collection_phase = DirectoryCollectionPhase::Ready;
        self.directory_order_phase = order_phase;
        self.clear_global_error();
        startup_trace::record_directory_collection_ready(self.entries.len());
        if self.directory_order_phase.is_ready() {
            startup_trace::mark_once("initial_directory_ready");
        }
        let command = self.focus_created_entry_for_rename();
        self.sync_active_tab_state();
        self.resort_size_sorted_list_panes();
        Task::batch([
            command,
            delayed_thumbnail_refresh_command(request.pane_id, self.current_dir.clone()),
            self.schedule_visible_list_directory_summaries_for_pane(request.pane_id),
            self.schedule_visible_directory_metadata(request.pane_id, None),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(request.pane_id),
            self.remeasure_active_file_drop_layout(),
        ])
    }

    pub(super) fn navigate_to(&mut self, path: PathBuf, mode: NavigationMode) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let cancel_address_editing = self.cancel_address_editing();
        let loading_placeholder = self.capture_directory_loading_placeholder();
        if mode == NavigationMode::RecordHistory && !self.is_trash_view && path != self.current_dir
        {
            self.back_stack.push(self.current_dir.clone());
            self.forward_stack.clear();
        }
        self.clear_icon_grid_expansion();
        self.current_dir = path.clone();
        self.is_trash_view = false;
        self.set_entries(empty_directory_entry_snapshot());
        self.directory_loading_placeholder = loading_placeholder;
        self.trash_entries.clear();
        self.deepest_open_column_directory = None;
        self.cancel_active_expanded_directory_loads();
        self.expanded_directories.clear();
        self.column_browser_viewport = Default::default();
        self.column_viewports.clear();
        self.clear_selection_context();
        self.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.clear_global_error();
        self.sync_active_tab_state();
        let request = self.next_directory_load_request(path);
        let cancellation = self.directory_load_cancellation(&request);
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
        if mode == NavigationMode::RecordHistory && !self.is_trash_view {
            self.back_stack.push(self.current_dir.clone());
            self.forward_stack.clear();
        }
        self.clear_icon_grid_expansion();
        self.current_dir = trash_location_path();
        self.is_trash_view = true;
        let has_cached_entries = cached_entries.is_some();
        self.trash_entries = cached_entries.unwrap_or_default();
        self.set_entries(
            self.trash_entries
                .iter()
                .map(|trash_entry| trash_entry.entry.clone())
                .collect::<Vec<_>>()
                .into(),
        );
        self.directory_discovery = None;
        self.directory_loading_placeholder = loading_placeholder;
        self.deepest_open_column_directory = None;
        self.cancel_active_expanded_directory_loads();
        self.expanded_directories.clear();
        self.column_browser_viewport = Default::default();
        self.column_viewports.clear();
        self.clear_selection_context();
        self.directory_collection_phase = if has_cached_entries {
            DirectoryCollectionPhase::Ready
        } else {
            DirectoryCollectionPhase::Discovering
        };
        self.clear_global_error();
        self.cancel_active_directory_load();
        self.sync_active_tab_state();
        Task::batch([
            cancel_address_editing,
            self.request_trash_snapshot_refresh(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(pane_id),
        ])
    }

    pub(super) fn reload_current(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_pane(self.active_pane_id());
        if self.is_trash_view {
            self.invalidate_trash_snapshot_refresh();
        }
        self.reload_current_preserving_list_directory_summaries()
    }

    pub(in crate::app) fn reload_current_for_file_drop(&mut self) -> Task<Message> {
        self.invalidate_list_directory_summaries_for_pane(self.active_pane_id());
        if self.is_trash_view {
            self.invalidate_trash_snapshot_refresh();
        }
        self.schedule_current_directory_reload_preserving_list_directory_summaries()
    }

    pub(super) fn reload_current_preserving_list_directory_summaries(&mut self) -> Task<Message> {
        self.clear_transient_interaction_state();
        self.schedule_current_directory_reload_preserving_list_directory_summaries()
    }

    fn schedule_current_directory_reload_preserving_list_directory_summaries(
        &mut self,
    ) -> Task<Message> {
        if self.is_trash_view {
            self.clear_icon_grid_expansion();
            self.directory_loading_placeholder = None;
            self.directory_collection_phase = if self.trash_refresh.snapshot().is_some() {
                DirectoryCollectionPhase::Ready
            } else {
                DirectoryCollectionPhase::Discovering
            };
            self.clear_global_error();
            self.cancel_active_directory_load();
            self.deepest_open_column_directory = None;
            self.cancel_active_expanded_directory_loads();
            self.expanded_directories.clear();
            self.prune_list_directory_summaries_to_live_roots();
            return self.request_trash_snapshot_refresh();
        }

        self.directory_loading_placeholder = None;
        self.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.clear_global_error();

        let mut commands = self.refresh_expanded_directory_commands();
        commands.extend(self.refresh_icon_grid_expansion_commands());
        let request = self.next_directory_load_request(self.current_dir.clone());
        let cancellation = self.directory_load_cancellation(&request);
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
        let active_pane_id = self.active_pane_id();
        let mut commands = vec![
            self.refresh_trash_snapshot_for_visible_panes(),
            self.schedule_current_directory_reload_preserving_list_directory_summaries(),
        ];
        self.sync_active_pane_state();

        for pane_id in self.pane_layout.visible_pane_ids() {
            if pane_id != active_pane_id {
                commands
                    .push(self.reload_inactive_pane_preserving_list_directory_summaries(pane_id));
            }
        }

        Task::batch(commands)
    }

    pub(super) fn reload_visible_panes_preserving_list_directory_summaries(
        &mut self,
    ) -> Task<Message> {
        let active_pane_id = self.active_pane_id();
        let mut commands = vec![self.reload_current_preserving_list_directory_summaries()];
        self.sync_active_pane_state();

        for pane_id in self.pane_layout.visible_pane_ids() {
            if pane_id != active_pane_id {
                commands
                    .push(self.reload_inactive_pane_preserving_list_directory_summaries(pane_id));
            }
        }

        Task::batch(commands)
    }

    fn reload_inactive_pane_preserving_list_directory_summaries(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        let options = self.options.clone();
        let has_trash_snapshot = self.trash_refresh.snapshot().is_some();
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return Task::none();
        };

        pane.directory_loading_placeholder = None;
        pane.directory_collection_phase = if pane.is_trash_view && has_trash_snapshot {
            DirectoryCollectionPhase::Ready
        } else {
            DirectoryCollectionPhase::Discovering
        };

        if pane.is_trash_view {
            pane.deepest_open_column_directory = None;
            for expanded in pane.expanded_directories.values_mut() {
                Self::cancel_expanded_directory_load(expanded);
            }
            pane.expanded_directories.clear();
            pane.sync_active_tab_state();
            self.prune_list_directory_summaries_to_live_roots();
            return self.request_trash_snapshot_refresh();
        }

        let current_dir = pane.current_dir.clone();
        let expanded_paths = pane
            .expanded_directories
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for path in &expanded_paths {
            if let Some(expanded) = pane.expanded_directories.get_mut(path) {
                expanded.status = ExpandedDirectoryStatus::Loading;
                expanded.is_expanded = true;
                expanded.is_collapsing = false;
                expanded.animation_progress = 1.0;
            }
        }
        pane.sync_active_tab_state();

        let mut commands = Vec::new();
        for path in expanded_paths {
            let Some(expanded) = pane.expanded_directories.get_mut(&path) else {
                continue;
            };
            let (request, cancellation) = Self::next_expanded_directory_load_request(
                DirectoryExpansionLoadContext::BrowserTree { pane_id },
                path,
                expanded,
            );
            commands.push(load_expanded_directory_command(
                request,
                options.clone(),
                cancellation,
            ));
        }
        let (request, cancellation) = Self::next_inactive_directory_load_request(pane, current_dir);
        commands.push(load_directory_command(request, options, cancellation));
        Task::batch(commands)
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
            return self.refresh_trash_snapshot_for_trash_tabs();
        }

        if self.is_trash_view {
            return Task::none();
        }

        // 增量优先：结构化变更直接修正条目，列表不重置；RescanRequired 或
        // 无应用目标（事件丢失、批量溢出）时才退回全量重扫。
        if let Some(task) = self.apply_entry_changes(&path, &change.changes) {
            return task;
        }

        if path == self.current_dir {
            self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&path);
            return self.reload_current_preserving_list_directory_summaries();
        }

        let mut commands = Vec::with_capacity(2);
        if let Some(command) = self.reload_observed_icon_grid_directory(&path) {
            commands.push(command);
        }

        self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&path);

        let pane_id = self.active_pane_id();
        let load_context = self.expanded_directory_load_context(pane_id, &path);
        if let Some(expanded) = self.expanded_directories.get_mut(&path) {
            expanded.status = ExpandedDirectoryStatus::Loading;
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = 1.0;
            let (request, cancellation) =
                Self::next_expanded_directory_load_request(load_context, path, expanded);
            commands.push(load_expanded_directory_command(
                request,
                self.options.clone(),
                cancellation,
            ));
        }
        Task::batch(commands)
    }

    pub(super) fn navigate_up(&mut self) -> Task<Message> {
        if self.is_trash_view {
            return Task::none();
        }

        if let Some(parent) = self.current_dir.parent() {
            self.navigate_to(parent.to_path_buf(), NavigationMode::RecordHistory)
        } else {
            Task::none()
        }
    }

    pub(super) fn navigate_back(&mut self) -> Task<Message> {
        if self.is_trash_view {
            if let Some(path) = self.back_stack.pop() {
                return self.navigate_to(path, NavigationMode::KeepHistory);
            }
            return Task::none();
        }

        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.current_dir.clone());
            self.navigate_to(path, NavigationMode::KeepHistory)
        } else {
            Task::none()
        }
    }

    pub(super) fn navigate_forward(&mut self) -> Task<Message> {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.current_dir.clone());
            self.navigate_to(path, NavigationMode::KeepHistory)
        } else {
            Task::none()
        }
    }

    // 主列表条目统一写入边界：任何整体替换都必须经过这里，
    // 使 hover 热路径的条目索引随之失效（内存/正确性不变量）。
    pub(super) fn set_entries(&mut self, entries: DirectoryEntrySnapshot) {
        self.entries = entries;
        self.entry_index = None;
    }

    // hover 热路径的 O(1) 查找前提：按需惰性重建索引（导航级一次 O(n)）。
    pub(super) fn refresh_entry_index(&mut self) {
        let already_valid = self
            .entry_index
            .as_ref()
            .is_some_and(|(entries, _)| std::sync::Arc::ptr_eq(entries, &self.entries));
        if already_valid {
            return;
        }
        let index = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.path.clone(), position))
            .collect();
        self.entry_index = Some((std::sync::Arc::clone(&self.entries), index));
    }

    pub(crate) fn entry_for_path(&self, path: &Path) -> Option<&DirectoryEntry> {
        if let Some((snapshot, index)) = &self.entry_index {
            if std::sync::Arc::ptr_eq(snapshot, &self.entries) {
                if let Some(position) = index.get(path) {
                    if let Some(entry) = self.entries.get(*position) {
                        return Some(entry);
                    }
                }
            }
        }
        // 索引过期或未命中主列表时退回线性查找，保证语义不变。
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .or_else(|| match self.view_mode {
                BrowserViewMode::Icons => self
                    .icon_grid_expansion
                    .as_ref()
                    .and_then(|state| state.entry(path)),
                BrowserViewMode::List | BrowserViewMode::Columns => self
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
        if self.view_mode != BrowserViewMode::List || self.entries.is_empty() {
            return None;
        }

        let row_height =
            crate::list_view::ListGeometry::for_level(self.user_config.list_view_density)
                .row_height;
        let range = self
            .column_viewports
            .get(&self.current_dir)
            .map(|viewport| {
                crate::visible_entries::list_entry_range_for_viewport(
                    &self.entries,
                    &self.expanded_directories,
                    row_height,
                    crate::list_view::LIST_HEADER_HEIGHT,
                    viewport.offset_y,
                    viewport.height,
                    crate::list_view::LIST_OVERSCAN_ROWS,
                )
            })
            .unwrap_or_else(|| {
                crate::visible_entries::initial_list_entry_range(
                    &self.entries,
                    &self.expanded_directories,
                    row_height,
                    crate::list_view::list_initial_rows(
                        self.main_window_height,
                        self.user_config.list_view_density,
                    ),
                )
            });
        let entries = crate::visible_entries::visible_entries_in_range(
            &self.entries,
            &self.expanded_directories,
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
                trailing_status_height: self
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

    fn clear_selection_context(&mut self) {
        self.selected = None;
        self.selected_paths.clear();
        self.selection_anchor = None;
        self.drag_selection_anchor = None;
        self.column_resize_drag = None;
        self.cancel_file_drag_interaction();
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
        self.clear_preview();
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
        self.clear_preview();
        self.context_menu = None;
        self.renaming = None;
        self.selection_marquee = None;
    }

    fn refresh_expanded_directory_commands(&mut self) -> Vec<Task<Message>> {
        let paths = self
            .expanded_directories
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for path in &paths {
            if let Some(expanded) = self.expanded_directories.get_mut(path) {
                expanded.status = ExpandedDirectoryStatus::Loading;
                expanded.is_expanded = true;
                expanded.is_collapsing = false;
                expanded.animation_progress = 1.0;
            }
        }

        let pane_id = self.active_pane_id();
        paths
            .into_iter()
            .filter_map(|path| {
                let load_context = self.expanded_directory_load_context(pane_id, &path);
                let expanded = self.expanded_directories.get_mut(&path)?;
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
    async fn hints_stay_ephemeral_until_authoritative_discovery_is_committed() {
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
        browser.current_dir = directory.path().to_path_buf();
        let request = browser.next_directory_load_request(directory.path().to_path_buf());
        for batch in batches {
            drop(browser.accept_directory_discovery_batch(request.clone(), batch));
        }

        assert_eq!(browser.entries.len(), 300);
        let active_tab = browser
            .tabs
            .iter()
            .find(|tab| tab.id == browser.active_tab_id)
            .expect("active tab");
        assert!(active_tab.entries.is_empty());

        drop(browser.accept_directory_discovery(
            request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        assert_eq!(browser.entries.len(), 300);
        assert!(browser.entries.iter().all(|entry| {
            entry.metadata.filesystem_availability == DirectoryMetadataAvailability::Pending
                && entry.metadata.identity_names_availability
                    == DirectoryMetadataAvailability::Pending
        }));
        let active_tab = browser
            .tabs
            .iter()
            .find(|tab| tab.id == browser.active_tab_id)
            .expect("active tab");
        assert!(Arc::ptr_eq(&browser.entries, &active_tab.entries));
        let active_pane = browser
            .panes
            .iter()
            .find(|pane| pane.id == browser.active_pane_id())
            .expect("active pane");
        assert!(Arc::ptr_eq(&browser.entries, &active_pane.entries));
        let committed_discovery = browser
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
        browser.current_dir = current_directory.clone();
        browser.view_mode = BrowserViewMode::List;
        browser.directory_collection_phase = DirectoryCollectionPhase::Ready;
        browser.entries = (0..5_000)
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
            .column_viewports
            .insert(current_directory.clone(), viewport);
        browser.sync_active_tab_state();
        let previous_entries = Arc::clone(&browser.entries);
        let expected_range = crate::visible_entries::list_entry_range_for_viewport(
            &previous_entries,
            &browser.expanded_directories,
            crate::list_view::LIST_ROW_HEIGHT,
            crate::list_view::LIST_HEADER_HEIGHT,
            viewport.offset_y,
            viewport.height,
            crate::list_view::LIST_OVERSCAN_ROWS,
        );

        drop(browser.navigate_to(destination, NavigationMode::RecordHistory));

        assert!(browser.entries.is_empty());
        assert_eq!(browser.entries.capacity(), 0);
        assert_eq!(previous_entries.len(), 5_000);
        let placeholder = browser
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
}
