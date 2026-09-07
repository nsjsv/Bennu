use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::Task;
use thumbnails::ThumbnailRequest;

use super::{FileBrowser, PaneIconGridViewport};
use crate::commands::thumbnail_batch_command;
use crate::model::{
    sanitized_application_log_detail, BrowserPane, BrowserPaneId, BrowserViewMode,
    IconGridViewport, Message, ScrollbarRegion,
};
use crate::thumbnail_cache::{
    request_for_entry, request_for_transfer_conflict_path, ColumnViewport, ThumbnailLoadOutcome,
    ThumbnailLoadPolicy, ThumbnailLoadResult, ThumbnailPriority, ThumbnailPurpose, ThumbnailScope,
    ThumbnailWork, COLUMN_THUMBNAIL_EDGE, LIST_THUMBNAIL_EDGE, TRANSFER_CONFLICT_THUMBNAIL_EDGE,
};
use crate::virtual_range::initial_virtual_range;

/// 缩略图预热 overscan 行数：对齐视图实际 overscan（16 行），避免大量无效解码排队。
const OVERSCAN_ROWS: usize = 12;

impl FileBrowser {
    pub(super) fn schedule_thumbnail_refresh(&mut self) -> Task<Message> {
        self.schedule_interaction_thumbnails();
        self.schedule_rendered_browser_thumbnails();
        self.pump_thumbnail_queue()
    }

    pub(super) fn schedule_thumbnail_refresh_for_pane(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        if pane_id == self.active_pane_id() {
            self.schedule_interaction_thumbnails();
        }
        self.schedule_rendered_browser_thumbnails_for_pane(pane_id);
        self.pump_thumbnail_queue()
    }

    pub(super) fn handle_column_scrolled(
        &mut self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
        offset_y: f32,
        height: f32,
    ) -> Task<Message> {
        let viewport = ColumnViewport {
            offset_y: offset_y.max(0.0),
            height: height.max(1.0),
        };
        if pane_id == self.active_pane_id() {
            self.column_viewports.insert(directory.clone(), viewport);
        } else {
            let Some(pane) = self.pane_by_id_mut(pane_id) else {
                return Task::none();
            };
            pane.column_viewports.insert(directory.clone(), viewport);
        }
        self.schedule_column_directory_thumbnails_for_pane(
            pane_id,
            &directory,
            ThumbnailPriority::Focused,
        );
        Task::batch([
            self.sync_selection_marquee_scroll(
                ScrollbarRegion::Column { pane_id, directory },
                offset_y,
            ),
            self.pump_thumbnail_queue(),
        ])
    }

    pub(super) fn handle_list_scrolled(
        &mut self,
        pane_id: BrowserPaneId,
        offset_y: f32,
        height: f32,
    ) -> Task<Message> {
        let viewport = ColumnViewport {
            offset_y: offset_y.max(0.0),
            height: height.max(1.0),
        };
        let Some(directory) = self.pane_view(pane_id).map(|pane| pane.current_dir.clone()) else {
            return Task::none();
        };
        if pane_id == self.active_pane_id() {
            self.column_viewports.insert(directory, viewport);
        } else if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.column_viewports.insert(directory, viewport);
        }
        self.schedule_visible_list_thumbnail_range_for_pane(pane_id, Some(viewport));
        Task::batch([
            self.sync_selection_marquee_scroll(ScrollbarRegion::PaneList(pane_id), offset_y),
            self.schedule_visible_list_directory_summary_range_for_pane(pane_id, Some(viewport)),
            self.pump_thumbnail_queue(),
        ])
    }

    pub(super) fn handle_icon_grid_scrolled(
        &mut self,
        pane_id: BrowserPaneId,
        offset_y: f32,
        width: f32,
        height: f32,
    ) -> Task<Message> {
        let Some(directory) = self.pane_view(pane_id).map(|pane| pane.current_dir.clone()) else {
            return Task::none();
        };
        self.icon_grid_viewports.insert(
            pane_id,
            PaneIconGridViewport {
                directory,
                viewport: IconGridViewport {
                    offset_y: offset_y.max(0.0),
                    width: width.max(1.0),
                    height: height.max(1.0),
                },
            },
        );
        self.schedule_visible_icon_grid_thumbnails_for_pane(pane_id);
        Task::batch([
            self.sync_selection_marquee_scroll(ScrollbarRegion::PaneIcons(pane_id), offset_y),
            self.pump_thumbnail_queue(),
        ])
    }

    pub(super) fn schedule_transfer_conflict_thumbnails(&mut self) -> Task<Message> {
        self.enqueue_current_transfer_conflict_thumbnail_requests();
        self.pump_thumbnail_queue()
    }

    pub(super) fn accept_thumbnail_batch(
        &mut self,
        outcomes: Vec<ThumbnailLoadOutcome>,
    ) -> Task<Message> {
        let mut commands = Vec::new();
        for outcome in outcomes {
            let key = outcome.work.key();
            self.thumbnail_cache.finish(&key);

            match outcome.result {
                ThumbnailLoadResult::Ready(thumbnail) => {
                    tracing::info!(
                        target: "app_ui::thumbnail",
                        source = ?thumbnail.source,
                        purpose = ?outcome.work.purpose,
                        width = thumbnail.width,
                        height = thumbnail.height,
                        cache_hit = thumbnail.cache_hit,
                        key_prefix = %thumbnail.key.as_str().get(..12).unwrap_or(""),
                        "thumbnail batch item loaded"
                    );
                    let is_current_request =
                        self.is_current_thumbnail_request(&outcome.work.request);
                    if !is_current_request {
                        self.requeue_drifted_thumbnail_request(&outcome.work);
                        if outcome.work.purpose == ThumbnailPurpose::Preview {
                            commands.push(
                                self.accept_preview_thumbnail_unavailable(&outcome.work.request),
                            );
                        }
                        continue;
                    }
                    // 诊断埋点：插入的 key 与当前条目 key 不同 = 视图将永远查不到这份结果。
                    if let Some(entry) = self.entry_for_path(&thumbnail.source) {
                        if let Some(current_request) =
                            request_for_entry(entry, outcome.work.request.max_edge)
                        {
                            let inserted = thumbnail.key.as_str();
                            let current = current_request.key();
                            if inserted != current.as_str() {
                                tracing::warn!(
                                    target: "app_ui::thumbnail",
                                    source = ?thumbnail.source,
                                    inserted_prefix = %inserted.get(..12).unwrap_or(""),
                                    current_prefix = %current.as_str().get(..12).unwrap_or(""),
                                    "inserted thumbnail key mismatches current entry key"
                                );
                            }
                        }
                    }
                    let ready = self
                        .thumbnail_cache
                        .insert_ready(thumbnail, outcome.work.request.max_edge);
                    if outcome.work.purpose == ThumbnailPurpose::Preview {
                        commands.push(
                            self.accept_preview_thumbnail_ready(&outcome.work.request, ready),
                        );
                    }
                }
                ThumbnailLoadResult::CacheMiss => {
                    tracing::debug!(
                        target: "app_ui::thumbnail",
                        source = ?outcome.work.request.source,
                        purpose = ?outcome.work.purpose,
                        "cached thumbnail missed"
                    );
                    if outcome.work.purpose == ThumbnailPurpose::Preview {
                        commands
                            .push(self.accept_preview_thumbnail_unavailable(&outcome.work.request));
                    }
                }
                ThumbnailLoadResult::Failed(error) => {
                    let log_error = sanitized_application_log_detail(&error);
                    tracing::info!(
                        target: "app_ui::thumbnail",
                        source = ?outcome.work.request.source,
                        purpose = ?outcome.work.purpose,
                        error = %log_error,
                        "thumbnail batch item failed"
                    );
                    self.thumbnail_cache.mark_failure(key);
                    if outcome.work.purpose == ThumbnailPurpose::Preview {
                        commands
                            .push(self.accept_preview_thumbnail_unavailable(&outcome.work.request));
                    }
                }
            }
        }

        self.schedule_interaction_thumbnails();
        commands.push(self.pump_thumbnail_queue());
        Task::batch(commands)
    }

    pub(super) fn pump_thumbnail_queue(&mut self) -> Task<Message> {
        let works = self.thumbnail_cache.take_next_batch();
        if works.is_empty() {
            return Task::none();
        }
        tracing::debug!(
            target: "app_ui::thumbnail",
            count = works.len(),
            "thumbnail batch scheduled"
        );
        thumbnail_batch_command(self.thumbnail_cache.cache_dir(), works)
    }

    fn schedule_interaction_thumbnails(&mut self) {
        // hover/选中缩略图调度走条目索引查找：先保证索引可用。
        self.refresh_entry_index();
        if self
            .pane_view(self.active_pane_id())
            .is_some_and(|pane| pane.view_mode == BrowserViewMode::Icons)
        {
            return;
        }

        if let Some(path) = self.selected.clone() {
            if let Some(entry) = self.entry_for_path(&path).cloned() {
                self.enqueue_list_thumbnail_for_entry(
                    &entry,
                    self.active_entry_thumbnail_edge(),
                    ThumbnailPriority::Focused,
                );
                tracing::info!(
                    target: "app_ui::thumbnail",
                    source = ?entry.path,
                    interaction = "selected",
                    "interaction thumbnail enqueue"
                );
            }
        }

        if let Some(path) = self.hovered_entry.clone() {
            if let Some(entry) = self.entry_for_path(&path).cloned() {
                self.enqueue_list_thumbnail_for_entry(
                    &entry,
                    self.active_entry_thumbnail_edge(),
                    ThumbnailPriority::Focused,
                );
                tracing::info!(
                    target: "app_ui::thumbnail",
                    source = ?entry.path,
                    interaction = "hovered",
                    "interaction thumbnail enqueue"
                );
            }
        }
    }

    fn schedule_rendered_browser_thumbnails(&mut self) {
        for pane_id in self.pane_layout.visible_pane_ids() {
            self.schedule_rendered_browser_thumbnails_for_pane(pane_id);
        }
    }

    fn schedule_rendered_browser_thumbnails_for_pane(&mut self, pane_id: BrowserPaneId) {
        match self.pane_view(pane_id).map(|pane| pane.view_mode) {
            Some(BrowserViewMode::List) => {
                self.schedule_visible_list_thumbnails_for_pane(pane_id);
                return;
            }
            Some(BrowserViewMode::Icons) => {
                self.schedule_visible_icon_grid_thumbnails_for_pane(pane_id);
                return;
            }
            Some(BrowserViewMode::Columns) => {}
            None => return,
        }

        let directories = rendered_column_directories_for_browser(self, pane_id);
        let focused_index = directories.len().saturating_sub(1);
        for (index, directory) in directories.iter().enumerate() {
            let priority = if index == focused_index {
                ThumbnailPriority::Focused
            } else {
                ThumbnailPriority::Visible
            };
            self.schedule_column_directory_thumbnails_for_pane(pane_id, directory, priority);
        }
    }

    fn schedule_visible_icon_grid_thumbnails_for_pane(&mut self, pane_id: BrowserPaneId) {
        let icon_edge = self.user_config.icons_icon_edge();
        let thumbnail_edge = crate::icon_grid_geometry::thumbnail_edge(icon_edge);
        let Some((directory, requests)) = self.pane_view(pane_id).map(|pane| {
            let layout = self.icon_grid_layout_for_pane(pane);
            let requests = layout
                .visible_entries(pane.icon_grid_viewport)
                .into_iter()
                .filter_map(|visible| request_for_entry(visible.entry, thumbnail_edge))
                .collect::<Vec<_>>();
            (pane.current_dir.clone(), requests)
        }) else {
            return;
        };
        let scope = thumbnail_scope_for_pane_directory(pane_id, &directory);
        let keep = requests
            .iter()
            .map(ThumbnailRequest::key)
            .collect::<Vec<_>>();
        self.thumbnail_cache.prune_scope_except(&scope, &keep);
        for request in requests {
            self.enqueue_list_thumbnail_request_for_scope(
                request,
                ThumbnailPriority::Visible,
                scope.clone(),
            );
        }
    }

    fn schedule_visible_list_thumbnails_for_pane(&mut self, pane_id: BrowserPaneId) {
        self.schedule_visible_list_thumbnail_range_for_pane(pane_id, None);
    }

    fn schedule_visible_list_thumbnail_range_for_pane(
        &mut self,
        pane_id: BrowserPaneId,
        viewport: Option<ColumnViewport>,
    ) {
        let Some(pane) = self.pane_view(pane_id) else {
            return;
        };
        let geometry =
            crate::list_view::ListGeometry::for_level(self.user_config.list_view_density);
        let directory = pane.current_dir.clone();
        // 初始预热行数必须由真实视口高度推导；固定行数在窗口较高时会让
        // 折叠线以下的可见条目永远等不到调度，只能靠 hover/滚动补救。
        let initial_rows = crate::list_view::list_initial_rows(
            self.main_window_height,
            self.user_config.list_view_density,
        );
        let range = viewport
            .map(|viewport| {
                crate::visible_entries::list_entry_range_for_viewport(
                    pane.entries,
                    pane.expanded_directories,
                    geometry.row_height,
                    crate::list_view::LIST_HEADER_HEIGHT,
                    viewport.offset_y,
                    viewport.height,
                    OVERSCAN_ROWS,
                )
            })
            .unwrap_or_else(|| {
                crate::visible_entries::initial_list_entry_range(
                    pane.entries,
                    pane.expanded_directories,
                    geometry.row_height,
                    initial_rows,
                )
            });
        let entries = crate::visible_entries::visible_entries_in_range(
            pane.entries,
            pane.expanded_directories,
            range.start,
            range.end,
        );
        let scope = thumbnail_scope_for_pane_directory(pane_id, &directory);
        let requests = entries
            .iter()
            .filter_map(|visible_entry| request_for_entry(visible_entry.entry, LIST_THUMBNAIL_EDGE))
            .collect::<Vec<_>>();
        tracing::info!(
            target: "app_ui::thumbnail",
            pane = %pane_id.key(),
            directory = %directory.display(),
            range_start = range.start,
            range_end = range.end,
            requests = requests.len(),
            "list thumbnail schedule"
        );
        let keep = requests
            .iter()
            .map(ThumbnailRequest::key)
            .collect::<Vec<_>>();
        self.thumbnail_cache.prune_scope_except(&scope, &keep);
        for request in requests {
            self.enqueue_list_thumbnail_request_for_scope(
                request,
                ThumbnailPriority::Visible,
                scope.clone(),
            );
        }
    }

    fn schedule_column_directory_thumbnails_for_pane(
        &mut self,
        pane_id: BrowserPaneId,
        directory: &Path,
        priority: ThumbnailPriority,
    ) {
        let requests = self.thumbnail_requests_for_pane_directory_range(pane_id, directory);
        let scope = thumbnail_scope_for_pane_directory(pane_id, directory);
        let keep = requests
            .iter()
            .map(ThumbnailRequest::key)
            .collect::<Vec<_>>();
        self.thumbnail_cache.prune_scope_except(&scope, &keep);
        for request in requests {
            self.enqueue_list_thumbnail_request_for_scope(request, priority, scope.clone());
        }
    }

    fn enqueue_list_thumbnail_for_entry(
        &mut self,
        entry: &DirectoryEntry,
        max_edge: u32,
        priority: ThumbnailPriority,
    ) {
        let Some(request) = request_for_entry(entry, max_edge) else {
            return;
        };
        self.enqueue_thumbnail_request(request, ThumbnailPurpose::List, priority);
    }

    fn enqueue_current_transfer_conflict_thumbnail_requests(&mut self) {
        let requests = self
            .transfer_conflict
            .as_ref()
            .and_then(|state| state.current_conflict())
            .map(|conflict| {
                [
                    request_for_transfer_conflict_path(
                        &conflict.target,
                        &conflict.target_metadata,
                        TRANSFER_CONFLICT_THUMBNAIL_EDGE,
                    ),
                    request_for_transfer_conflict_path(
                        &conflict.source,
                        &conflict.source_metadata,
                        TRANSFER_CONFLICT_THUMBNAIL_EDGE,
                    ),
                ]
            });

        let Some(requests) = requests else {
            return;
        };

        for request in requests.into_iter().flatten() {
            self.enqueue_thumbnail_request(
                request,
                ThumbnailPurpose::TransferConflict,
                ThumbnailPriority::Focused,
            );
        }
    }

    fn enqueue_thumbnail_request(
        &mut self,
        request: ThumbnailRequest,
        purpose: ThumbnailPurpose,
        priority: ThumbnailPriority,
    ) {
        match self.thumbnail_load_policy_for_path(&request.source) {
            ThumbnailLoadPolicy::LoadOrGenerate => {
                self.thumbnail_cache
                    .enqueue_request(request, purpose, priority);
            }
            ThumbnailLoadPolicy::CacheOnly => {
                self.thumbnail_cache
                    .enqueue_cached_request(request, purpose, priority);
            }
        }
    }

    fn enqueue_list_thumbnail_request_for_scope(
        &mut self,
        request: ThumbnailRequest,
        priority: ThumbnailPriority,
        scope: ThumbnailScope,
    ) {
        match self.thumbnail_load_policy_for_path(&request.source) {
            ThumbnailLoadPolicy::LoadOrGenerate => {
                self.thumbnail_cache.enqueue_request_for_scope(
                    request,
                    ThumbnailPurpose::List,
                    priority,
                    scope,
                );
            }
            ThumbnailLoadPolicy::CacheOnly => {
                self.thumbnail_cache.enqueue_cached_request_for_scope(
                    request,
                    ThumbnailPurpose::List,
                    priority,
                    scope,
                );
            }
        }
    }

    fn thumbnail_load_policy_for_path(&self, path: &Path) -> ThumbnailLoadPolicy {
        if self.path_is_remote_mount(path) && !self.remote_list_thumbnail_downloads_enabled() {
            ThumbnailLoadPolicy::CacheOnly
        } else {
            ThumbnailLoadPolicy::LoadOrGenerate
        }
    }

    fn thumbnail_requests_for_pane_directory_range(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
    ) -> Vec<ThumbnailRequest> {
        let Some(entries) = self.entries_for_pane_directory(pane_id, directory) else {
            return Vec::new();
        };
        if entries.is_empty() {
            return Vec::new();
        }

        let (start, end) =
            self.thumbnail_range_for_pane_directory(pane_id, directory, entries.len());
        entries[start..end]
            .iter()
            .filter_map(|entry| request_for_entry(entry, COLUMN_THUMBNAIL_EDGE))
            .collect()
    }

    fn entries_for_pane_directory(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
    ) -> Option<&[DirectoryEntry]> {
        if pane_id == self.active_pane_id() {
            return self.entries_for_directory(directory);
        }

        let pane = self.pane_by_id(pane_id)?;
        if directory == pane.current_dir {
            return Some(&pane.entries);
        }

        pane.expanded_directories
            .get(directory)
            .map(|expanded| expanded.entries.as_slice())
    }

    fn entries_for_directory(&self, directory: &Path) -> Option<&[DirectoryEntry]> {
        if directory == self.current_dir {
            return Some(&self.entries);
        }

        self.expanded_directories
            .get(directory)
            .map(|expanded| expanded.entries.as_slice())
    }

    fn thumbnail_range_for_pane_directory(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
        len: usize,
    ) -> (usize, usize) {
        let geometry = crate::three_column_view::ColumnGeometry::for_level(
            self.user_config.columns_view_density,
        );
        let range = self
            .column_viewport_for_pane_directory(pane_id, directory)
            .map(|viewport| {
                crate::three_column_view::column_virtual_range_for_viewport(
                    len,
                    viewport.offset_y,
                    viewport.height,
                    OVERSCAN_ROWS,
                    geometry,
                )
            })
            .unwrap_or_else(|| {
                initial_virtual_range(
                    len,
                    geometry.entry_scroll_height,
                    crate::three_column_view::column_initial_rows(
                        self.main_window_height,
                        self.user_config.columns_view_density,
                    ),
                )
            });
        (range.start, range.end)
    }

    fn column_viewport_for_pane_directory(
        &self,
        pane_id: BrowserPaneId,
        directory: &Path,
    ) -> Option<ColumnViewport> {
        if pane_id == self.active_pane_id() {
            return self.column_viewports.get(directory).copied();
        }

        self.pane_by_id(pane_id)
            .and_then(|pane| pane.column_viewports.get(directory).copied())
    }

    fn is_current_thumbnail_request(&self, request: &ThumbnailRequest) -> bool {
        self.entry_for_path(&request.source)
            .is_some_and(|entry| thumbnail_request_matches_entry(entry, request))
            || self
                .panes
                .iter()
                .any(|pane| thumbnail_request_matches_pane(pane, request))
            || self
                .transfer_conflict
                .as_ref()
                .is_some_and(|state| thumbnail_request_matches_transfer_conflict(state, request))
    }

    /// 被拒结果里条目仍在、只是请求 key 漂移时（inflight 期间条目元数据落地：
    /// 初始扫描的 Pending → demand 填充、增量应用预填真实值），用当前条目的
    /// key 重新入队。否则该条目在 hover/滚动重新调度前一直显示占位图标。
    fn requeue_drifted_thumbnail_request(&mut self, work: &ThumbnailWork) {
        let Some(entry) = self.entry_for_path(&work.request.source) else {
            return;
        };
        let Some(request) = request_for_entry(entry, work.request.max_edge) else {
            return;
        };
        if request.key() == work.request.key() {
            return;
        }
        tracing::info!(
            target: "app_ui::thumbnail",
            source = ?work.request.source,
            purpose = ?work.purpose,
            "thumbnail request key drifted; requeueing with current entry metadata"
        );
        match (work.load_policy, work.scope.clone()) {
            (ThumbnailLoadPolicy::LoadOrGenerate, Some(scope)) => self
                .thumbnail_cache
                .enqueue_request_for_scope(request, work.purpose, work.priority, scope),
            (ThumbnailLoadPolicy::LoadOrGenerate, None) => {
                let _ = self
                    .thumbnail_cache
                    .enqueue_request(request, work.purpose, work.priority);
            }
            (ThumbnailLoadPolicy::CacheOnly, Some(scope)) => self
                .thumbnail_cache
                .enqueue_cached_request_for_scope(request, work.purpose, work.priority, scope),
            (ThumbnailLoadPolicy::CacheOnly, None) => self
                .thumbnail_cache
                .enqueue_cached_request(request, work.purpose, work.priority),
        }
    }

    fn active_entry_thumbnail_edge(&self) -> u32 {
        match self
            .pane_view(self.active_pane_id())
            .map(|pane| pane.view_mode)
        {
            Some(BrowserViewMode::Columns) => COLUMN_THUMBNAIL_EDGE,
            Some(BrowserViewMode::Icons) => {
                crate::icon_grid_geometry::thumbnail_edge(self.user_config.icons_icon_edge())
            }
            Some(BrowserViewMode::List) | None => LIST_THUMBNAIL_EDGE,
        }
    }
}

fn rendered_column_directories_for_browser(
    browser: &FileBrowser,
    pane_id: BrowserPaneId,
) -> Vec<PathBuf> {
    browser
        .pane_view(pane_id)
        .map(crate::three_column_view::column_directories_for_pane)
        .unwrap_or_default()
}

fn thumbnail_request_matches_entry(entry: &DirectoryEntry, request: &ThumbnailRequest) -> bool {
    request_for_entry(entry, request.max_edge).is_some_and(|current| current.key() == request.key())
}

fn thumbnail_request_matches_pane(pane: &BrowserPane, request: &ThumbnailRequest) -> bool {
    let entry = match pane.view_mode {
        BrowserViewMode::Icons => pane
            .entries
            .iter()
            .find(|entry| entry.path == request.source),
        BrowserViewMode::Columns | BrowserViewMode::List => pane
            .entries
            .iter()
            .chain(
                pane.expanded_directories
                    .values()
                    .flat_map(|expanded| expanded.entries.iter()),
            )
            .find(|entry| entry.path == request.source),
    };
    entry.is_some_and(|entry| thumbnail_request_matches_entry(entry, request))
}

fn thumbnail_request_matches_transfer_conflict(
    state: &crate::model::TransferConflictState,
    request: &ThumbnailRequest,
) -> bool {
    state.current_conflict().is_some_and(|conflict| {
        thumbnail_request_matches_transfer_conflict_path(
            &conflict.target,
            &conflict.target_metadata,
            request,
        ) || thumbnail_request_matches_transfer_conflict_path(
            &conflict.source,
            &conflict.source_metadata,
            request,
        )
    })
}

fn thumbnail_request_matches_transfer_conflict_path(
    path: &Path,
    metadata: &file_core::TransferConflictMetadata,
    request: &ThumbnailRequest,
) -> bool {
    request_for_transfer_conflict_path(path, metadata, request.max_edge)
        .is_some_and(|current| current.key() == request.key())
}

fn thumbnail_scope_for_pane_directory(pane_id: BrowserPaneId, directory: &Path) -> ThumbnailScope {
    ThumbnailScope::PaneDirectory {
        pane_id: pane_id.key(),
        directory: directory.to_path_buf(),
    }
}

#[cfg(test)]
mod tests;
