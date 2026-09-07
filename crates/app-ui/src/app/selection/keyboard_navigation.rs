use std::path::{Path, PathBuf};

use file_core::{is_supported_archive_path, FileKind};
use iced::Task;

use super::super::{
    FileBrowser, PendingKeyboardColumnFocus, right_preview_panel::PreviewLoadSurface,
};
use crate::commands::{
    animated_image_preview_command, image_preview_dimensions_command,
    load_expanded_directory_command, open_file_command, open_terminal_command, preview_command,
    start_audio_preview_command,
};
use crate::formatting::format_file_size;
use crate::list_view::LIST_HEADER_HEIGHT;
use crate::model::{
    AudioPreviewPlayback, BrowserViewMode, DirectoryExpansionLoadContext, ExpandedDirectory,
    ExpandedDirectoryStatus, ImagePreviewViewport, Message, NavigationMode, PreviewState,
    PreviewWindowProfile, ScrollbarRegion,
};
use crate::shortcuts::FileSelectionDirection;
use crate::virtual_range::vertical_scroll_delta_to_reveal;

#[derive(Debug, Clone, Copy)]
enum SelectionStep {
    Previous,
    Next,
}

impl FileBrowser {
    pub(crate) fn activate_selected_path(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let Some(path) = self.selected.clone() else {
            return Task::none();
        };
        self.activate_path(path)
    }

    pub(crate) fn move_file_selection(
        &mut self,
        direction: FileSelectionDirection,
    ) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }

        if self.view_mode == BrowserViewMode::Icons {
            return self.move_file_selection_in_icon_grid(direction);
        }

        match direction {
            FileSelectionDirection::Up if self.view_mode == BrowserViewMode::List => {
                self.move_file_selection_in_visible_list(SelectionStep::Previous)
            }
            FileSelectionDirection::Down if self.view_mode == BrowserViewMode::List => {
                self.move_file_selection_in_visible_list(SelectionStep::Next)
            }
            FileSelectionDirection::Up => {
                self.move_file_selection_vertically(SelectionStep::Previous)
            }
            FileSelectionDirection::Down => {
                self.move_file_selection_vertically(SelectionStep::Next)
            }
            FileSelectionDirection::Left if self.view_mode == BrowserViewMode::List => {
                self.collapse_selected_list_directory_or_select_parent()
            }
            FileSelectionDirection::Right if self.view_mode == BrowserViewMode::List => {
                self.expand_selected_list_directory()
            }
            FileSelectionDirection::Left => self.move_file_selection_to_parent_column(),
            FileSelectionDirection::Right => self.move_file_selection_to_child_column(),
        }
    }

    fn move_file_selection_in_icon_grid(
        &mut self,
        direction: FileSelectionDirection,
    ) -> Task<Message> {
        let pane_id = self.active_pane_id();
        let direction = match direction {
            FileSelectionDirection::Up => crate::icon_grid_geometry::IconGridDirection::Up,
            FileSelectionDirection::Down => crate::icon_grid_geometry::IconGridDirection::Down,
            FileSelectionDirection::Left => crate::icon_grid_geometry::IconGridDirection::Left,
            FileSelectionDirection::Right => crate::icon_grid_geometry::IconGridDirection::Right,
        };
        let Some((target, target_directory)) = self.pane_view(pane_id).and_then(|pane| {
            let layout = self.icon_grid_layout_for_pane(pane);
            let target = layout.keyboard_target(self.selected.as_deref(), direction)?;
            Some((target.entry.path.clone(), target.directory.to_path_buf()))
        }) else {
            return Task::none();
        };

        if let Some(state) = self.icon_grid_expansion.as_mut() {
            state.set_selection_directory(&target_directory);
        }
        let scroll_task = self.select_path_from_keyboard(target);

        Task::batch([scroll_task, self.schedule_thumbnail_refresh()])
    }

    fn move_file_selection_in_visible_list(&mut self, step: SelectionStep) -> Task<Message> {
        let paths = self.visible_entry_paths();
        let Some(target) = stepped_selection_target(&paths, self.selected.as_deref(), step) else {
            return Task::none();
        };

        let scroll_task = self.select_path_from_keyboard(target);
        Task::batch([scroll_task, self.schedule_thumbnail_refresh()])
    }

    fn move_file_selection_vertically(&mut self, step: SelectionStep) -> Task<Message> {
        let directory = self
            .focused_rendered_column_directory()
            .or_else(|| {
                self.selected
                    .as_ref()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| self.current_dir.clone());
        let paths = self.entry_paths_in_directory(&directory);
        let Some(target) = stepped_selection_target(&paths, self.selected.as_deref(), step) else {
            return Task::none();
        };

        let scroll_task = self.select_path_from_keyboard(target.clone());
        Task::batch([scroll_task, self.open_column_for_keyboard_selection(target)])
    }

    fn move_file_selection_to_parent_column(&mut self) -> Task<Message> {
        let Some(selected) = self.selected.clone() else {
            return Task::none();
        };
        let Some(parent) = selected.parent().map(Path::to_path_buf) else {
            return Task::none();
        };
        if parent == self.current_dir || self.entry_kind(&parent) != Some(FileKind::Directory) {
            return Task::none();
        }

        self.column_return_targets
            .insert(parent.clone(), selected.clone());
        let scroll_task = self.select_path_from_keyboard(parent.clone());
        Task::batch([scroll_task, self.focus_column_containing_path(&parent)])
    }

    fn move_file_selection_to_child_column(&mut self) -> Task<Message> {
        let Some(selected) = self.selected.clone() else {
            return self.move_file_selection_vertically(SelectionStep::Next);
        };
        if self.entry_kind(&selected) != Some(FileKind::Directory) {
            return Task::none();
        }

        let preferred_child = self.column_return_targets.get(&selected).cloned();
        let open_command = self.open_column_for_directory(selected.clone());
        if let Some(path) = self.keyboard_child_focus_target(&selected, preferred_child.as_deref())
        {
            self.pending_keyboard_column_focus = None;
            let scroll_task = self.select_path_from_keyboard(path.clone());
            return Task::batch([
                open_command,
                scroll_task,
                self.open_column_for_keyboard_selection(path),
            ]);
        }

        if self.directory_children_are_loading(&selected) {
            self.pending_keyboard_column_focus = Some(PendingKeyboardColumnFocus {
                pane_id: self.active_pane_id(),
                directory: selected,
                preferred_child,
            });
        } else {
            self.pending_keyboard_column_focus = None;
        }
        open_command
    }

    pub(crate) fn complete_pending_keyboard_column_focus(
        &mut self,
        directory: &Path,
    ) -> Task<Message> {
        let Some(pending) = self.pending_keyboard_column_focus.clone() else {
            return Task::none();
        };
        if pending.pane_id != self.active_pane_id() || pending.directory != directory {
            return Task::none();
        }

        self.pending_keyboard_column_focus = None;
        if self.selected.as_deref() != Some(directory) {
            return Task::none();
        }

        let Some(path) =
            self.keyboard_child_focus_target(directory, pending.preferred_child.as_deref())
        else {
            return Task::none();
        };

        let scroll_task = self.select_path_from_keyboard(path.clone());
        Task::batch([scroll_task, self.open_column_for_keyboard_selection(path)])
    }

    fn keyboard_child_focus_target(
        &self,
        directory: &Path,
        preferred_child: Option<&Path>,
    ) -> Option<PathBuf> {
        let paths = self.entry_paths_in_directory(directory);
        if let Some(preferred_child) = preferred_child {
            if paths.iter().any(|path| path == preferred_child) {
                return Some(preferred_child.to_path_buf());
            }
        }
        paths.into_iter().next()
    }

    fn directory_children_are_loading(&self, directory: &Path) -> bool {
        self.expanded_directories
            .get(directory)
            .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading))
    }

    fn open_column_for_keyboard_selection(&mut self, path: PathBuf) -> Task<Message> {
        if self.entry_kind(&path) == Some(FileKind::Directory) {
            self.open_column_for_directory(path)
        } else {
            self.update_open_column_directory_for_entry(&path);
            Task::none()
        }
    }

    pub(crate) fn select_path_from_keyboard(&mut self, path: PathBuf) -> Task<Message> {
        if self.view_mode == BrowserViewMode::Columns {
            self.focused_column_directory = Some(self.entry_parent_directory(&path));
        }
        self.select_path(path.clone());
        self.selection_anchor = Some(path.clone());
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.pending_keyboard_column_focus = None;
        self.reveal_keyboard_selection(&path)
    }

    fn reveal_keyboard_selection(&self, path: &Path) -> Task<Message> {
        let Some((region, scroll_delta)) = self.keyboard_selection_scroll(path) else {
            return Task::none();
        };
        if scroll_delta.abs() <= f32::EPSILON {
            return Task::none();
        }
        iced::widget::operation::scroll_by(
            crate::app::smooth_scroll::smooth_scroll_id(&region),
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: scroll_delta,
            },
        )
    }

    fn keyboard_selection_scroll(&self, path: &Path) -> Option<(ScrollbarRegion, f32)> {
        let pane_id = self.active_pane_id();
        Some(match self.view_mode {
            BrowserViewMode::Icons => {
                let pane = self.pane_view(pane_id)?;
                let viewport = pane.icon_grid_viewport;
                let layout = self.icon_grid_layout_for_pane(pane);
                (
                    ScrollbarRegion::PaneIcons(pane_id),
                    layout.scroll_delta_to_reveal(viewport, path),
                )
            }
            BrowserViewMode::List => {
                let viewport = self.column_viewports.get(&self.current_dir)?;
                let geometry =
                    crate::list_view::ListGeometry::for_level(self.user_config.list_view_density);
                let (item_offset, item_height) =
                    crate::visible_entries::list_entry_vertical_bounds(
                        &self.entries,
                        &self.expanded_directories,
                        path,
                        geometry.row_height,
                        LIST_HEADER_HEIGHT,
                    )?;
                (
                    ScrollbarRegion::PaneList(pane_id),
                    vertical_scroll_delta_to_reveal(
                        viewport.offset_y,
                        viewport.height,
                        item_offset,
                        item_height,
                    ),
                )
            }
            BrowserViewMode::Columns => {
                let directory = self.entry_parent_directory(path);
                let viewport = self.column_viewports.get(&directory)?;
                let entries = if directory == self.current_dir {
                    self.entries.as_ref()
                } else {
                    self.expanded_directories
                        .get(&directory)?
                        .entries
                        .as_slice()
                };
                let row_index = entries.iter().position(|entry| entry.path == path)?;
                let geometry = crate::three_column_view::ColumnGeometry::for_level(
                    self.user_config.columns_view_density,
                );
                (
                    ScrollbarRegion::Column { pane_id, directory },
                    vertical_scroll_delta_to_reveal(
                        viewport.offset_y,
                        viewport.height,
                        geometry.entries_top_padding
                            + row_index as f32 * geometry.entry_scroll_height,
                        geometry.entry_height,
                    ),
                )
            }
        })
    }

    pub(crate) fn request_preview(&mut self) -> Task<Message> {
        // 空格 toggle 的对象是键盘选中项;悬停不作门控——纯键盘操作没有
        // hovered_entry,若沿用“悬停才开”会让键盘用户永远开不了预览。
        if self.standalone_preview_session_active()
            && self.preview_shown_path.as_deref() == self.selected.as_deref()
        {
            self.context_menu = None;
            return self.close_preview_window();
        }

        self.open_preview()
    }

    /// Space toggle 的不变量：独立预览窗口会话正在活跃显示。preview 会话
    /// 状态与右侧停靠面板共享（面板会话同样置 preview 非空），单看
    /// preview.is_some() 会把面板加载误判成“独立窗口已显示”，导致面板
    /// 开着时按 Space 打不开独立窗口；补上窗口存在性即回到原语义。
    fn standalone_preview_session_active(&self) -> bool {
        self.preview_window.is_some() && self.preview.is_some()
    }

    pub(crate) fn open_path(&mut self, path: PathBuf) -> Task<Message> {
        self.context_menu = None;
        self.activate_path(path)
    }

    pub(crate) fn activate_path(&mut self, path: PathBuf) -> Task<Message> {
        if self.is_trash_view {
            if !self.selected_paths.contains(&path) {
                self.select_path(path);
            } else if self.selected.as_deref() != Some(path.as_path()) {
                self.focus_path(path);
            }
            return self.restore_selected();
        }

        match self.entry_kind(&path) {
            Some(FileKind::Directory) => self.navigate_to(path, NavigationMode::RecordHistory),
            Some(_) | None if is_supported_archive_path(&path) => {
                self.request_archive_extraction(path)
            }
            Some(_) | None => open_file_command(path, self.terminal_emulator),
        }
    }

    pub(crate) fn open_terminal_here(&mut self, directory: PathBuf) -> Task<Message> {
        self.context_menu = None;
        if self.is_trash_view {
            return Task::none();
        }
        open_terminal_command(directory, self.terminal_emulator)
    }

    pub(crate) fn open_column_for_directory(&mut self, path: PathBuf) -> Task<Message> {
        if self.is_trash_view {
            return Task::none();
        }

        if self.entry_kind(&path) != Some(FileKind::Directory) {
            return Task::none();
        }
        self.set_deepest_open_column_directory(Some(path.clone()));

        if let Some(expanded) = self.expanded_directories.get_mut(&path) {
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = 1.0;
            self.sync_active_tab_state();
            return Task::batch([
                self.focus_latest_column(),
                self.request_browser_session_save(),
                self.reveal_address_bar_current_segment(self.active_pane_id()),
            ]);
        }

        let mut expanded = ExpandedDirectory {
            entries: Vec::new(),
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loading,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        };
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            DirectoryExpansionLoadContext::BrowserTree {
                pane_id: self.active_pane_id(),
            },
            path.clone(),
            &mut expanded,
        );
        self.expanded_directories.insert(path, expanded);
        self.sync_active_tab_state();
        Task::batch([
            load_expanded_directory_command(request, self.options.clone(), cancellation),
            self.focus_latest_column(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
    }

    fn open_preview(&mut self) -> Task<Message> {
        self.context_menu = None;
        // 空格驱动的加载会话面向独立窗口:异步回流时窗口尺寸/聚焦动作
        // 全部照旧;面板发起的会话经 sync_right_preview_panel 另行标注。
        self.preview_load_surface = PreviewLoadSurface::StandaloneWindow;
        // 内部“先关后开”会复位固定状态；切换预览内容时必须保留用户设定的固定。
        let pinned = self.preview_window_pinned;
        let close_window_command = self.close_preview_window();

        // 悬停在条目上但没有选中项：无操作。
        let Some(path) = self.selected.clone() else {
            return Task::none();
        };

        let kind = self.entry_kind(&path).unwrap_or(FileKind::Other);
        self.preview_shown_path = Some(path.clone());
        if kind == FileKind::File {
            // 门禁放在远程下载与大小检查之前：不可预览的远程文件不值得下载，
            // 也不应误报“文件太大”。
            if crate::preview::classify_preview_path(
                &path,
                &self.user_config.preview_extension_rules,
            )
            .is_none()
            {
                self.preview_window_pinned = pinned;
                return close_window_command.chain(self.show_unpreviewable_file_preview());
            }
            if let Some(command) = self.reject_oversized_file_preview(&path) {
                return close_window_command.chain(command);
            }
        }
        if kind == FileKind::File && self.path_is_remote_mount(&path) {
            self.preview_window_pinned = pinned;
            return close_window_command.chain(self.start_remote_preview_download(path));
        }

        self.preview_window_pinned = pinned;
        close_window_command.chain(self.open_preview_for_resolved_path(path, kind))
    }

    /// QuickLook 式占位：不可预览类型弹预览窗口显示错误，与“文件太大”同一表现。
    fn show_unpreviewable_file_preview(&mut self) -> Task<Message> {
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.clear_preview();
        self.preview = Some(PreviewState::Error(
            "No preview available for this file type.".to_owned(),
        ));
        window_command
    }

    pub(in crate::app) fn open_preview_for_resolved_path(
        &mut self,
        path: PathBuf,
        kind: FileKind,
    ) -> Task<Message> {
        // 新预览会话不复用上一个文件的缩放/平移；同会话内的
        // 缩略图→原图替换不经过这里，视口得以保留。
        self.preview_image_viewport = ImagePreviewViewport::default();
        if kind == FileKind::File {
            // 预览会话的诞生边界：远程下载回流与本地入口共用这一道门，
            // 文本兜底同样受白名单约束。
            let classification = crate::preview::classify_preview_path(
                &path,
                &self.user_config.preview_extension_rules,
            );
            let Some(classification) = classification else {
                return self.show_unpreviewable_file_preview();
            };
            return self.start_classified_preview(path, classification);
        }

        // 目录与未识别类型不走文件分类：目录预览由 load_preview 展开，
        // Symlink/Other 的错误仍由 load_preview 边界返回。
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.clear_preview();
        self.preview = Some(PreviewState::Loading(path.clone()));
        self.clear_global_error();
        let max_file_bytes = self.preview_file_size_limit_for(&path);
        Task::batch([
            window_command,
            preview_command(
                path,
                kind,
                self.user_config.preview_extension_rules.clone(),
                self.options.clone(),
                max_file_bytes,
            ),
            self.request_browser_session_save(),
        ])
    }

    fn start_classified_preview(
        &mut self,
        path: PathBuf,
        classification: crate::preview::PreviewPathKind,
    ) -> Task<Message> {
        match classification {
            crate::preview::PreviewPathKind::Document => self.start_document_preview(path),
            crate::preview::PreviewPathKind::AnimatedImage => {
                self.preview = Some(PreviewState::Loading(path.clone()));
                self.clear_global_error();
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                let generation = self.next_animated_image_preview_generation();
                Task::batch([
                    animated_image_preview_command(path, generation, max_file_bytes),
                    self.request_browser_session_save(),
                ])
            }
            crate::preview::PreviewPathKind::Image => {
                self.preview = Some(PreviewState::Loading(path.clone()));
                self.clear_global_error();
                let generation = self.next_original_image_preview_generation();
                Task::batch([
                    image_preview_dimensions_command(path, generation),
                    self.request_browser_session_save(),
                ])
            }
            crate::preview::PreviewPathKind::Video => {
                self.preview = Some(PreviewState::Loading(path.clone()));
                self.clear_global_error();
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                preview_command(
                    path,
                    FileKind::File,
                    self.user_config.preview_extension_rules.clone(),
                    self.options.clone(),
                    max_file_bytes,
                )
            }
            crate::preview::PreviewPathKind::Audio => {
                let window_command =
                    self.preview_window_presentation_command(PreviewWindowProfile::Audio);
                self.clear_preview();
                self.preview = Some(PreviewState::Loading(path.clone()));
                self.clear_global_error();
                self.audio_preview = Some(AudioPreviewPlayback::loading(path.clone()));
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    preview_command(
                        path.clone(),
                        FileKind::File,
                        self.user_config.preview_extension_rules.clone(),
                        self.options.clone(),
                        max_file_bytes,
                    ),
                    start_audio_preview_command(path),
                    self.request_browser_session_save(),
                ])
            }
            crate::preview::PreviewPathKind::Archive
            | crate::preview::PreviewPathKind::Sqlite
            | crate::preview::PreviewPathKind::Text => {
                let window_command =
                    self.preview_window_presentation_command(PreviewWindowProfile::Regular);
                self.clear_preview();
                self.preview = Some(PreviewState::Loading(path.clone()));
                self.clear_global_error();
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    preview_command(
                        path,
                        FileKind::File,
                        self.user_config.preview_extension_rules.clone(),
                        self.options.clone(),
                        max_file_bytes,
                    ),
                    self.request_browser_session_save(),
                ])
            }
        }
    }

    fn reject_oversized_file_preview(&mut self, path: &Path) -> Option<Task<Message>> {
        let entry = self.entry_for_path(path)?;
        let max_bytes = self.preview_file_size_limit_for(path);
        let file_bytes = entry.metadata.len;
        if max_bytes == 0 || file_bytes <= max_bytes {
            return None;
        }

        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.clear_preview();
        self.preview = Some(PreviewState::Error(format!(
            "File is too large to preview ({}). Maximum preview size is {}.",
            format_file_size(file_bytes),
            format_file_size(max_bytes)
        )));
        Some(window_command)
    }
}

fn stepped_selection_target(
    paths: &[PathBuf],
    selected: Option<&Path>,
    step: SelectionStep,
) -> Option<PathBuf> {
    if paths.is_empty() {
        return None;
    }

    let current = selected.and_then(|selected| paths.iter().position(|path| path == selected));
    let index = match (current, step) {
        (Some(index), SelectionStep::Previous) => index.saturating_sub(1),
        (Some(index), SelectionStep::Next) => (index + 1).min(paths.len() - 1),
        (None, SelectionStep::Previous) => paths.len() - 1,
        (None, SelectionStep::Next) => 0,
    };
    paths.get(index).cloned()
}

#[cfg(test)]
mod tests {
    use file_core::{DirectoryEntry, EntryMetadata, FileKind};
    use iced::futures::StreamExt;
    use iced_runtime::Action;

    use super::*;
    use crate::config;
    use crate::list_view::LIST_ROW_HEIGHT;
    use crate::thumbnail_cache::ColumnViewport;

    fn default_column_geometry() -> crate::three_column_view::ColumnGeometry {
        crate::three_column_view::ColumnGeometry::for_level(config::ViewDensityLevel::DEFAULT)
    }

    async fn widget_action_count(task: Task<Message>) -> usize {
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            return 0;
        };
        let mut count = 0;
        while let Some(action) = stream.next().await {
            if matches!(action, Action::Widget(_)) {
                count += 1;
            }
        }
        count
    }

    fn entry(index: usize) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from(format!("/workspace/{index}.txt")),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )
    }

    fn expanded_directory(
        entries: Vec<DirectoryEntry>,
        animation_progress: f32,
    ) -> ExpandedDirectory {
        ExpandedDirectory {
            entries,
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        }
    }

    fn browser_for_vertical_keyboard_navigation(
        view_mode: BrowserViewMode,
        viewport_height: f32,
    ) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.current_dir = PathBuf::from("/workspace");
        browser.view_mode = view_mode;
        browser.entries = (0..3).map(entry).collect::<Vec<_>>().into();
        browser.column_viewports.insert(
            browser.current_dir.clone(),
            ColumnViewport {
                offset_y: 0.0,
                height: viewport_height,
            },
        );
        browser.select_path(browser.entries[0].path.clone());
        browser
    }

    #[test]
    fn keyboard_reveal_uses_scroll_content_offsets() {
        let column_geometry = default_column_geometry();
        for (view_mode, viewport_height, expected_delta) in [
            (
                BrowserViewMode::List,
                LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT,
                LIST_ROW_HEIGHT,
            ),
            (
                BrowserViewMode::Columns,
                column_geometry.entries_top_padding + column_geometry.entry_height,
                column_geometry.entry_scroll_height,
            ),
        ] {
            let browser = browser_for_vertical_keyboard_navigation(view_mode, viewport_height);
            let (_, scroll_delta) = browser
                .keyboard_selection_scroll(&browser.entries[1].path)
                .expect("second row has a scroll target");

            assert_eq!(scroll_delta, expected_delta, "wrong delta in {view_mode:?}");
        }
    }

    #[test]
    fn list_keyboard_reveal_counts_status_and_animation_rows() {
        let mut browser = browser_for_vertical_keyboard_navigation(BrowserViewMode::List, 500.0);
        let directory = DirectoryEntry::new(
            browser.current_dir.join("directory"),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        );
        let sibling = entry(1);
        browser.entries = vec![directory.clone(), sibling.clone()].into();
        browser
            .expanded_directories
            .insert(directory.path.clone(), expanded_directory(Vec::new(), 1.0));

        assert_eq!(
            crate::visible_entries::list_entry_vertical_bounds(
                &browser.entries,
                &browser.expanded_directories,
                &sibling.path,
                LIST_ROW_HEIGHT,
                LIST_HEADER_HEIGHT,
            ),
            Some((LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT * 2.0, LIST_ROW_HEIGHT))
        );

        let child = DirectoryEntry::new(
            directory.path.join("child.txt"),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        );
        browser
            .expanded_directories
            .insert(directory.path.clone(), expanded_directory(vec![child], 0.5));

        assert_eq!(
            crate::visible_entries::list_entry_vertical_bounds(
                &browser.entries,
                &browser.expanded_directories,
                &sibling.path,
                LIST_ROW_HEIGHT,
                LIST_HEADER_HEIGHT,
            ),
            Some((LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT * 1.5, LIST_ROW_HEIGHT,))
        );
    }

    #[tokio::test]
    async fn list_keyboard_navigation_reveals_offscreen_selection() {
        let mut browser = browser_for_vertical_keyboard_navigation(
            BrowserViewMode::List,
            LIST_HEADER_HEIGHT + LIST_ROW_HEIGHT,
        );

        let widget_actions =
            widget_action_count(browser.move_file_selection(FileSelectionDirection::Down)).await;

        assert_eq!(browser.selected, Some(browser.entries[1].path.clone()));
        assert_eq!(widget_actions, 1);
    }

    #[tokio::test]
    async fn column_keyboard_navigation_reveals_offscreen_selection() {
        let mut browser = browser_for_vertical_keyboard_navigation(
            BrowserViewMode::Columns,
            default_column_geometry().entries_top_padding + default_column_geometry().entry_height,
        );

        let widget_actions =
            widget_action_count(browser.move_file_selection(FileSelectionDirection::Down)).await;

        assert_eq!(browser.selected, Some(browser.entries[1].path.clone()));
        assert_eq!(widget_actions, 1);
    }

    #[tokio::test]
    async fn visible_keyboard_selection_does_not_scroll() {
        for view_mode in [BrowserViewMode::List, BrowserViewMode::Columns] {
            let mut browser = browser_for_vertical_keyboard_navigation(view_mode, 500.0);

            let widget_actions =
                widget_action_count(browser.move_file_selection(FileSelectionDirection::Down))
                    .await;

            assert_eq!(browser.selected, Some(browser.entries[1].path.clone()));
            assert_eq!(widget_actions, 0, "unexpected scroll in {view_mode:?}");
        }
    }

    #[test]
    fn icon_grid_keyboard_navigation_uses_current_column_count() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.current_dir = PathBuf::from("/workspace");
        browser.view_mode = BrowserViewMode::Icons;
        browser.main_window_width = 500.0;
        browser.sidebar_width = 0.0;
        browser.entries = (0..8).map(entry).collect::<Vec<_>>().into();
        browser.select_path(browser.entries[1].path.clone());

        drop(browser.move_file_selection(FileSelectionDirection::Down));
        assert_eq!(browser.selected, Some(browser.entries[4].path.clone()));

        drop(browser.move_file_selection(FileSelectionDirection::Left));
        assert_eq!(browser.selected, Some(browser.entries[3].path.clone()));
    }

    #[test]
    fn icon_grid_keyboard_navigation_clamps_in_incomplete_last_row() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.current_dir = PathBuf::from("/workspace");
        browser.view_mode = BrowserViewMode::Icons;
        browser.main_window_width = 500.0;
        browser.sidebar_width = 0.0;
        browser.entries = (0..8).map(entry).collect::<Vec<_>>().into();
        browser.select_path(browser.entries[5].path.clone());

        drop(browser.move_file_selection(FileSelectionDirection::Down));

        assert_eq!(browser.selected, Some(browser.entries[7].path.clone()));
    }
}
