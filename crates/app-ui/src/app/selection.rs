use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use file_core::FileKind;
use iced::Task;

use super::panes::BrowserPaneView;
use super::{FileBrowser, DOUBLE_CLICK_THRESHOLD};

use crate::model::{
    BrowserPaneId, BrowserViewMode, ContextMenuState, FileContextMenuExpansion,
    FileContextMenuState, FileDeleteAction, FileDragStationaryAction, LastActivationClick, Message,
};
use crate::selection_summary::{summarize_selected_entries, PaneSelectionSummary};

#[cfg(test)]
mod activation_tests;
mod clipboard;
#[cfg(test)]
mod column_gesture_tests;
mod conflict;
mod drag;
mod entry_actions;
mod file_drop;
mod file_drop_target;
mod keyboard_navigation;
mod marquee;
#[cfg(test)]
mod tests;
mod visible_paths;
#[cfg(test)]
mod wayland_drop;
#[cfg(test)]
mod x11_drop;

#[cfg(test)]
use drag::resolve_file_drag_target;

impl FileBrowser {
    pub(crate) fn is_path_selected(&self, path: &Path) -> bool {
        self.selected_paths.contains(path)
    }

    pub(super) fn select_path(&mut self, path: PathBuf) {
        self.cancel_expansion_follow_plans();
        self.selected_paths.clear();
        self.selected_paths.insert(path.clone());
        self.selection_anchor = Some(path.clone());
        self.pending_keyboard_column_focus = None;
        self.focus_path(path);
    }

    /// 批量操作(复制副本等)完成后的整批选中:全部进入选中集,
    /// 主选中与焦点落在最后一个目标上(与 Finder 的批次选中一致)。
    pub(super) fn select_operation_result_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(anchor) = paths.first().cloned() else {
            return;
        };
        let Some(primary) = paths.last().cloned() else {
            return;
        };
        self.cancel_expansion_follow_plans();
        self.selected_paths = paths.into_iter().collect();
        self.selection_anchor = Some(anchor);
        self.pending_keyboard_column_focus = None;
        self.focus_path(primary);
    }

    pub(super) fn handle_column_entry_clicked(&mut self, path: PathBuf) -> Task<Message> {
        let directory = self.entry_parent_directory(&path);
        self.focus_column_from_pointer_click(directory);
        let column_directories_snapshot = crate::three_column_view::column_directories(self);
        self.handle_file_entry_clicked(
            path,
            FileDragStationaryAction::ActivateColumnEntry,
            column_directories_snapshot,
        )
    }

    pub(super) fn handle_flat_entry_clicked(&mut self, path: PathBuf) -> Task<Message> {
        let expansion_command = self.prepare_icon_grid_entry_interaction(&path);
        let column_directories_snapshot = Vec::new();
        Task::batch([
            expansion_command,
            self.handle_file_entry_clicked(
                path,
                FileDragStationaryAction::SelectionOnly,
                column_directories_snapshot,
            ),
        ])
    }

    fn handle_file_entry_clicked(
        &mut self,
        path: PathBuf,
        stationary_action: FileDragStationaryAction,
        column_directories_snapshot: Vec<PathBuf>,
    ) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let was_selected = self.is_path_selected(&path);
        let rename_command = self.commit_rename_if_active();

        let now = Instant::now();
        let has_selection_modifier =
            self.keyboard_modifiers.control() || self.keyboard_modifiers.shift();
        let is_double_click = !has_selection_modifier
            && self
                .last_activation_click
                .as_ref()
                .is_some_and(|last_activation_click| {
                    last_activation_click.path == path
                        && now.duration_since(last_activation_click.at) <= DOUBLE_CLICK_THRESHOLD
                });

        if self.keyboard_modifiers.shift() {
            let anchor = self
                .selection_anchor
                .clone()
                .or_else(|| self.selected.clone())
                .unwrap_or_else(|| path.clone());
            let range_anchor =
                self.select_range(anchor, path.clone(), self.keyboard_modifiers.control());
            self.selection_anchor = Some(range_anchor.clone());
            self.drag_selection_anchor = Some(range_anchor);
        } else if self.keyboard_modifiers.control() {
            self.toggle_path_selection(path.clone());
            self.selection_anchor = Some(path.clone());
            self.drag_selection_anchor = Some(path.clone());
            self.cancel_file_drag_interaction();
        } else {
            if was_selected && self.is_trash_view {
                self.select_path(path.clone());
            } else if was_selected {
                self.focus_path(path.clone());
                self.selection_anchor = Some(path.clone());
            } else {
                self.select_path(path.clone());
            }
            self.drag_selection_anchor = None;
            self.selection_marquee = None;
            self.start_file_drag(path.clone(), stationary_action, column_directories_snapshot);
        }

        self.last_activation_click = if has_selection_modifier {
            None
        } else {
            Some(LastActivationClick {
                path: path.clone(),
                at: now,
            })
        };

        let action_command = if is_double_click {
            self.drag_selection_anchor = None;
            self.cancel_file_drag_interaction();
            self.activate_path(path)
        } else {
            Task::none()
        };
        Task::batch([
            rename_command,
            action_command,
            self.schedule_thumbnail_refresh(),
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn handle_entry_hovered(&mut self, path: PathBuf) -> Task<Message> {
        // hover 是鼠标移动热路径：先保证条目索引可用，避免每次悬停线性扫描。
        self.refresh_entry_index();
        self.hovered_entry = Some(path.clone());
        self.cursor_paste_directory = Some(self.entry_parent_directory(&path));
        if self.file_drag.is_some() {
            self.set_file_drag_target(self.directory_drop_target_for_entry(&path));
        } else if self.selection_marquee.is_none() {
            self.extend_drag_selection_to(path);
        }
        self.schedule_thumbnail_refresh()
    }

    pub(super) fn handle_entry_hover_cleared(&mut self, path: PathBuf) -> Task<Message> {
        if self.hovered_entry.as_ref() == Some(&path) {
            self.hovered_entry = None;
            self.cursor_paste_directory = Some(self.entry_parent_directory(&path));
        }
        Task::none()
    }

    pub(super) fn handle_drop_target_hovered(&mut self, directory: PathBuf) -> Task<Message> {
        self.hovered_entry = None;
        if self.file_drag.is_some() {
            self.set_file_drag_target(directory);
        } else {
            self.cursor_paste_directory = Some(directory);
        }
        Task::none()
    }

    pub(super) fn handle_drop_target_hover_cleared(&mut self, directory: PathBuf) -> Task<Message> {
        if self.file_drag.is_none() && self.cursor_paste_directory.as_ref() == Some(&directory) {
            self.cursor_paste_directory = None;
        }
        self.clear_file_drag_target_if_matching(&directory);
        Task::none()
    }

    pub(super) fn handle_file_drag_entry_hovered_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        path: PathBuf,
    ) -> Task<Message> {
        if self.file_drag.is_none() {
            return Task::none();
        }

        let Some(target_directory) = self.directory_drop_target_for_entry_in_pane(pane_id, &path)
        else {
            self.clear_file_drag_target();
            return Task::none();
        };

        self.set_file_drag_target(target_directory);
        Task::none()
    }

    pub(super) fn handle_file_drag_entry_hover_cleared_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        path: PathBuf,
    ) -> Task<Message> {
        if self.file_drag.is_none() {
            return Task::none();
        }

        if let Some(target_directory) = self.directory_drop_target_for_entry_in_pane(pane_id, &path)
        {
            self.clear_file_drag_target_if_matching(&target_directory);
        }
        Task::none()
    }

    pub(super) fn handle_file_drag_drop_target_hovered_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
    ) -> Task<Message> {
        if self.file_drag.is_none() {
            return Task::none();
        }
        if !self.pane_accepts_file_drag(pane_id) {
            self.clear_file_drag_target();
            return Task::none();
        }

        self.set_file_drag_target(directory);
        Task::none()
    }

    pub(super) fn handle_file_drag_drop_target_hover_cleared_in_pane(
        &mut self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
    ) -> Task<Message> {
        if self.file_drag.is_some() && self.pane_accepts_file_drag(pane_id) {
            self.clear_file_drag_target_if_matching(&directory);
        }
        Task::none()
    }

    pub(super) fn handle_sidebar_hovered(&mut self, path: PathBuf) -> Task<Message> {
        self.hovered_sidebar = Some(path.clone());
        self.cursor_paste_directory = None;
        if self.file_drag.is_some() {
            self.set_file_drop_target(file_drop_target::sidebar_file_drop_target_for_directory(
                &path,
            ));
        }
        Task::none()
    }

    pub(super) fn handle_sidebar_hover_cleared(&mut self, path: PathBuf) -> Task<Message> {
        if self.hovered_sidebar.as_ref() == Some(&path) {
            self.hovered_sidebar = None;
        }
        self.clear_file_drag_target_if_matching(&path);
        Task::none()
    }

    pub(super) fn clear_cursor_paste_target(&mut self) -> Task<Message> {
        self.cursor_paste_directory = None;
        Task::none()
    }

    pub(super) fn handle_column_blank_clicked(&mut self, directory: PathBuf) -> Task<Message> {
        self.focus_column_from_pointer_click(directory.clone());
        let rename_command = self.commit_rename_if_active();
        self.clear_preview();
        self.context_menu = None;
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.focus_column_blank_context_or_clear_selection(directory);

        Task::batch([rename_command, self.request_browser_session_save()])
    }

    pub(super) fn handle_column_placeholder_pressed(&mut self) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        self.clear_preview();
        self.context_menu = None;
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.record_pane_drag_pointer_press();

        rename_command
    }

    pub(super) fn handle_entry_right_clicked(&mut self, path: PathBuf) -> Task<Message> {
        if self.view_mode == BrowserViewMode::Columns {
            self.record_pointer_clicked_column(self.entry_parent_directory(&path));
        }
        let expansion_command = self.prepare_icon_grid_entry_interaction(&path);
        let rename_command = self.commit_rename_if_active();
        self.select_context_menu_target(path.clone());
        let delete_action = self.file_delete_action_for_selection();
        self.clear_preview();
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        let target_is_directory = self.entry_kind(&path) == Some(FileKind::Directory);
        let can_batch_rename = self.batch_rename_available_for_selection();
        let can_create_symlink = self.selection_all_paths_local();
        if self.file_area_menu_is_visible(true, target_is_directory, can_batch_rename) {
            self.context_menu = Some(ContextMenuState::FileArea(FileContextMenuState {
                target: Some(path.clone()),
                target_is_directory,
                paste_directory: self.entry_parent_directory(&path),
                can_batch_rename,
                can_create_symlink,
                delete_action,
                position: self.cursor_position,
                expansion: FileContextMenuExpansion::None,
            }));
        }
        Task::batch([expansion_command, rename_command])
    }

    pub(super) fn handle_column_blank_right_clicked(
        &mut self,
        directory: PathBuf,
    ) -> Task<Message> {
        self.record_pointer_clicked_column(directory.clone());
        self.handle_blank_area_right_clicked(directory)
    }

    pub(super) fn handle_blank_area_right_clicked(&mut self, directory: PathBuf) -> Task<Message> {
        let expansion_command =
            if self.view_mode == BrowserViewMode::Icons && directory == self.current_dir {
                self.dismiss_icon_grid_expansion_from_outside()
            } else {
                if let Some(state) = self.icon_grid_expansion.as_mut() {
                    state.set_selection_directory(&directory);
                }
                Task::none()
            };
        let rename_command = self.commit_rename_if_active();
        self.clear_preview();
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        if self.file_area_menu_is_visible(false, false, false) {
            self.context_menu = Some(ContextMenuState::FileArea(FileContextMenuState {
                target: None,
                target_is_directory: false,
                paste_directory: directory,
                can_batch_rename: false,
                can_create_symlink: false,
                delete_action: FileDeleteAction::MoveToTrash,
                position: self.cursor_position,
                expansion: FileContextMenuExpansion::None,
            }));
        }
        Task::batch([expansion_command, rename_command])
    }

    /// 与 view 层 trash/entry/blank 面板选择同一谓词,配置后无可见面板时菜单不进入状态。
    pub(super) fn file_area_menu_is_visible(
        &self,
        has_target: bool,
        target_is_directory: bool,
        can_batch_rename: bool,
    ) -> bool {
        let menus = &self.user_config.context_menus;
        if self.is_trash_view {
            !menus.trash_items(has_target).is_empty()
        } else if has_target {
            !menus
                .file_entry_items(target_is_directory, can_batch_rename, true)
                .is_empty()
        } else {
            !menus.file_blank_items().is_empty()
        }
    }

    /// 选中项全部位于本地挂载时才允许创建符号链接;混合选中按远程处理。
    fn selection_all_paths_local(&self) -> bool {
        self.selected_paths_for_operation()
            .iter()
            .all(|path| !self.path_is_remote_mount(path))
    }

    pub(super) fn update_file_context_menu_expansion(
        &mut self,
        expansion: FileContextMenuExpansion,
    ) -> Task<Message> {
        if let Some(ContextMenuState::FileArea(menu)) = &mut self.context_menu {
            menu.expansion = expansion;
        }
        Task::none()
    }

    fn focus_column_blank_context_or_clear_selection(&mut self, directory: PathBuf) {
        if let Some(directory) = self.column_blank_context_directory(&directory) {
            self.set_deepest_open_column_directory(Some(directory.clone()));
            self.select_path(directory);
        } else {
            self.clear_column_blank_selection_context();
        }
    }

    fn column_blank_context_directory(&self, directory: &Path) -> Option<PathBuf> {
        if directory == self.current_dir || self.entry_kind(directory) != Some(FileKind::Directory)
        {
            None
        } else {
            Some(directory.to_path_buf())
        }
    }

    fn update_open_column_directory_for_entry(&mut self, path: &Path) {
        let next_directory = match self.entry_kind(path) {
            Some(FileKind::Directory) => Some(path.to_path_buf()),
            _ => path.parent().map(Path::to_path_buf),
        };
        self.set_deepest_open_column_directory(next_directory);
    }

    pub(super) fn set_deepest_open_column_directory(&mut self, directory: Option<PathBuf>) {
        self.deepest_open_column_directory = directory.filter(|directory| {
            directory != &self.current_dir && directory.starts_with(self.current_dir.as_path())
        });
    }

    fn clear_file_selection(&mut self) {
        self.selected_paths.clear();
        self.selected = None;
        self.selection_anchor = None;
        self.rename_input.clear();
    }

    fn clear_column_blank_selection_context(&mut self) {
        self.deepest_open_column_directory = None;
        self.clear_file_selection();
    }

    pub(super) fn select_all_in_file_selection_scope(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        self.cancel_expansion_follow_plans();
        let paths = self.select_all_selection_scope_paths();
        self.selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
        self.selection_anchor = paths.first().cloned();
        if let Some(path) = paths.first().cloned() {
            self.focus_path(path);
        } else {
            self.selected = None;
            self.rename_input.clear();
        }
        self.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    fn select_all_selection_scope_paths(&self) -> Vec<PathBuf> {
        if self.is_trash_view {
            return self.visible_entry_paths();
        }

        if self.view_mode == BrowserViewMode::Icons {
            let directory = self
                .icon_grid_expansion
                .as_ref()
                .map(|state| state.selection_directory())
                .unwrap_or(self.current_dir.as_path());
            return self.entry_paths_in_directory(directory);
        }

        if self.view_mode == BrowserViewMode::Columns {
            return self.entry_paths_in_directory(&self.column_select_all_directory());
        }

        self.visible_entry_paths()
    }

    fn column_select_all_directory(&self) -> PathBuf {
        if let Some(directory) = self.focused_rendered_column_directory() {
            return directory;
        }

        if let Some(path) = &self.hovered_entry {
            return self.entry_parent_directory(path);
        }

        if let Some(directory) = self.hovered_column_directory() {
            return directory;
        }

        self.selected
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.current_dir.clone())
    }

    fn focused_rendered_column_directory(&self) -> Option<PathBuf> {
        let directory = self.focused_column_directory.as_ref()?;
        crate::three_column_view::column_directories(self)
            .into_iter()
            .find(|rendered_directory| rendered_directory == directory)
    }

    pub(super) fn last_pointer_clicked_rendered_column_directory(&self) -> Option<PathBuf> {
        let directory = self.last_pointer_clicked_column_directory.as_ref()?;
        crate::three_column_view::column_directories(self)
            .into_iter()
            .find(|rendered_directory| rendered_directory == directory)
    }

    pub(in crate::app) fn clear_column_interaction_context(&mut self) {
        self.focused_column_directory = None;
        self.last_pointer_clicked_column_directory = None;
    }

    pub(in crate::app) fn focus_column_from_pointer_click(&mut self, directory: PathBuf) {
        self.focused_column_directory = Some(directory.clone());
        self.record_pointer_clicked_column(directory);
    }

    fn record_pointer_clicked_column(&mut self, directory: PathBuf) {
        self.last_pointer_clicked_column_directory = Some(directory);
    }

    /// 多栏视图下鼠标悬停的栏目录;终端目录跟随等跨模块场景使用。
    pub(crate) fn pointer_hovered_column_directory(&self) -> Option<PathBuf> {
        self.hovered_column_directory()
    }

    fn hovered_column_directory(&self) -> Option<PathBuf> {
        let directory = self.cursor_paste_directory.as_ref()?;
        crate::three_column_view::column_directories(self)
            .into_iter()
            .find(|column_directory| column_directory == directory)
    }

    pub(super) fn entry_kind(&self, path: &Path) -> Option<FileKind> {
        self.entry_kind_recursive(path)
    }

    fn directory_drop_target_for_entry(&self, path: &Path) -> PathBuf {
        if self.entry_kind(path) == Some(FileKind::Directory) {
            path.to_path_buf()
        } else {
            self.entry_parent_directory(path)
        }
    }

    fn directory_drop_target_for_entry_in_pane(
        &self,
        pane_id: BrowserPaneId,
        path: &Path,
    ) -> Option<PathBuf> {
        let pane = self.pane_view(pane_id)?;
        if pane.is_trash_view {
            return None;
        }

        let entry_kind = match pane.view_mode {
            BrowserViewMode::Icons if pane_id == self.active_pane_id() => self.entry_kind(path),
            BrowserViewMode::Icons => pane
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .map(|entry| entry.kind),
            BrowserViewMode::Columns | BrowserViewMode::List => pane
                .entries
                .iter()
                .chain(
                    pane.expanded_directories
                        .values()
                        .flat_map(|expanded| expanded.entries.iter()),
                )
                .find(|entry| entry.path == path)
                .map(|entry| entry.kind),
        };

        if entry_kind == Some(FileKind::Directory) {
            Some(path.to_path_buf())
        } else {
            Some(
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| pane.current_dir.clone()),
            )
        }
    }

    fn pane_accepts_file_drag(&self, pane_id: BrowserPaneId) -> bool {
        self.pane_view(pane_id)
            .is_some_and(|pane| !pane.is_trash_view)
    }

    pub(in crate::app) fn entry_parent_directory(&self, path: &Path) -> PathBuf {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.current_dir.clone())
    }

    fn select_context_menu_target(&mut self, path: PathBuf) {
        if self.is_path_selected(&path) {
            return;
        }

        self.cancel_expansion_follow_plans();
        self.selected_paths.clear();
        self.selected_paths.insert(path.clone());
        self.selection_anchor = Some(path);
    }

    fn toggle_path_selection(&mut self, path: PathBuf) {
        if self.selected_paths.remove(&path) {
            self.selected = self.last_visible_selected_path();
            if let Some(selected) = self.selected.clone() {
                self.focus_path(selected);
            } else {
                self.rename_input.clear();
            }
        } else {
            self.selected_paths.insert(path.clone());
            self.focus_path(path);
        }
        self.clear_preview();
        self.context_menu = None;
    }

    fn select_range(
        &mut self,
        anchor: PathBuf,
        target: PathBuf,
        preserve_existing: bool,
    ) -> PathBuf {
        let effective_anchor =
            if self.range_selection_confined_to_directory() && anchor.parent() != target.parent() {
                target.clone()
            } else {
                anchor
            };
        if !preserve_existing {
            self.selected_paths.clear();
        }

        for path in self.visible_range_paths(&effective_anchor, &target) {
            self.selected_paths.insert(path);
        }

        self.focus_path(target);
        effective_anchor
    }

    fn select_drag_range(&mut self, anchor: PathBuf, target: PathBuf, preserve_existing: bool) {
        if !preserve_existing {
            self.selected_paths.clear();
        }

        for path in self.visible_range_paths(&anchor, &target) {
            self.selected_paths.insert(path);
        }
    }

    pub(super) fn selected_paths_for_operation(&self) -> Vec<PathBuf> {
        let mut paths = self
            .visible_entry_paths()
            .into_iter()
            .filter(|path| self.selected_paths.contains(path))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            if let Some(path) = self.selected.clone() {
                paths.push(path);
            }
        }
        paths
    }

    /// 窗格底部选中统计。候选条目与各视图的操作可见集同源:
    /// 列表/多栏取直接条目+展开子条目,图标取直接条目+交互展开子条目,
    /// 保证底栏数字与删除/复制等操作作用范围一致。
    pub(crate) fn pane_selection_summary(
        &self,
        pane: BrowserPaneView<'_>,
    ) -> Option<PaneSelectionSummary> {
        // 搜索工作区替换了窗格内容,生效的是搜索结果独立选择集,
        // 底栏统计只属于文件浏览窗格。
        if pane.selected_paths.is_empty() || self.search_workspace.is_some() {
            return None;
        }

        let mut summary = summarize_selected_entries(
            pane.entries.iter(),
            pane.selected_paths,
            |entry| pane.metadata_for_entry(entry).len,
        );
        match pane.view_mode {
            BrowserViewMode::Icons => {
                if let Some(expansion) = self
                    .icon_grid_expansion
                    .as_ref()
                    .filter(|state| {
                        state.context().pane_id == pane.id
                            && state.context().current_dir == *pane.current_dir
                    })
                {
                    let selected_children = pane
                        .selected_paths
                        .iter()
                        .filter(|path| path.parent() != Some(pane.current_dir.as_path()))
                        .filter_map(|path| expansion.entry(path));
                    summary.merge(summarize_selected_entries(
                        selected_children,
                        pane.selected_paths,
                        |entry| pane.metadata_for_entry(entry).len,
                    ));
                }
            }
            BrowserViewMode::Columns | BrowserViewMode::List => {
                summary.merge(summarize_selected_entries(
                    pane.expanded_directories
                        .values()
                        .flat_map(|expanded| expanded.entries.iter()),
                    pane.selected_paths,
                    |entry| pane.metadata_for_entry(entry).len,
                ));
            }
        }
        (!summary.is_empty()).then_some(summary)
    }

    /// 底部工具栏左端的选中统计:按窗格顺序,只产出有选中的窗格。
    pub(crate) fn pane_selection_summaries(&self) -> Vec<PaneSelectionSummary> {
        self.panes
            .iter()
            .filter_map(|pane| {
                let pane = self.pane_view(pane.id)?;
                self.pane_selection_summary(pane)
            })
            .collect()
    }
    pub(super) fn focus_after_removed_file_operation_paths(
        &mut self,
        removed_paths: &[PathBuf],
    ) -> Task<Message> {
        let Some(selected) = self.selected.clone() else {
            return Task::none();
        };
        if !Self::path_is_removed_by_file_operation(&selected, removed_paths) {
            return Task::none();
        }

        let paths = match self.view_mode {
            BrowserViewMode::Columns => {
                let directory = selected
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.current_dir.clone());
                self.entry_paths_in_directory(&directory)
            }
            BrowserViewMode::Icons | BrowserViewMode::List => self.visible_entry_paths(),
        };
        let Some(selected_index) = paths.iter().position(|path| path == &selected) else {
            self.clear_file_selection();
            self.clear_preview();
            return Task::none();
        };

        let target = paths
            .iter()
            .skip(selected_index + 1)
            .chain(paths[..selected_index].iter().rev())
            .find(|path| !Self::path_is_removed_by_file_operation(path.as_path(), removed_paths))
            .cloned();
        let Some(target) = target else {
            self.clear_file_selection();
            self.clear_preview();
            return Task::none();
        };

        if self.view_mode == BrowserViewMode::Icons {
            let target_directory = self.entry_parent_directory(&target);
            if let Some(state) = self.icon_grid_expansion.as_mut() {
                state.set_selection_directory(&target_directory);
            }
        }
        self.select_path_from_keyboard(target)
    }
    fn path_is_removed_by_file_operation(path: &Path, removed_paths: &[PathBuf]) -> bool {
        removed_paths
            .iter()
            .any(|removed| path == removed || path.starts_with(removed))
    }

    pub(super) fn active_file_selection(&self) -> Vec<PathBuf> {
        self.active_search_selection()
            .unwrap_or_else(|| self.selected_paths_for_operation())
    }

    fn file_delete_action_for_selection(&self) -> FileDeleteAction {
        let paths = self.selected_paths_for_operation();
        let has_remote_path = paths.iter().any(|path| self.path_is_remote_mount(path));
        let has_local_path = paths.iter().any(|path| !self.path_is_remote_mount(path));
        match (has_remote_path, has_local_path) {
            (true, false) => FileDeleteAction::DeletePermanently,
            (true, true) => FileDeleteAction::MixedSelection,
            _ => FileDeleteAction::MoveToTrash,
        }
    }

    fn selected_trash_entries_for_operation(&self) -> Vec<file_core::TrashRestoreEntry> {
        let paths = self.selected_paths_for_operation();
        self.trash_entries
            .iter()
            .filter(|trash_entry| paths.iter().any(|path| path == &trash_entry.trash_path))
            .map(|trash_entry| trash_entry.restore_entry())
            .collect()
    }

    fn last_visible_selected_path(&self) -> Option<PathBuf> {
        self.visible_entry_paths()
            .into_iter()
            .rev()
            .find(|path| self.selected_paths.contains(path))
    }

    fn focus_path(&mut self, path: PathBuf) {
        self.pending_keyboard_column_focus = None;
        self.update_rename_input(&path);
        self.selected = Some(path);
        self.clear_preview();
        self.context_menu = None;
    }

    fn update_rename_input(&mut self, path: &Path) {
        self.rename_input = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}
