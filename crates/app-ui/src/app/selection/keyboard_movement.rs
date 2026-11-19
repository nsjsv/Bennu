use std::path::{Path, PathBuf};

use iced::Task;

use super::super::{FileBrowser, PendingKeyboardColumnFocus};
use crate::icon_grid_geometry::IconGridDirection;
use crate::icon_grid_layout::IconGridJump;
use crate::list_view::LIST_HEADER_HEIGHT;
use crate::model::{BrowserViewMode, ExpandedDirectoryStatus, Message, ScrollbarRegion};
use crate::shortcuts::{MoveExtent, MoveStep, SelectionMove};
use crate::virtual_range::vertical_scroll_delta_to_reveal;

/// 列表和多栏里沿一条直线序列移动：往前 / 往后几项，或到头到尾。
#[derive(Debug, Clone, Copy)]
enum LinearStep {
    Back(usize),
    Forward(usize),
    First,
    Last,
}

impl LinearStep {
    /// ←→ 在列表和多栏里是结构移动（折叠展开、父栏子栏），不是线性的。
    fn from_vertical(step: MoveStep, page_rows: usize) -> Option<Self> {
        Some(match step {
            MoveStep::Up => Self::Back(1),
            MoveStep::Down => Self::Forward(1),
            MoveStep::PageUp => Self::Back(page_rows),
            MoveStep::PageDown => Self::Forward(page_rows),
            MoveStep::First => Self::First,
            MoveStep::Last => Self::Last,
            MoveStep::Left | MoveStep::Right => return None,
        })
    }
}

impl FileBrowser {
    pub(crate) fn move_file_selection(&mut self, movement: SelectionMove) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }

        match self.tab().view_mode() {
            BrowserViewMode::Icons => self.move_file_selection_in_icon_grid(movement),
            BrowserViewMode::List => match movement.step {
                MoveStep::Left => self.collapse_selected_list_directory_or_select_parent(),
                MoveStep::Right => self.expand_selected_list_directory(),
                step => self.move_file_selection_in_visible_list(step, movement.extent),
            },
            BrowserViewMode::Columns => match movement.step {
                MoveStep::Left => self.move_file_selection_to_parent_column(),
                MoveStep::Right => self.move_file_selection_to_child_column(),
                step => self.move_file_selection_in_column(step, movement.extent),
            },
        }
    }

    fn move_file_selection_in_icon_grid(&mut self, movement: SelectionMove) -> Task<Message> {
        let pane_id = self.active_pane_id();
        let selected = self.tab().selected.clone();
        let Some((target, target_directory)) = self.pane_view(pane_id).and_then(|pane| {
            let layout = self.icon_grid_layout_for_pane(pane);
            let viewport_height = pane.icon_grid_viewport.height;
            let current = selected.as_deref();
            let target = match movement.step {
                MoveStep::Up => layout.keyboard_target(current, IconGridDirection::Up),
                MoveStep::Down => layout.keyboard_target(current, IconGridDirection::Down),
                MoveStep::Left => layout.keyboard_target(current, IconGridDirection::Left),
                MoveStep::Right => layout.keyboard_target(current, IconGridDirection::Right),
                MoveStep::PageUp => {
                    layout.keyboard_jump_target(current, IconGridJump::PageUp(viewport_height))
                }
                MoveStep::PageDown => {
                    layout.keyboard_jump_target(current, IconGridJump::PageDown(viewport_height))
                }
                MoveStep::First => layout.keyboard_jump_target(current, IconGridJump::First),
                MoveStep::Last => layout.keyboard_jump_target(current, IconGridJump::Last),
            }?;
            Some((
                target.entry.path().to_path_buf(),
                target.directory.map(Path::to_path_buf),
            ))
        }) else {
            return Task::none();
        };

        if let (Some(state), Some(target_directory)) =
            (self.icon_grid_expansion.as_mut(), target_directory)
        {
            state.set_selection_directory(&target_directory);
        }
        let scroll_task = self.apply_keyboard_move(target, movement.extent);

        Task::batch([scroll_task, self.schedule_thumbnail_refresh()])
    }

    fn move_file_selection_in_visible_list(
        &mut self,
        step: MoveStep,
        extent: MoveExtent,
    ) -> Task<Message> {
        let Some(step) = LinearStep::from_vertical(step, self.list_page_rows()) else {
            return Task::none();
        };
        let paths = self.visible_entry_paths();
        let Some(target) = stepped_selection_target(&paths, self.tab().selected.as_deref(), step)
        else {
            return Task::none();
        };

        let scroll_task = self.apply_keyboard_move(target, extent);
        Task::batch([scroll_task, self.schedule_thumbnail_refresh()])
    }

    fn move_file_selection_in_column(
        &mut self,
        step: MoveStep,
        extent: MoveExtent,
    ) -> Task<Message> {
        let Some(directory) = self.keyboard_column_directory() else {
            return Task::none();
        };
        let Some(step) = LinearStep::from_vertical(step, self.column_page_rows(&directory)) else {
            return Task::none();
        };
        let paths = self.entry_paths_in_directory(&directory);
        let Some(target) = stepped_selection_target(&paths, self.tab().selected.as_deref(), step)
        else {
            return Task::none();
        };

        match extent {
            MoveExtent::Cursor => {
                let scroll_task = self.select_path_from_keyboard(target.clone());
                Task::batch([scroll_task, self.open_column_for_keyboard_selection(target)])
            }
            // 扩选时不打开经过的目录的下一栏：选中的是一串项，没有哪一个是“当前打开的”。
            MoveExtent::FromAnchor => self.extend_selection_from_keyboard(target),
        }
    }

    /// 多栏里键盘作用的栏：最后点过或键盘走到的那一栏，其次是选中项所在的栏。
    fn keyboard_column_directory(&self) -> Option<PathBuf> {
        self.focused_rendered_column_directory()
            .or_else(|| {
                self.tab()
                    .selected
                    .as_ref()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.tab().current_directory().map(Path::to_path_buf))
    }

    /// 翻一页走的行数：可见行数少一行，翻页后上一屏的最后一行还在屏上。
    fn list_page_rows(&self) -> usize {
        let row_height =
            crate::list_view::ListGeometry::for_level(self.user_config.list_view_density)
                .row_height;
        let visible_height = self
            .pane()
            .list_viewport
            .map(|viewport| viewport.height - LIST_HEADER_HEIGHT)
            .unwrap_or(0.0);
        page_rows(visible_height, row_height)
    }

    fn column_page_rows(&self, directory: &Path) -> usize {
        let entry_height = crate::three_column_view::ColumnGeometry::for_level(
            self.user_config.columns_view_density,
        )
        .entry_scroll_height;
        let visible_height = self
            .pane()
            .column_viewports
            .get(directory)
            .map(|viewport| viewport.height)
            .unwrap_or(0.0);
        page_rows(visible_height, entry_height)
    }

    fn apply_keyboard_move(&mut self, target: PathBuf, extent: MoveExtent) -> Task<Message> {
        match extent {
            MoveExtent::Cursor => self.select_path_from_keyboard(target),
            MoveExtent::FromAnchor => self.extend_selection_from_keyboard(target),
        }
    }

    /// Shift+移动键：从锚点选到目标，和 Shift+点击用同一个锚点，锚点本身不动。
    fn extend_selection_from_keyboard(&mut self, target: PathBuf) -> Task<Message> {
        if self.tab().view_mode() == BrowserViewMode::Columns {
            self.focused_column_directory = Some(self.entry_parent_directory(&target));
        }
        let tab = self.tab();
        let anchor = tab
            .selection_anchor
            .clone()
            .or_else(|| tab.selected.clone())
            .unwrap_or_else(|| target.clone());
        let range_anchor = self.select_range(anchor, target.clone(), false);
        self.tab_mut().selection_anchor = Some(range_anchor);
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.pending_keyboard_column_focus = None;
        Task::batch([
            self.schedule_selected_metadata_for_all_panes(),
            self.reveal_keyboard_selection(&target),
        ])
    }

    fn move_file_selection_to_parent_column(&mut self) -> Task<Message> {
        let Some(selected) = self.tab().selected.clone() else {
            return Task::none();
        };
        let Some(parent) = selected.parent().map(Path::to_path_buf) else {
            return Task::none();
        };
        if self.tab().current_directory() == Some(parent.as_path())
            || !self.entry_acts_as_directory(&parent)
        {
            return Task::none();
        }

        self.column_return_targets
            .insert(parent.clone(), selected.clone());
        let scroll_task = self.select_path_from_keyboard(parent.clone());
        Task::batch([scroll_task, self.focus_column_containing_path(&parent)])
    }

    fn move_file_selection_to_child_column(&mut self) -> Task<Message> {
        let Some(selected) = self.tab().selected.clone() else {
            return self.move_file_selection_in_column(MoveStep::Down, MoveExtent::Cursor);
        };
        if !self.entry_acts_as_directory(&selected) {
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
        if self.tab().selected.as_deref() != Some(directory) {
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
        self.tab()
            .expanded_directories
            .get(directory)
            .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading))
    }

    fn open_column_for_keyboard_selection(&mut self, path: PathBuf) -> Task<Message> {
        if self.entry_acts_as_directory(&path) {
            self.open_column_for_directory(path)
        } else {
            self.update_open_column_directory_for_entry(&path);
            Task::none()
        }
    }

    pub(crate) fn select_path_from_keyboard(&mut self, path: PathBuf) -> Task<Message> {
        if self.tab().view_mode() == BrowserViewMode::Columns {
            self.focused_column_directory = Some(self.entry_parent_directory(&path));
        }
        self.select_path(path.clone());
        self.tab_mut().selection_anchor = Some(path.clone());
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.cancel_file_drag_interaction();
        self.pending_keyboard_column_focus = None;
        Task::batch([
            self.schedule_selected_metadata_for_all_panes(),
            self.reveal_keyboard_selection(&path),
        ])
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
        Some(match self.tab().view_mode() {
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
                let viewport = self.pane().list_viewport?;
                let geometry =
                    crate::list_view::ListGeometry::for_level(self.user_config.list_view_density);
                // 键盘跟随与列表渲染消费同一合并行流(含占位/状态行),
                // 占位行在选中项上方时不漏算位移。
                let pane = self.pane_view(pane_id)?;
                let (item_offset, item_height) = crate::transfer_placeholder_view::list_row_layout(
                    self,
                    pane,
                    geometry.row_height,
                )
                .vertical_bounds(pane, path, LIST_HEADER_HEIGHT)?;
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
                let viewport = self.pane().column_viewports.get(&directory)?;
                // 多栏渲染的是占位合入后的合并序列,行偏移按同一序列推算。
                let pane = self.pane_view(pane_id)?;
                let geometry = crate::three_column_view::ColumnGeometry::for_level(
                    self.user_config.columns_view_density,
                );
                let item_offset = geometry.entries_top_padding
                    + crate::transfer_placeholder_view::column_transfer_item_offset(
                        self,
                        pane,
                        &directory,
                        path,
                        geometry.entry_scroll_height,
                    )?;
                (
                    ScrollbarRegion::Column { pane_id, directory },
                    vertical_scroll_delta_to_reveal(
                        viewport.offset_y,
                        viewport.height,
                        item_offset,
                        geometry.entry_height,
                    ),
                )
            }
        })
    }
}

fn stepped_selection_target(
    paths: &[PathBuf],
    selected: Option<&Path>,
    step: LinearStep,
) -> Option<PathBuf> {
    let last = paths.len().checked_sub(1)?;
    let current = selected.and_then(|selected| paths.iter().position(|path| path == selected));
    let index = match (current, step) {
        (_, LinearStep::First) => 0,
        (_, LinearStep::Last) => last,
        (Some(index), LinearStep::Back(rows)) => index.saturating_sub(rows),
        (Some(index), LinearStep::Forward(rows)) => index.saturating_add(rows).min(last),
        (None, LinearStep::Back(_)) => last,
        (None, LinearStep::Forward(_)) => 0,
    };
    paths.get(index).cloned()
}

fn page_rows(visible_height: f32, row_height: f32) -> usize {
    if row_height <= 0.0 {
        return 1;
    }
    ((visible_height / row_height).floor() as usize)
        .saturating_sub(1)
        .max(1)
}

#[cfg(test)]
#[path = "keyboard_movement_tests.rs"]
mod tests;
