#[path = "icon_grid_expansion_follow.rs"]
mod follow;
#[cfg(test)]
#[path = "icon_grid_sibling_switch_tests.rs"]
mod sibling_switch_tests;
#[cfg(test)]
#[path = "icon_grid_expansion_tests.rs"]
mod tests;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::{DirectoryDiscoveryBatch, DirectoryEntry, FileKind};
use iced::Task;

use super::panes::BrowserPaneView;
use super::FileBrowser;
use crate::commands::load_expanded_directory_command;
use crate::icon_grid_layout::IconGridLayout;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext, DirectoryLoadFailure,
    ExpandedDirectory, ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus,
    IconGridAnchorReconciliation, IconGridChildSwitch, IconGridExpansionAnchor,
    IconGridExpansionContext, IconGridExpansionSessionId, IconGridExpansionState,
    IconGridRemovedPathReconciliation, Message,
};

const ICON_GRID_EXPANSION_ANIMATION_STEP: f32 = 0.18;

impl FileBrowser {
    pub(crate) fn icon_grid_layout_for_pane<'a>(
        &'a self,
        pane: BrowserPaneView<'a>,
    ) -> IconGridLayout<'a> {
        let root_placeholders = crate::transfer_placeholders::transfer_placeholders_for_directory(
            &self.operation_queue,
            pane.current_dir,
        );
        self.icon_grid_layout_for_pane_with_placeholders(pane, &root_placeholders)
    }

    /// 空目录但有传入占位时,视图用预派生的占位集合建布局,避免
    /// 「No items」空态与布局各查一遍队列导致两帧之间闪烁。
    pub(crate) fn icon_grid_layout_for_pane_with_placeholders<'a>(
        &'a self,
        pane: BrowserPaneView<'a>,
        root_placeholders: &[crate::transfer_placeholders::TransferPlaceholder],
    ) -> IconGridLayout<'a> {
        let expansion = self.icon_grid_expansion.as_ref().filter(|state| {
            pane.view_mode == BrowserViewMode::Icons
                && state.context().pane_id == pane.id
                && state.context().current_dir == *pane.current_dir
        });
        // 展开子面板与根面板同一事实源:拖放进展开中的目录时,band 内
        // 占位按该目录的排序出现,与列表/多栏行为一致。
        let mut expanded_placeholders =
            crate::icon_grid_layout::ExpandedTransferPlaceholderIndex::new();
        if let Some(expansion) = expansion {
            for (path, directory) in expansion
                .directories()
                .filter(|(_, directory)| directory.is_visible())
            {
                let placeholders =
                    crate::transfer_placeholders::transfer_placeholders_for_directory(
                        &self.operation_queue,
                        path,
                    );
                if placeholders.is_empty() {
                    continue;
                }
                expanded_placeholders.insert(
                    path.to_path_buf(),
                    (
                        placeholders,
                        crate::transfer_placeholder_view::transfer_sort_for_expanded(
                            self,
                            &directory.contents,
                        ),
                    ),
                );
            }
        }
        // 大图根面板与列表共用同一分组划分器:分组开启时根 cells 先按
        // 目录置顶、文件段按组划分;子面板/band 永远平铺。元数据解析
        // 绑定 pane 的显示同源入口,分组键与列表行流同源。
        let root_grouping =
            crate::transfer_placeholder_view::root_grouping::RootGrouping::for_pane_root(
                self,
                pane,
                pane.current_dir.as_path(),
            );
        let metadata_for_entry = |entry: &DirectoryEntry| pane.metadata_for_entry(entry);
        let root_grouping = root_grouping.as_ref().map(|grouping| {
            crate::icon_grid_layout::IconGridRootGrouping {
                grouping,
                metadata_for_entry: &metadata_for_entry,
            }
        });
        IconGridLayout::with_root_transfer_placeholders(
            pane.current_dir,
            pane.entries,
            root_placeholders,
            crate::transfer_placeholder_view::transfer_sort_for_root(self),
            &expanded_placeholders,
            pane.icon_grid_viewport.width,
            self.main_window_height,
            self.user_config.icons_icon_edge(),
            expansion,
            root_grouping,
        )
    }

    pub(crate) fn icon_grid_disclosure(
        &self,
        pane_id: BrowserPaneId,
        current_dir: &Path,
        path: &Path,
    ) -> Option<(f32, bool)> {
        let state = self.icon_grid_expansion.as_ref().filter(|state| {
            state.context().pane_id == pane_id && state.context().current_dir == current_dir
        })?;
        if state.path_is_pending(path) {
            return Some((0.0, false));
        }
        let expanded = state.directory(path)?;
        let is_open = expanded.contents.is_expanded && !expanded.contents.is_collapsing;
        let rotation = expanded.contents.animation_progress.clamp(0.0, 1.0) * 90.0;
        Some((rotation, is_open))
    }

    pub(super) fn toggle_icon_grid_directory(
        &mut self,
        pane_id: BrowserPaneId,
        anchor: IconGridExpansionAnchor,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        if !self.icon_grid_anchor_is_valid(pane_id, &anchor) {
            return Task::none();
        }
        self.cancel_expansion_follow_plans();

        let rename_command = self.commit_rename_if_active();
        let expansion_command = self.toggle_valid_icon_grid_directory(anchor);
        Task::batch([rename_command, expansion_command])
    }

    fn toggle_valid_icon_grid_directory(
        &mut self,
        anchor: IconGridExpansionAnchor,
    ) -> Task<Message> {
        if self
            .icon_grid_expansion
            .as_ref()
            .is_some_and(|state| !self.icon_grid_state_matches_active_context(state))
        {
            self.clear_icon_grid_expansion();
        }

        if let Some(expanded) = self
            .icon_grid_expansion
            .as_ref()
            .and_then(|state| state.directory(&anchor.path))
        {
            if expanded.contents.is_expanded && !expanded.contents.is_collapsing {
                return self.close_icon_grid_directory(anchor.path);
            }

            let loading_paths = self
                .icon_grid_expansion
                .as_mut()
                .map(|state| {
                    state.reopen_directory(&anchor.path);
                    state.loading_subtree_paths(&anchor.path)
                })
                .unwrap_or_default();
            if loading_paths.is_empty() {
                return self.schedule_thumbnail_refresh();
            }
            return Task::batch(
                loading_paths
                    .into_iter()
                    .map(|path| self.reload_icon_grid_directory(path)),
            );
        }

        if anchor.parent_directory == self.current_dir {
            if self.icon_grid_expansion.is_some() {
                let (hidden_paths, closed_immediately, fallback) = {
                    let state = self
                        .icon_grid_expansion
                        .as_mut()
                        .expect("checked icon grid expansion");
                    let fallback = state.root_path().to_path_buf();
                    let hidden_paths = state.begin_root_replacement(anchor);
                    (hidden_paths, state.root_is_closed(), fallback)
                };
                self.retain_selection_after_arrow_collapse(&hidden_paths, fallback);
                return if closed_immediately {
                    self.finish_closed_icon_grid_root()
                } else {
                    self.schedule_thumbnail_refresh()
                };
            }
            return self.start_icon_grid_root(anchor);
        }

        let child_switch = self
            .icon_grid_expansion
            .as_mut()
            .map(|state| state.begin_child_switch(anchor))
            .unwrap_or(IconGridChildSwitch {
                hidden_paths: Vec::new(),
                closing_path: None,
                ready_child: None,
            });
        let had_closing_child = child_switch.closing_path.is_some();
        if let Some(fallback) = child_switch.closing_path {
            self.retain_selection_after_arrow_collapse(&child_switch.hidden_paths, fallback);
        }
        match child_switch.ready_child {
            Some(ready_child) => self.start_icon_grid_child(ready_child),
            None if had_closing_child => self.schedule_thumbnail_refresh(),
            None => Task::none(),
        }
    }

    fn start_icon_grid_root(&mut self, anchor: IconGridExpansionAnchor) -> Task<Message> {
        let pane_id = self.active_pane_id();
        if !self.icon_grid_anchor_is_valid(pane_id, &anchor)
            || anchor.parent_directory != self.current_dir
        {
            return Task::none();
        }

        let context = IconGridExpansionContext {
            pane_id,
            current_dir: self.current_dir.clone(),
            session_id: self.next_icon_grid_expansion_session_id(),
        };
        let mut expanded = loading_icon_grid_directory();
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            icon_grid_load_context(&context),
            anchor.path.clone(),
            &mut expanded,
        );
        self.icon_grid_expansion = Some(IconGridExpansionState::new(context, anchor, expanded));
        load_expanded_directory_command(request, self.options.clone(), cancellation)
    }

    fn start_icon_grid_child(&mut self, anchor: IconGridExpansionAnchor) -> Task<Message> {
        let Some(context) = self
            .icon_grid_expansion
            .as_ref()
            .map(|state| state.context().clone())
        else {
            return Task::none();
        };
        let mut expanded = loading_icon_grid_directory();
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            icon_grid_load_context(&context),
            anchor.path.clone(),
            &mut expanded,
        );
        let inserted = self
            .icon_grid_expansion
            .as_mut()
            .is_some_and(|state| state.insert_directory(anchor, expanded));
        if !inserted {
            cancellation.cancel();
            if let Some(state) = self.icon_grid_expansion.as_mut() {
                state.cancel_follow_plan();
            }
            return Task::none();
        }
        load_expanded_directory_command(request, self.options.clone(), cancellation)
    }

    fn reload_icon_grid_directory(&mut self, path: PathBuf) -> Task<Message> {
        let Some((context, hidden_paths)) = self
            .icon_grid_expansion
            .as_ref()
            .map(|state| (state.context().clone(), state.entry_paths_in_subtree(&path)))
        else {
            return Task::none();
        };
        let Some(expanded) = self
            .icon_grid_expansion
            .as_mut()
            .and_then(|state| state.directory_mut(&path))
        else {
            return Task::none();
        };
        expanded.contents.status = ExpandedDirectoryStatus::Loading;
        expanded.contents.entries.clear();
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            icon_grid_load_context(&context),
            path,
            &mut expanded.contents,
        );
        self.remove_hidden_icon_grid_selection(&hidden_paths);
        load_expanded_directory_command(request, self.options.clone(), cancellation)
    }

    fn close_icon_grid_directory(&mut self, path: PathBuf) -> Task<Message> {
        let (hidden_paths, root_closed, fallback) = {
            let Some(state) = self.icon_grid_expansion.as_mut() else {
                return Task::none();
            };
            let hidden_paths = state.begin_directory_dismissal(&path);
            (hidden_paths, state.root_is_closed(), path)
        };
        self.retain_selection_after_arrow_collapse(&hidden_paths, fallback);
        if root_closed {
            return self.finish_closed_icon_grid_root();
        }
        self.schedule_thumbnail_refresh()
    }

    pub(super) fn press_icon_grid_panel(
        &mut self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
            return Task::none();
        }
        self.cancel_expansion_follow_plans();
        self.start_icon_grid_panel_selection_marquee(directory)
    }

    pub(super) fn prepare_icon_grid_entry_interaction(&mut self, path: &Path) -> Task<Message> {
        if self.view_mode != BrowserViewMode::Icons {
            return Task::none();
        }
        self.cancel_expansion_follow_plans();
        let parent_directory = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.current_dir.clone());
        let path_belongs_to_tree = self.icon_grid_expansion.as_mut().is_some_and(|state| {
            let belongs = state.contains_tree_path(path);
            state.set_selection_directory(&parent_directory);
            belongs
        });
        if path_belongs_to_tree {
            Task::none()
        } else {
            self.dismiss_icon_grid_expansion_from_outside()
        }
    }

    pub(super) fn dismiss_icon_grid_expansion_from_outside(&mut self) -> Task<Message> {
        let (hidden_paths, root_closed) = {
            let Some(state) = self.icon_grid_expansion.as_mut() else {
                return Task::none();
            };
            let hidden_paths = state.begin_root_dismissal();
            (hidden_paths, state.root_is_closed())
        };
        self.retain_selection_before_outside_interaction(&hidden_paths);
        if root_closed {
            return self.finish_closed_icon_grid_root();
        }
        self.schedule_thumbnail_refresh()
    }

    pub(super) fn escape_icon_grid_expansion(&mut self) -> Option<Task<Message>> {
        let (root_path, root_closed) = {
            let state = self.icon_grid_expansion.as_mut()?;
            let root_path = state.root_path().to_path_buf();
            state.begin_root_dismissal();
            (root_path, state.root_is_closed())
        };
        self.select_path(root_path);
        self.selection_marquee = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        Some(if root_closed {
            self.finish_closed_icon_grid_root()
        } else {
            self.schedule_thumbnail_refresh()
        })
    }

    pub(super) fn clear_icon_grid_expansion(&mut self) {
        if let Some(mut state) = self.icon_grid_expansion.take() {
            let root_path = state.root_path().to_path_buf();
            let hidden_paths = state.entry_paths_in_subtree(&root_path);
            state.cancel_all_loads();
            self.remove_hidden_icon_grid_selection(&hidden_paths);
        }
    }

    pub(super) fn clear_icon_grid_expansion_for_context_change(&mut self) {
        self.cancel_expansion_follow_plans();
        let had_expansion = self.icon_grid_expansion.is_some();
        self.clear_icon_grid_expansion();
        if had_expansion {
            self.retain_direct_entry_selection();
        }
    }

    pub(super) fn icon_grid_expansion_animation_is_active(&self) -> bool {
        self.icon_grid_expansion
            .as_ref()
            .is_some_and(IconGridExpansionState::animation_is_active)
    }

    pub(super) fn advance_icon_grid_expansion_animation(&mut self) -> Task<Message> {
        let Some(advance) = self
            .icon_grid_expansion
            .as_mut()
            .map(|state| state.advance_animations(ICON_GRID_EXPANSION_ANIMATION_STEP))
        else {
            return Task::none();
        };
        if advance.root_closed {
            return self.finish_closed_icon_grid_root();
        }

        let mut commands = advance
            .ready_children
            .into_iter()
            .map(|anchor| self.start_icon_grid_child(anchor))
            .collect::<Vec<_>>();
        if advance.entries_became_interactive {
            commands.push(self.schedule_thumbnail_refresh());
        }
        commands.push(self.advance_icon_grid_expansion_follow());
        if commands.is_empty() {
            Task::none()
        } else {
            Task::batch(commands)
        }
    }

    fn finish_closed_icon_grid_root(&mut self) -> Task<Message> {
        let pending_root = self.icon_grid_expansion.take().and_then(|mut state| {
            state.cancel_all_loads();
            state.take_pending_root()
        });
        match pending_root {
            Some(anchor) => self.start_icon_grid_root(anchor),
            None => self.schedule_thumbnail_refresh(),
        }
    }

    pub(super) fn accept_icon_grid_directory_discovery_batch(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        batch: DirectoryDiscoveryBatch,
    ) -> Task<Message> {
        if !self.icon_grid_request_is_current(&request) {
            return Task::none();
        }
        let mut entries = batch
            .entries
            .iter()
            .map(file_core::DiscoveredDirectoryEntry::display_entry)
            .collect::<Vec<_>>();
        file_core::sort_entries(&mut entries, &self.options);
        let Some(expanded) = self
            .icon_grid_expansion
            .as_mut()
            .and_then(|state| state.directory_mut(&request.path))
        else {
            return Task::none();
        };
        expanded.contents.entries.extend(entries);
        Task::none()
    }

    #[cfg(test)]
    pub(super) fn accept_complete_icon_grid_directory_fixture(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        scan: Result<file_core::DirectoryScan, DirectoryLoadFailure>,
    ) -> Task<Message> {
        self.accept_icon_grid_directory_entries(request, scan.map(|scan| scan.entries))
    }

    pub(super) fn accept_icon_grid_directory_entries(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        entries: Result<Vec<DirectoryEntry>, DirectoryLoadFailure>,
    ) -> Task<Message> {
        if !self.icon_grid_request_is_current(&request) {
            return Task::none();
        }

        match entries {
            Ok(entries) => {
                let Some(state) = self.icon_grid_expansion.as_mut() else {
                    return Task::none();
                };
                state.reconcile_child_anchors(&request.path, &entries);
                let Some(expanded) = state.directory_mut(&request.path) else {
                    return Task::none();
                };
                expanded.contents.entries = entries;
                expanded.contents.status = ExpandedDirectoryStatus::Loaded;
                expanded.contents.load_context = None;
                expanded.contents.load_cancel = None;
            }
            Err(DirectoryLoadFailure::DirectoryUnavailable { .. }) => {
                self.reconcile_icon_grid_removed_paths(std::slice::from_ref(&request.path));
                return Task::batch([
                    self.schedule_thumbnail_refresh(),
                    self.remeasure_active_file_drop_layout(),
                ]);
            }
            Err(DirectoryLoadFailure::ReadFailed { message }) => {
                let Some(expanded) = self
                    .icon_grid_expansion
                    .as_mut()
                    .and_then(|state| state.directory_mut(&request.path))
                else {
                    return Task::none();
                };
                expanded.contents.entries.clear();
                expanded.contents.status = ExpandedDirectoryStatus::Error;
                expanded.contents.load_context = None;
                expanded.contents.load_cancel = None;
                if let Some(state) = self.icon_grid_expansion.as_mut() {
                    state.cancel_follow_plan();
                }
                self.show_global_error(message);
            }
        }

        self.retain_icon_grid_visible_selection();
        Task::batch([
            self.advance_icon_grid_expansion_follow(),
            self.schedule_thumbnail_refresh(),
            self.remeasure_active_file_drop_layout(),
        ])
    }

    pub(super) fn reconcile_icon_grid_removed_paths(&mut self, removed_paths: &[PathBuf]) {
        if let Some(state) = self.icon_grid_expansion.as_mut() {
            state.cancel_follow_plan();
        }
        let reconciliation = self
            .icon_grid_expansion
            .as_mut()
            .map(|state| state.reconcile_removed_paths(removed_paths));
        match reconciliation {
            Some(IconGridRemovedPathReconciliation::RootRemoved) => {
                self.clear_icon_grid_expansion();
                self.retain_direct_entry_selection();
            }
            Some(IconGridRemovedPathReconciliation::Retained { hidden_paths }) => {
                self.remove_hidden_icon_grid_selection(&hidden_paths);
            }
            None => {}
        }
    }

    pub(super) fn reconcile_icon_grid_root_after_scan(&mut self, entries: &[DirectoryEntry]) {
        let pane_id = self.active_pane_id();
        let current_dir = self.current_dir.clone();
        let root_removed = self
            .icon_grid_expansion
            .as_mut()
            .filter(|state| {
                state.context().pane_id == pane_id && state.context().current_dir == current_dir
            })
            .is_some_and(|state| {
                state.reconcile_child_anchors(&current_dir, entries)
                    == IconGridAnchorReconciliation::RootRemoved
            });
        if root_removed {
            self.clear_icon_grid_expansion();
        }
    }

    pub(super) fn retain_icon_grid_visible_selection(&mut self) {
        if self.selected.is_none()
            && self.selected_paths.is_empty()
            && self.selection_anchor.is_none()
            && self.drag_selection_anchor.is_none()
            && self.pending_created_entry_rename.is_none()
            && self.renaming.is_none()
        {
            return;
        }

        let mut tracked_paths = Vec::with_capacity(
            self.selected_paths.len()
                + usize::from(self.selected.is_some())
                + usize::from(self.selection_anchor.is_some())
                + usize::from(self.drag_selection_anchor.is_some())
                + usize::from(self.pending_created_entry_rename.is_some())
                + usize::from(self.renaming.is_some()),
        );
        if let Some(path) = &self.selected {
            tracked_paths.push(path.clone());
        }
        tracked_paths.extend(self.selected_paths.iter().cloned());
        if let Some(path) = &self.selection_anchor {
            tracked_paths.push(path.clone());
        }
        if let Some(path) = &self.drag_selection_anchor {
            tracked_paths.push(path.clone());
        }
        if let Some(path) = &self.pending_created_entry_rename {
            tracked_paths.push(path.clone());
        }
        if let Some(path) = &self.renaming {
            tracked_paths.push(path.clone());
        }

        let visible_paths = self
            .pane_view(self.active_pane_id())
            .map(|pane| {
                let layout = self.icon_grid_layout_for_pane(pane);
                layout.interactive_paths_matching(&tracked_paths)
            })
            .unwrap_or_else(|| {
                self.entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect()
            });
        let primary_selection_was_hidden = self
            .selected
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path));
        self.selected_paths
            .retain(|path| visible_paths.contains(path));
        if primary_selection_was_hidden {
            self.selected = self.selected_paths.iter().min().cloned();
            self.clear_preview();
        }
        if self
            .selection_anchor
            .as_ref()
            .is_some_and(|path| !self.selected_paths.contains(path))
        {
            self.selection_anchor = None;
        }
        if self
            .drag_selection_anchor
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path))
        {
            self.drag_selection_anchor = None;
        }
        if self
            .pending_created_entry_rename
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path))
        {
            self.pending_created_entry_rename = None;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|path| !visible_paths.contains(path))
        {
            self.renaming = None;
            self.rename_input.clear();
        }
    }

    pub(super) fn refresh_icon_grid_expansion_commands(&mut self) -> Vec<Task<Message>> {
        if !self
            .icon_grid_expansion
            .as_ref()
            .is_some_and(|state| self.icon_grid_state_matches_active_context(state))
        {
            return Vec::new();
        }
        let paths = self
            .icon_grid_expansion
            .as_ref()
            .map(|state| {
                state
                    .directories()
                    .filter(|(path, _)| state.accepts_directory_load(path))
                    .map(|(path, _)| path.to_path_buf())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        paths
            .into_iter()
            .map(|path| self.reload_icon_grid_directory(path))
            .collect()
    }

    pub(super) fn reload_observed_icon_grid_directory(
        &mut self,
        path: &Path,
    ) -> Option<Task<Message>> {
        self.icon_grid_expansion
            .as_ref()
            .filter(|state| state.accepts_directory_load(path))?;
        Some(self.reload_icon_grid_directory(path.to_path_buf()))
    }

    pub(super) fn icon_grid_expansion_watch_directories(&self) -> Vec<PathBuf> {
        self.icon_grid_expansion
            .as_ref()
            .filter(|state| self.icon_grid_state_matches_active_context(state))
            .map(|state| {
                state
                    .directories()
                    .filter(|(path, _)| state.accepts_directory_load(path))
                    .map(|(path, _)| path.to_path_buf())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn icon_grid_request_is_current(&self, request: &ExpandedDirectoryLoadRequest) -> bool {
        let DirectoryExpansionLoadContext::IconGrid {
            pane_id,
            current_dir,
            session_id,
        } = &request.context
        else {
            return false;
        };
        *pane_id == self.active_pane_id()
            && self.view_mode == BrowserViewMode::Icons
            && current_dir == &self.current_dir
            && self.icon_grid_expansion.as_ref().is_some_and(|state| {
                state.matches_context(*pane_id, current_dir, *session_id)
                    && state.accepts_directory_load(&request.path)
                    && state.directory(&request.path).is_some_and(|expanded| {
                        expanded.contents.load_generation == request.generation
                            && expanded.contents.load_context.as_ref() == Some(&request.context)
                    })
            })
    }

    fn icon_grid_anchor_is_valid(
        &self,
        pane_id: BrowserPaneId,
        anchor: &IconGridExpansionAnchor,
    ) -> bool {
        if pane_id != self.active_pane_id()
            || self.view_mode != BrowserViewMode::Icons
            || self.is_trash_view
        {
            return false;
        }
        let entries = if anchor.parent_directory == self.current_dir {
            Some(self.entries.as_slice())
        } else {
            self.icon_grid_expansion
                .as_ref()
                .and_then(|state| state.entries_in_interactive_directory(&anchor.parent_directory))
        };
        entries
            .and_then(|entries| entries.get(anchor.index))
            .is_some_and(|entry| entry.path == anchor.path && entry.kind == FileKind::Directory)
    }

    fn icon_grid_state_matches_active_context(&self, state: &IconGridExpansionState) -> bool {
        self.view_mode == BrowserViewMode::Icons
            && state.context().pane_id == self.active_pane_id()
            && state.context().current_dir == self.current_dir
    }

    fn next_icon_grid_expansion_session_id(&mut self) -> IconGridExpansionSessionId {
        let session_id = IconGridExpansionSessionId::new(self.next_icon_grid_expansion_session_id);
        self.next_icon_grid_expansion_session_id =
            self.next_icon_grid_expansion_session_id.wrapping_add(1);
        session_id
    }

    fn retain_selection_after_arrow_collapse(
        &mut self,
        hidden_paths: &[PathBuf],
        fallback: PathBuf,
    ) {
        self.remove_hidden_icon_grid_selection(hidden_paths);
        if self.selected_paths.is_empty() {
            self.selected_paths.insert(fallback.clone());
            self.selected = Some(fallback.clone());
            self.selection_anchor = Some(fallback);
        }
    }

    fn retain_selection_before_outside_interaction(&mut self, hidden_paths: &[PathBuf]) {
        self.remove_hidden_icon_grid_selection(hidden_paths);
    }

    fn remove_hidden_icon_grid_selection(&mut self, hidden_paths: &[PathBuf]) {
        let hidden_paths = hidden_paths
            .iter()
            .map(PathBuf::as_path)
            .collect::<HashSet<_>>();
        let primary_selection_was_hidden = self
            .selected
            .as_ref()
            .is_some_and(|path| hidden_paths.contains(path.as_path()));
        self.selected_paths
            .retain(|path| !hidden_paths.contains(path.as_path()));
        if primary_selection_was_hidden {
            self.selected = self.selected_paths.iter().min().cloned();
            self.clear_preview();
        }
        if self
            .selection_anchor
            .as_ref()
            .is_some_and(|path| hidden_paths.contains(path.as_path()))
        {
            self.selection_anchor = None;
        }
        if self
            .drag_selection_anchor
            .as_ref()
            .is_some_and(|path| hidden_paths.contains(path.as_path()))
        {
            self.drag_selection_anchor = None;
        }
        if self
            .pending_created_entry_rename
            .as_ref()
            .is_some_and(|path| hidden_paths.contains(path.as_path()))
        {
            self.pending_created_entry_rename = None;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|path| hidden_paths.contains(path.as_path()))
        {
            self.renaming = None;
            self.rename_input.clear();
        }
    }
}

fn loading_icon_grid_directory() -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loading,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 0.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn icon_grid_load_context(context: &IconGridExpansionContext) -> DirectoryExpansionLoadContext {
    DirectoryExpansionLoadContext::IconGrid {
        pane_id: context.pane_id,
        current_dir: context.current_dir.clone(),
        session_id: context.session_id,
    }
}
