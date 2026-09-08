use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use file_core::{
    discovered_sort_is_ready, sort_discovered_entry_indices, DirectoryDiscovery,
    DirectoryMetadataRequirement, DirectoryMetadataResolution, DirectoryMetadataState, SortField,
};
use iced::Task;

use super::{DirectoryMetadataDemandKey, FileBrowser};
use crate::commands::load_directory_metadata_command;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryMetadataLoadContext, DirectoryMetadataLoadFailure,
    DirectoryMetadataLoadRequest, ExpandedDirectoryStatus, ListColumnKind, Message,
};
use crate::thumbnail_cache::ColumnViewport;

struct DirectoryMetadataDemandSource {
    context: DirectoryMetadataLoadContext,
    discovery: DirectoryDiscovery,
    cancellation: tokio_util::sync::CancellationToken,
    targets: Vec<usize>,
}

impl FileBrowser {
    pub(super) fn clear_directory_metadata_demands_for_pane(&mut self, pane_id: BrowserPaneId) {
        self.directory_metadata_in_flight
            .retain(|key| key.context.pane_id() != pane_id);
    }

    pub(super) fn schedule_visible_directory_metadata(
        &mut self,
        pane_id: BrowserPaneId,
        viewport_override: Option<ColumnViewport>,
    ) -> Task<Message> {
        self.ensure_expanded_metadata_cancellation_tokens(pane_id);
        // 选中统计的大小需求与视图模式无关,借同一入口调度:目录加载、
        // 滚动等既有触发点由此自动覆盖选中条目。
        let selected_demand = self.schedule_selected_directory_metadata(pane_id);
        let visible_demand =
            self.schedule_list_visible_directory_metadata(pane_id, viewport_override);
        Task::batch([selected_demand, visible_demand])
    }

    /// 选中条目的文件系统元数据 demand:底部统计栏要显示选中文件的真实
    /// 大小,而可见范围 demand 只覆盖视口内且相关列可见的行。选中的文件
    /// 一律纳入,覆盖三种视图:直接条目走 Root,列表/多栏/图标的展开子
    /// 条目走 Expanded。是否真的发起由 in-flight 去重决定,重复调度安全。
    fn schedule_selected_directory_metadata(&mut self, pane_id: BrowserPaneId) -> Task<Message> {
        let Some(pane) = self.pane_view(pane_id) else {
            return Task::none();
        };
        if pane.selected_paths.is_empty() {
            return Task::none();
        }
        let root_generation = if pane_id == self.active_pane_id() {
            self.directory_load_generation
        } else {
            self.pane_by_id(pane_id)
                .map(|pane| pane.directory_load_generation)
                .unwrap_or_default()
        };
        let root_context = DirectoryMetadataLoadContext::Root {
            pane_id,
            path: pane.current_dir.clone(),
            collection_generation: root_generation,
        };
        // (context, discovery, cancellation, targets):一次性收集后统一调度
        let mut groups: Vec<(
            DirectoryMetadataLoadContext,
            DirectoryDiscovery,
            tokio_util::sync::CancellationToken,
            Vec<usize>,
        )> = Vec::new();

        let selected = pane.selected_paths;
        // 直接条目:一次遍历窗格条目,避免按选中路径逐个线性查找。
        if let Some(discovery) = pane.directory_discovery.cloned() {
            let mut targets = Vec::new();
            for entry in pane.entries.iter() {
                if !selected.contains(&entry.path) {
                    continue;
                }
                if let Some(index) = entry.discovery_index {
                    targets.push(index);
                }
            }
            if !targets.is_empty() {
                groups.push((
                    root_context.clone(),
                    discovery,
                    self.directory_load_cancel_for_pane(pane_id),
                    targets,
                ));
            }
        }
        // 列表/多栏:展开子条目挂在 expanded_directories。
        for (directory_path, expanded) in pane.expanded_directories.iter() {
            if !matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
                continue;
            }
            let Some(discovery) = expanded.directory_discovery.clone() else {
                continue;
            };
            let mut targets = Vec::new();
            for entry in expanded.entries.iter() {
                if !selected.contains(&entry.path) {
                    continue;
                }
                if let Some(index) = entry.discovery_index {
                    targets.push(index);
                }
            }
            if !targets.is_empty() {
                groups.push((
                    DirectoryMetadataLoadContext::Expanded {
                        pane_id,
                        path: directory_path.clone(),
                        load_generation: expanded.load_generation,
                    },
                    discovery,
                    expanded.load_cancel.clone().unwrap_or_default(),
                    targets,
                ));
            }
        }
        // 图标视图:展开子条目挂在 icon_grid_expansion 的交互目录里。
        let icon_expansion = self.icon_grid_expansion.as_ref().filter(|state| {
            state.context().pane_id == pane.id
                && state.context().current_dir == *pane.current_dir
        });
        if let Some(expansion) = icon_expansion {
            for (directory_path, directory) in expansion.directories() {
                let contents = &directory.contents;
                if !matches!(contents.status, ExpandedDirectoryStatus::Loaded) {
                    continue;
                }
                let Some(discovery) = contents.directory_discovery.clone() else {
                    continue;
                };
                let mut targets = Vec::new();
                for entry in contents.entries.iter() {
                    if !selected.contains(&entry.path) {
                        continue;
                    }
                    if let Some(index) = entry.discovery_index {
                        targets.push(index);
                    }
                }
                if !targets.is_empty() {
                    groups.push((
                        DirectoryMetadataLoadContext::Expanded {
                            pane_id,
                            path: directory_path.to_path_buf(),
                            load_generation: contents.load_generation,
                        },
                        discovery,
                        contents.load_cancel.clone().unwrap_or_default(),
                        targets,
                    ));
                }
            }
        }

        let mut tasks = Vec::new();
        for (context, discovery, cancellation, targets) in groups {
            tasks.push(self.schedule_directory_metadata_requirement(
                context,
                &discovery,
                cancellation,
                DirectoryMetadataRequirement::Filesystem,
                targets,
            ));
        }
        Task::batch(tasks)
    }

    /// 选中集合签名:排序后哈希保证同一集合签名稳定;签名变化才补调度,
    /// 让每条消息出口处的检查在选中未变时零成本通过。
    pub(super) fn schedule_selected_metadata_if_selection_changed(&mut self) -> Task<Message> {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        {
            let mut paths: Vec<&PathBuf> = self.selected_paths.iter().collect();
            paths.sort();
            paths.hash(&mut hasher);
        }
        for pane in &self.panes {
            let mut paths: Vec<&PathBuf> = pane.selected_paths.iter().collect();
            paths.sort();
            paths.hash(&mut hasher);
        }
        let signature = hasher.finish();
        if self.selection_metadata_signature == signature {
            return Task::none();
        }
        self.selection_metadata_signature = signature;
        let pane_ids: Vec<BrowserPaneId> = self.panes.iter().map(|pane| pane.id).collect();
        Task::batch(
            pane_ids
                .into_iter()
                .map(|pane_id| self.schedule_selected_directory_metadata(pane_id))
                .collect::<Vec<_>>(),
        )
    }

    fn directory_load_cancel_for_pane(&self, pane_id: BrowserPaneId) -> tokio_util::sync::CancellationToken {
        if pane_id == self.active_pane_id() {
            self.directory_load_cancel.clone()
        } else {
            self.pane_by_id(pane_id)
                .and_then(|pane| pane.directory_load_cancel.clone())
        }
        .unwrap_or_default()
    }

    fn schedule_list_visible_directory_metadata(
        &mut self,
        pane_id: BrowserPaneId,
        viewport_override: Option<ColumnViewport>,
    ) -> Task<Message> {
        let root_generation = if pane_id == self.active_pane_id() {
            self.directory_load_generation
        } else {
            self.pane_by_id(pane_id)
                .map(|pane| pane.directory_load_generation)
                .unwrap_or_default()
        };
        let root_cancellation = if pane_id == self.active_pane_id() {
            self.directory_load_cancel.clone()
        } else {
            self.pane_by_id(pane_id)
                .and_then(|pane| pane.directory_load_cancel.clone())
        }
        .unwrap_or_default();
        let window_height = self.main_window_height;
        let sources = {
            let Some(pane) = self.pane_view(pane_id) else {
                return Task::none();
            };
            if pane.view_mode != BrowserViewMode::List {
                return Task::none();
            }
            let Some(root_discovery) = pane.directory_discovery.cloned() else {
                return Task::none();
            };
            let root_context = DirectoryMetadataLoadContext::Root {
                pane_id,
                path: pane.current_dir.clone(),
                collection_generation: root_generation,
            };
            let mut sources = HashMap::from([(
                root_context.clone(),
                DirectoryMetadataDemandSource {
                    context: root_context.clone(),
                    discovery: root_discovery,
                    cancellation: root_cancellation,
                    targets: Vec::new(),
                },
            )]);
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
            for visible in crate::visible_entries::visible_entries_in_range(
                pane.entries,
                pane.expanded_directories,
                range.start,
                range.end,
            ) {
                let Some(index) = visible.entry.discovery_index else {
                    continue;
                };
                let context = if visible.entry.path.parent() == Some(pane.current_dir.as_path()) {
                    root_context.clone()
                } else {
                    let Some(parent) = visible.entry.path.parent() else {
                        continue;
                    };
                    let Some(expanded) = pane.expanded_directories.get(parent).filter(|expanded| {
                        matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
                    }) else {
                        continue;
                    };
                    let Some(discovery) = expanded.directory_discovery.clone() else {
                        continue;
                    };
                    let context = DirectoryMetadataLoadContext::Expanded {
                        pane_id,
                        path: parent.to_path_buf(),
                        load_generation: expanded.load_generation,
                    };
                    sources.entry(context.clone()).or_insert_with(|| {
                        DirectoryMetadataDemandSource {
                            context: context.clone(),
                            discovery,
                            cancellation: expanded.load_cancel.clone().unwrap_or_default(),
                            targets: Vec::new(),
                        }
                    });
                    context
                };
                if let Some(source) = sources.get_mut(&context) {
                    source.targets.push(index);
                }
            }
            sources.into_values().collect::<Vec<_>>()
        };
        let visible_columns = self
            .user_config
            .list_view_preferences
            .visible_columns()
            .map(|column| column.kind)
            .collect::<Vec<_>>();
        let filesystem_is_visible = visible_columns.iter().any(|kind| {
            matches!(
                kind,
                ListColumnKind::Modified
                    | ListColumnKind::Size
                    | ListColumnKind::Permissions
                    | ListColumnKind::Accessed
                    | ListColumnKind::Created
            )
        });
        let identity_names_are_visible = visible_columns
            .iter()
            .any(|kind| matches!(kind, ListColumnKind::Owner | ListColumnKind::Group));
        let sort_requires_filesystem = matches!(
            self.options.sort_field,
            SortField::Size | SortField::Modified
        );
        let mut tasks = Vec::new();
        for source in sources {
            let filesystem_targets = if sort_requires_filesystem {
                (0..source.discovery.entries.len()).collect()
            } else if filesystem_is_visible {
                source.targets.clone()
            } else {
                Vec::new()
            };
            tasks.push(self.schedule_directory_metadata_requirement(
                source.context.clone(),
                &source.discovery,
                source.cancellation.clone(),
                DirectoryMetadataRequirement::Filesystem,
                filesystem_targets,
            ));
            if identity_names_are_visible {
                tasks.push(self.schedule_directory_metadata_requirement(
                    source.context,
                    &source.discovery,
                    source.cancellation,
                    DirectoryMetadataRequirement::IdentityNames,
                    source.targets,
                ));
            }
        }
        Task::batch(tasks)
    }

    fn ensure_expanded_metadata_cancellation_tokens(&mut self, pane_id: BrowserPaneId) {
        let expanded_directories = if pane_id == self.active_pane_id() {
            &mut self.expanded_directories
        } else {
            let Some(pane) = self.pane_by_id_mut(pane_id) else {
                return;
            };
            &mut pane.expanded_directories
        };
        for expanded in expanded_directories.values_mut().filter(|expanded| {
            matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
                && expanded.directory_discovery.is_some()
        }) {
            if expanded.load_cancel.is_none() {
                expanded.load_cancel = Some(tokio_util::sync::CancellationToken::new());
            }
        }
    }

    fn schedule_directory_metadata_requirement(
        &mut self,
        context: DirectoryMetadataLoadContext,
        discovery: &DirectoryDiscovery,
        cancellation: tokio_util::sync::CancellationToken,
        requirement: DirectoryMetadataRequirement,
        mut targets: Vec<usize>,
    ) -> Task<Message> {
        targets.sort_unstable();
        targets.dedup();
        targets.retain(|index| {
            let Some(entry) = discovery.entries.get(*index) else {
                return false;
            };
            let pending = match requirement {
                DirectoryMetadataRequirement::Filesystem => {
                    matches!(entry.filesystem_metadata(), DirectoryMetadataState::Pending)
                }
                DirectoryMetadataRequirement::IdentityNames => {
                    matches!(entry.identity_names(), DirectoryMetadataState::Pending)
                }
            };
            let key = DirectoryMetadataDemandKey {
                context: context.clone(),
                requirement,
                index: *index,
            };
            pending && self.directory_metadata_in_flight.insert(key)
        });
        if targets.is_empty() {
            return Task::none();
        }
        let request_generation = self.next_directory_metadata_request_generation;
        self.next_directory_metadata_request_generation = request_generation.wrapping_add(1);
        if requirement == DirectoryMetadataRequirement::Filesystem
            && matches!(
                self.options.sort_field,
                SortField::Size | SortField::Modified
            )
        {
            self.set_directory_order_waiting(&context, request_generation);
        }
        load_directory_metadata_command(
            DirectoryMetadataLoadRequest {
                context,
                request_generation,
                requirement,
                targets,
            },
            discovery.metadata_resolver.clone(),
            cancellation,
        )
    }

    fn set_directory_order_waiting(
        &mut self,
        context: &DirectoryMetadataLoadContext,
        request_generation: u64,
    ) {
        let phase = crate::model::DirectoryOrderPhase::WaitingForMetadata {
            request_generation,
            field: self.options.sort_field,
            direction: self.options.sort_direction,
        };
        match context {
            DirectoryMetadataLoadContext::Root { pane_id, .. }
                if *pane_id == self.active_pane_id() =>
            {
                self.directory_order_phase = phase;
            }
            DirectoryMetadataLoadContext::Root { pane_id, .. } => {
                if let Some(pane) = self.pane_by_id_mut(*pane_id) {
                    pane.directory_order_phase = phase;
                }
            }
            DirectoryMetadataLoadContext::Expanded { pane_id, path, .. }
                if *pane_id == self.active_pane_id() =>
            {
                if let Some(expanded) = self.expanded_directories.get_mut(path) {
                    expanded.directory_order_phase = phase;
                }
            }
            DirectoryMetadataLoadContext::Expanded { pane_id, path, .. } => {
                if let Some(expanded) = self
                    .pane_by_id_mut(*pane_id)
                    .and_then(|pane| pane.expanded_directories.get_mut(path))
                {
                    expanded.directory_order_phase = phase;
                }
            }
        }
    }

    pub(super) fn accept_directory_metadata_resolution(
        &mut self,
        request: DirectoryMetadataLoadRequest,
        outcome: Result<DirectoryMetadataResolution, DirectoryMetadataLoadFailure>,
    ) -> Task<Message> {
        for index in &request.targets {
            self.directory_metadata_in_flight
                .remove(&DirectoryMetadataDemandKey {
                    context: request.context.clone(),
                    requirement: request.requirement,
                    index: *index,
                });
        }
        if !self.directory_metadata_request_is_current(&request) {
            return Task::none();
        }
        let resolution = match outcome {
            Ok(resolution)
                if resolution.request_generation == request.request_generation
                    && resolution.requirement == request.requirement =>
            {
                resolution
            }
            Ok(_) => return Task::none(),
            Err(DirectoryMetadataLoadFailure::Cancelled) => return Task::none(),
            Err(DirectoryMetadataLoadFailure::ReadFailed(message)) => {
                self.show_global_error(message);
                return Task::none();
            }
        };
        crate::startup_trace::record_directory_metadata_resolution(
            directory_metadata_requirement_label(request.requirement),
            request.targets.len(),
            resolution.resolved_indices.len(),
            resolution.warnings.len(),
        );
        if let Some(warning) = resolution.warnings.first() {
            self.show_global_error(warning.message.clone());
        }
        self.commit_requested_metadata_sort_if_ready(&request.context, request.request_generation)
    }

    fn directory_metadata_request_is_current(
        &self,
        request: &DirectoryMetadataLoadRequest,
    ) -> bool {
        match &request.context {
            DirectoryMetadataLoadContext::Root {
                pane_id,
                path,
                collection_generation,
            } if *pane_id == self.active_pane_id() => {
                path == &self.current_dir
                    && *collection_generation == self.directory_load_generation
                    && self.directory_discovery.is_some()
            }
            DirectoryMetadataLoadContext::Root {
                pane_id,
                path,
                collection_generation,
            } => self.pane_by_id(*pane_id).is_some_and(|pane| {
                path == &pane.current_dir
                    && *collection_generation == pane.directory_load_generation
                    && pane.directory_discovery.is_some()
            }),
            DirectoryMetadataLoadContext::Expanded {
                pane_id,
                path,
                load_generation,
            } if *pane_id == self.active_pane_id() => {
                self.expanded_directories.get(path).is_some_and(|expanded| {
                    *load_generation == expanded.load_generation
                        && matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
                        && expanded.directory_discovery.is_some()
                })
            }
            DirectoryMetadataLoadContext::Expanded {
                pane_id,
                path,
                load_generation,
            } => self
                .pane_by_id(*pane_id)
                .and_then(|pane| pane.expanded_directories.get(path))
                .is_some_and(|expanded| {
                    *load_generation == expanded.load_generation
                        && matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
                        && expanded.directory_discovery.is_some()
                }),
        }
    }

    fn commit_requested_metadata_sort_if_ready(
        &mut self,
        context: &DirectoryMetadataLoadContext,
        request_generation: u64,
    ) -> Task<Message> {
        match context {
            DirectoryMetadataLoadContext::Root { pane_id, .. } => {
                self.commit_root_metadata_sort_if_ready(*pane_id, request_generation)
            }
            DirectoryMetadataLoadContext::Expanded { pane_id, path, .. } => {
                self.commit_expanded_metadata_sort_if_ready(*pane_id, path, request_generation)
            }
        }
    }

    fn commit_root_metadata_sort_if_ready(
        &mut self,
        pane_id: BrowserPaneId,
        request_generation: u64,
    ) -> Task<Message> {
        let base_options = self.options.clone();
        if pane_id != self.active_pane_id() {
            let Some(pane) = self.pane_by_id_mut(pane_id) else {
                return Task::none();
            };
            let crate::model::DirectoryOrderPhase::WaitingForMetadata {
                request_generation: expected_generation,
                field,
                direction,
            } = pane.directory_order_phase
            else {
                return Task::none();
            };
            if request_generation != expected_generation {
                return Task::none();
            }
            let options = file_core::ScanOptions {
                sort_field: field,
                sort_direction: direction,
                ..base_options.clone()
            };
            let Some(discovery) = pane.directory_discovery.as_mut() else {
                return Task::none();
            };
            if !discovered_sort_is_ready(&discovery.entries, &options) {
                return Task::none();
            }
            discovery.order = Arc::new(sort_discovered_entry_indices(&discovery.entries, &options));
            pane.entries = Arc::new(crate::model::display_entries_in_discovery_order(discovery));
            pane.directory_order_phase =
                crate::model::DirectoryOrderPhase::Ready { field, direction };
            pane.sync_active_tab_state();
            // 条目元数据从 Pending 落地为真实值，缩略图缓存 key 随之漂移；
            // 重建后必须用新条目重新调度，否则缩略图要等 hover/滚动才出现。
            return self.schedule_thumbnail_refresh_for_pane(pane_id);
        }
        let crate::model::DirectoryOrderPhase::WaitingForMetadata {
            request_generation: expected_generation,
            field,
            direction,
        } = self.directory_order_phase
        else {
            return Task::none();
        };
        if request_generation != expected_generation {
            return Task::none();
        }
        let options = file_core::ScanOptions {
            sort_field: field,
            sort_direction: direction,
            ..base_options
        };
        let display_entries = {
            let Some(discovery) = self.directory_discovery.as_mut() else {
                return Task::none();
            };
            if !discovered_sort_is_ready(&discovery.entries, &options) {
                return Task::none();
            }
            discovery.order = Arc::new(sort_discovered_entry_indices(&discovery.entries, &options));
            Arc::new(crate::model::display_entries_in_discovery_order(discovery))
        };
        self.set_entries(display_entries);
        self.directory_order_phase = crate::model::DirectoryOrderPhase::Ready { field, direction };
        crate::startup_trace::mark_once("initial_directory_requested_sort_ready");
        crate::startup_trace::mark_once("initial_directory_ready");
        self.sync_active_tab_state();
        // 同非活动窗格分支：元数据落地重建条目后，缩略图 key 漂移，必须重调度。
        tracing::info!(
            target: "app_ui::entry_changes",
            entries = self.entries.len(),
            "metadata commit rebuilt root entries; rescheduling thumbnails"
        );
        self.schedule_thumbnail_refresh()
    }

    fn commit_expanded_metadata_sort_if_ready(
        &mut self,
        pane_id: BrowserPaneId,
        path: &std::path::Path,
        request_generation: u64,
    ) -> Task<Message> {
        let base_options = self.options.clone();
        if pane_id != self.active_pane_id() {
            let Some(pane) = self.pane_by_id_mut(pane_id) else {
                return Task::none();
            };
            let Some(expanded) = pane.expanded_directories.get_mut(path) else {
                return Task::none();
            };
            if !commit_expanded_metadata_sort(expanded, request_generation, &base_options) {
                return Task::none();
            }
            pane.sync_active_tab_state();
            // 展开目录条目元数据落地同样漂移缩略图 key，重调度整个窗格可见范围。
            return self.schedule_thumbnail_refresh_for_pane(pane_id);
        }
        let Some(expanded) = self.expanded_directories.get_mut(path) else {
            return Task::none();
        };
        if !commit_expanded_metadata_sort(expanded, request_generation, &base_options) {
            return Task::none();
        }
        self.sync_active_tab_state();
        self.schedule_thumbnail_refresh()
    }
}

fn commit_expanded_metadata_sort(
    expanded: &mut crate::model::ExpandedDirectory,
    request_generation: u64,
    base_options: &file_core::ScanOptions,
) -> bool {
    let crate::model::DirectoryOrderPhase::WaitingForMetadata {
        request_generation: expected_generation,
        field,
        direction,
    } = expanded.directory_order_phase
    else {
        return false;
    };
    if request_generation != expected_generation {
        return false;
    }
    let options = file_core::ScanOptions {
        sort_field: field,
        sort_direction: direction,
        ..base_options.clone()
    };
    let Some(discovery) = expanded.directory_discovery.as_mut() else {
        return false;
    };
    if !discovered_sort_is_ready(&discovery.entries, &options) {
        return false;
    }
    discovery.order = Arc::new(sort_discovered_entry_indices(&discovery.entries, &options));
    expanded.entries = crate::model::display_entries_in_discovery_order(discovery);
    expanded.directory_order_phase = crate::model::DirectoryOrderPhase::Ready { field, direction };
    true
}

fn directory_metadata_requirement_label(requirement: DirectoryMetadataRequirement) -> &'static str {
    match requirement {
        DirectoryMetadataRequirement::Filesystem => "filesystem",
        DirectoryMetadataRequirement::IdentityNames => "identity_names",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{
        discover_directory_with_progress, resolve_directory_metadata, DirectoryMetadataRequest,
        ScanOptions, SortDirection, SortField,
    };
    use tokio_util::sync::CancellationToken;

    use super::*;

    async fn discovered_fixture(
        entry_count: usize,
        options: ScanOptions,
    ) -> (tempfile::TempDir, DirectoryDiscovery) {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..entry_count {
            std::fs::write(directory.path().join(format!("file-{index:04}.dat")), []).unwrap();
        }
        let discovery = discover_directory_with_progress(
            directory.path(),
            options,
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        (directory, discovery)
    }

    #[tokio::test]
    async fn visible_metadata_demand_is_bounded_and_deduplicated() {
        let options = ScanOptions::default();
        let (directory, discovery) = discovered_fixture(300, options.clone()).await;
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        let request = browser.next_directory_load_request(directory.path().to_path_buf());

        drop(browser.accept_directory_discovery(
            request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));
        let first_demand_count = browser.directory_metadata_in_flight.len();
        assert!(first_demand_count > 0);
        assert!(first_demand_count < 300);

        drop(browser.schedule_visible_directory_metadata(BrowserPaneId::PRIMARY, None));
        assert_eq!(
            browser.directory_metadata_in_flight.len(),
            first_demand_count
        );
    }

    #[tokio::test]
    async fn size_sort_demands_every_discovered_entry_once() {
        let options = ScanOptions {
            sort_field: SortField::Size,
            sort_direction: SortDirection::Ascending,
            ..ScanOptions::default()
        };
        let (directory, discovery) = discovered_fixture(300, options.clone()).await;
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        let request = browser.next_directory_load_request(directory.path().to_path_buf());

        drop(browser.accept_directory_discovery(
            request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        assert_eq!(browser.directory_metadata_in_flight.len(), 300);
        assert!(browser.directory_metadata_in_flight.iter().all(|key| {
            key.requirement == DirectoryMetadataRequirement::Filesystem
                && matches!(
                    key.context,
                    DirectoryMetadataLoadContext::Root {
                        collection_generation,
                        ..
                    } if collection_generation == browser.directory_load_generation
                )
        }));
    }

    #[tokio::test]
    async fn empty_size_sort_is_ready_without_metadata_request() {
        let options = ScanOptions {
            sort_field: SortField::Size,
            sort_direction: SortDirection::Ascending,
            ..ScanOptions::default()
        };
        let (directory, discovery) = discovered_fixture(0, options.clone()).await;
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        let request = browser.next_directory_load_request(directory.path().to_path_buf());

        drop(browser.accept_directory_discovery(
            request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        assert_eq!(
            browser.directory_collection_phase,
            crate::model::DirectoryCollectionPhase::Ready
        );
        assert!(browser.directory_order_phase.is_ready());
        assert!(browser.directory_metadata_in_flight.is_empty());
    }

    #[tokio::test]
    async fn size_sort_waits_for_metadata_before_committing_order() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a-large.dat"), vec![0_u8; 100]).unwrap();
        std::fs::write(directory.path().join("z-small.dat"), [0_u8]).unwrap();
        let options = ScanOptions {
            sort_field: SortField::Size,
            sort_direction: SortDirection::Ascending,
            ..ScanOptions::default()
        };
        let discovery = discover_directory_with_progress(
            directory.path(),
            options.clone(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let resolver = discovery.metadata_resolver.clone();
        resolve_directory_metadata(
            resolver.clone(),
            DirectoryMetadataRequest {
                request_generation: 98,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        let load_request = browser.next_directory_load_request(directory.path().to_path_buf());
        let collection_generation = load_request.generation;

        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));
        assert_eq!(
            browser.directory_collection_phase,
            crate::model::DirectoryCollectionPhase::Ready
        );
        let crate::model::DirectoryOrderPhase::WaitingForMetadata {
            request_generation, ..
        } = browser.directory_order_phase
        else {
            panic!("size order must wait for metadata");
        };
        let targets = vec![0, 1];
        let stale_request_generation = request_generation + 1;
        let stale_resolution = resolve_directory_metadata(
            resolver.clone(),
            DirectoryMetadataRequest {
                request_generation: stale_request_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: targets.clone(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        drop(browser.accept_directory_metadata_resolution(
            DirectoryMetadataLoadRequest {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: BrowserPaneId::PRIMARY,
                    path: directory.path().to_path_buf(),
                    collection_generation,
                },
                request_generation: stale_request_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: targets.clone(),
            },
            Ok(stale_resolution),
        ));
        assert!(matches!(
            browser.directory_order_phase,
            crate::model::DirectoryOrderPhase::WaitingForMetadata {
                request_generation: expected,
                ..
            } if expected == request_generation
        ));

        let resolution = resolve_directory_metadata(
            resolver,
            DirectoryMetadataRequest {
                request_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: targets.clone(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();

        drop(browser.accept_directory_metadata_resolution(
            DirectoryMetadataLoadRequest {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: BrowserPaneId::PRIMARY,
                    path: directory.path().to_path_buf(),
                    collection_generation,
                },
                request_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets,
            },
            Ok(resolution),
        ));

        assert!(browser.directory_order_phase.is_ready());
        assert_eq!(
            browser
                .entries
                .iter()
                .map(|entry| entry.name.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["z-small.dat", "a-large.dat"]
        );
    }

    #[tokio::test]
    async fn stale_metadata_completion_cannot_change_new_collection_phase() {
        let options = ScanOptions::default();
        let (directory, discovery) = discovered_fixture(1, options.clone()).await;
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        let load_request = browser.next_directory_load_request(directory.path().to_path_buf());
        let stale_collection_generation = load_request.generation;
        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        let replacement = directory.path().join("replacement");
        std::fs::create_dir(&replacement).unwrap();
        browser.current_dir = replacement.clone();
        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Discovering;
        let _ = browser.next_directory_load_request(replacement);
        drop(browser.accept_directory_metadata_resolution(
            DirectoryMetadataLoadRequest {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: BrowserPaneId::PRIMARY,
                    path: directory.path().to_path_buf(),
                    collection_generation: stale_collection_generation,
                },
                request_generation: 1,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            Ok(file_core::DirectoryMetadataResolution {
                request_generation: 1,
                requirement: DirectoryMetadataRequirement::Filesystem,
                requested_targets: 1,
                resolved_indices: vec![0],
                warnings: Vec::new(),
                filesystem_calls: 1,
                user_name_lookups: 0,
                group_name_lookups: 0,
                identity_worker_runs: 0,
            }),
        ));

        assert_eq!(
            browser.directory_collection_phase,
            crate::model::DirectoryCollectionPhase::Discovering
        );
        assert!(browser.directory_discovery.is_none());
    }

    #[tokio::test]
    async fn owner_column_requests_identity_names_without_offscreen_filesystem_demand() {
        let options = ScanOptions::default();
        let (directory, discovery) = discovered_fixture(2, options.clone()).await;
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();
        for column in [
            ListColumnKind::Modified,
            ListColumnKind::Size,
            ListColumnKind::Permissions,
            ListColumnKind::Accessed,
            ListColumnKind::Created,
        ] {
            browser
                .user_config
                .list_view_preferences
                .set_column_visible(column, false);
        }
        browser
            .user_config
            .list_view_preferences
            .set_column_visible(ListColumnKind::Owner, true);
        let load_request = browser.next_directory_load_request(directory.path().to_path_buf());
        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));

        drop(browser.schedule_visible_directory_metadata(BrowserPaneId::PRIMARY, None));

        assert!(!browser
            .directory_metadata_in_flight
            .iter()
            .any(|key| key.requirement == DirectoryMetadataRequirement::Filesystem));
        assert!(browser
            .directory_metadata_in_flight
            .iter()
            .any(|key| key.requirement == DirectoryMetadataRequirement::IdentityNames));
    }

    #[tokio::test]
    async fn expanded_metadata_uses_its_own_discovery_owner() {
        let root = tempfile::tempdir().unwrap();
        let expanded_path = root.path().join("expanded");
        std::fs::create_dir(&expanded_path).unwrap();
        std::fs::write(expanded_path.join("child.dat"), vec![0_u8; 100]).unwrap();
        let root_discovery = discover_directory_with_progress(
            root.path(),
            ScanOptions::default(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let expanded_discovery = discover_directory_with_progress(
            &expanded_path,
            ScanOptions::default(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.current_dir = root.path().to_path_buf();
        let load_request = browser.next_directory_load_request(root.path().to_path_buf());
        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(root_discovery),
        ));
        let expanded_entries =
            crate::model::display_entries_in_discovery_order(&expanded_discovery);
        browser.expanded_directories.insert(
            expanded_path.clone(),
            crate::model::ExpandedDirectory {
                entries: expanded_entries,
                directory_discovery: Some(expanded_discovery.clone()),
                status: ExpandedDirectoryStatus::Loaded,
                is_expanded: true,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 7,
                load_context: None,
                load_cancel: Some(CancellationToken::new()),
                directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                    field: SortField::Name,
                    direction: SortDirection::Ascending,
                },
            },
        );
        browser.directory_metadata_in_flight.clear();

        drop(browser.schedule_visible_directory_metadata(BrowserPaneId::PRIMARY, None));

        assert!(browser
            .directory_metadata_in_flight
            .iter()
            .any(|key| { matches!(key.context, DirectoryMetadataLoadContext::Root { .. }) }));
        assert!(browser.directory_metadata_in_flight.iter().any(|key| {
            matches!(
                &key.context,
                DirectoryMetadataLoadContext::Expanded { path, .. } if path == &expanded_path
            )
        }));
        resolve_directory_metadata(
            expanded_discovery.metadata_resolver.clone(),
            DirectoryMetadataRequest {
                request_generation: 99,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
        let child_entry = &pane.expanded_directories[&expanded_path].entries[0];

        assert_eq!(child_entry.discovery_index, Some(0));
        assert_eq!(pane.metadata_for_entry(child_entry).len, 100);
    }

    #[tokio::test]
    async fn current_metadata_warning_remains_user_visible() {
        let directory = tempfile::tempdir().unwrap();
        let file_path = directory.path().join("removed.dat");
        std::fs::write(&file_path, b"data").unwrap();
        let discovery = discover_directory_with_progress(
            directory.path(),
            ScanOptions::default(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let resolver = discovery.metadata_resolver.clone();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.current_dir = directory.path().to_path_buf();
        let load_request = browser.next_directory_load_request(directory.path().to_path_buf());
        let collection_generation = load_request.generation;
        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));
        std::fs::remove_file(file_path).unwrap();
        let resolution = resolve_directory_metadata(
            resolver,
            DirectoryMetadataRequest {
                request_generation: 1,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();

        drop(browser.accept_directory_metadata_resolution(
            DirectoryMetadataLoadRequest {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: BrowserPaneId::PRIMARY,
                    path: directory.path().to_path_buf(),
                    collection_generation,
                },
                request_generation: 1,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            Ok(resolution),
        ));

        assert!(browser.current_error().is_some());
    }

    #[tokio::test]
    async fn metadata_commit_reschedules_thumbnails_with_real_entry_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let image_path = directory.path().join("a-photo.png");
        std::fs::write(&image_path, vec![0_u8; 64]).unwrap();
        let options = ScanOptions {
            sort_field: SortField::Size,
            sort_direction: SortDirection::Ascending,
            ..ScanOptions::default()
        };
        let discovery = discover_directory_with_progress(
            directory.path(),
            options.clone(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        let resolver = discovery.metadata_resolver.clone();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.view_mode = BrowserViewMode::List;
        browser.options = options;
        browser.current_dir = directory.path().to_path_buf();

        // 预占全部解码槽位：commit 内部的 pump 取不走新请求，才能断言入队结果。
        let max_slots = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        let mut filler_keys = Vec::new();
        for index in 0..max_slots {
            let filler_request = thumbnails::ThumbnailRequest::new(
                PathBuf::from(format!("/filler-{index}.png")),
                thumbnails::ThumbnailSourceMetadata {
                    len: index as u64,
                    modified: None,
                },
                crate::thumbnail_cache::LIST_THUMBNAIL_EDGE,
            );
            filler_keys.push(filler_request.key());
            browser.thumbnail_cache.enqueue_request(
                filler_request,
                crate::thumbnail_cache::ThumbnailPurpose::List,
                crate::thumbnail_cache::ThumbnailPriority::Visible,
            );
        }
        drop(browser.thumbnail_cache.take_next_batch());

        let load_request = browser.next_directory_load_request(directory.path().to_path_buf());
        let collection_generation = load_request.generation;
        drop(browser.accept_directory_discovery(
            load_request,
            crate::model::PrebuiltDirectoryDiscovery::build(discovery),
        ));
        // 权威扫描提交后条目元数据是 Pending，自动调度的缩略图请求基于空值 key。
        assert_eq!(
            browser.entries[0].metadata.filesystem_availability,
            file_core::DirectoryMetadataAvailability::Pending
        );
        let stale_key = crate::thumbnail_cache::request_for_entry(
            &browser.entries[0],
            crate::thumbnail_cache::LIST_THUMBNAIL_EDGE,
        )
        .expect("pending request")
        .key();

        let expected_generation = match browser.directory_order_phase {
            crate::model::DirectoryOrderPhase::WaitingForMetadata {
                request_generation, ..
            } => request_generation,
            other => panic!("size sort must wait for metadata, got {other:?}"),
        };
        let resolution = resolve_directory_metadata(
            resolver,
            DirectoryMetadataRequest {
                request_generation: expected_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        drop(browser.accept_directory_metadata_resolution(
            DirectoryMetadataLoadRequest {
                context: DirectoryMetadataLoadContext::Root {
                    pane_id: BrowserPaneId::PRIMARY,
                    path: directory.path().to_path_buf(),
                    collection_generation,
                },
                request_generation: expected_generation,
                requirement: DirectoryMetadataRequirement::Filesystem,
                targets: vec![0],
            },
            Ok(resolution),
        ));

        // 排序提交重建条目后元数据落地，且必须已经用真实 key 重新入队缩略图。
        assert_eq!(
            browser.entries[0].metadata.filesystem_availability,
            file_core::DirectoryMetadataAvailability::Complete
        );
        let current_request = crate::thumbnail_cache::request_for_entry(
            &browser.entries[0],
            crate::thumbnail_cache::LIST_THUMBNAIL_EDGE,
        )
        .expect("current request");
        assert_ne!(current_request.key(), stale_key);

        browser.thumbnail_cache.finish(&filler_keys[0]);
        let requeued = browser.thumbnail_cache.take_next_batch();
        assert_eq!(requeued.len(), 1);
        assert_eq!(requeued[0].request.source, image_path);
        assert_eq!(requeued[0].request.metadata.len, 64);
        assert_eq!(requeued[0].key(), current_request.key());
    }
}
