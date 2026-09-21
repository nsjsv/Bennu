use std::collections::HashSet;
use std::path::PathBuf;

use iced::{Point, Rectangle, Task};

use super::FileBrowser;
use crate::app::POINTER_DRAG_ACTIVATION_DISTANCE;
use crate::model::{
    BrowserViewMode, ColumnEntryBounds, Message, ScrollbarRegion, SelectionMarquee,
    SelectionMarqueePhase, SelectionMarqueeScrollAnchor, SelectionMarqueeSource,
};

impl FileBrowser {
    pub(in crate::app) fn start_selection_marquee(&mut self) -> Task<Message> {
        let expansion_command = self.dismiss_icon_grid_expansion_from_outside();
        Task::batch([
            expansion_command,
            self.start_selection_marquee_from(SelectionMarqueeSource::PaneBlank),
        ])
    }

    pub(in crate::app) fn start_column_blank_selection_marquee(
        &mut self,
        directory: PathBuf,
    ) -> Task<Message> {
        self.focus_column_from_pointer_click(directory.clone());
        if self.renaming.is_some() {
            return self.handle_column_blank_clicked(directory);
        }

        self.start_selection_marquee_from(SelectionMarqueeSource::ColumnBlank { directory })
    }

    pub(in crate::app) fn start_icon_grid_panel_selection_marquee(
        &mut self,
        directory: PathBuf,
    ) -> Task<Message> {
        if let Some(state) = self.icon_grid_expansion.as_mut() {
            state.set_selection_directory(&directory);
        }
        self.start_selection_marquee_from(SelectionMarqueeSource::IconGridPanel { directory })
    }

    fn start_selection_marquee_from(&mut self, source: SelectionMarqueeSource) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }

        let column_context_directory = match &source {
            SelectionMarqueeSource::ColumnBlank { directory } => {
                self.column_blank_context_directory(directory)
            }
            SelectionMarqueeSource::PaneBlank | SelectionMarqueeSource::IconGridPanel { .. } => {
                None
            }
        };
        self.clear_preview();
        self.context_menu = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        let preserve_existing = self.keyboard_modifiers.control();
        let base_selection = self.selected_paths.clone();
        if matches!(&source, SelectionMarqueeSource::PaneBlank) && !preserve_existing {
            self.set_deepest_open_column_directory(None);
        }
        if !preserve_existing {
            match (&source, column_context_directory) {
                (_, Some(directory)) => self.select_path(directory),
                (SelectionMarqueeSource::ColumnBlank { .. }, None)
                | (SelectionMarqueeSource::IconGridPanel { .. }, _) => {
                    self.clear_file_selection();
                }
                (SelectionMarqueeSource::PaneBlank, None) => {
                    self.clear_column_blank_selection_context();
                }
            }
        }
        self.record_pane_drag_pointer_press();
        let scroll_anchor = self.selection_marquee_scroll_anchor(&source);
        self.selection_marquee = Some(SelectionMarquee {
            gesture_origin: self.cursor_position,
            start: self.cursor_position,
            current: self.cursor_position,
            source,
            phase: SelectionMarqueePhase::WaitingForMovement,
            scroll_anchor,
            base_selection,
            preserve_existing,
        });
        Task::none()
    }

    fn selection_marquee_scroll_anchor(
        &self,
        source: &SelectionMarqueeSource,
    ) -> SelectionMarqueeScrollAnchor {
        let pane_id = self.active_pane_id();
        match self.view_mode {
            BrowserViewMode::List => SelectionMarqueeScrollAnchor::List {
                pane_id,
                offset_y: self
                    .column_viewports
                    .get(&self.current_dir)
                    .map_or(0.0, |viewport| viewport.offset_y),
            },
            BrowserViewMode::Icons => SelectionMarqueeScrollAnchor::Icons {
                pane_id,
                offset_y: self
                    .icon_grid_viewport_for(pane_id, &self.current_dir)
                    .offset_y,
            },
            BrowserViewMode::Columns => match source {
                SelectionMarqueeSource::ColumnBlank { directory } => {
                    SelectionMarqueeScrollAnchor::Column {
                        pane_id,
                        directory: directory.clone(),
                        browser_offset_x: self.column_browser_viewport.offset_x,
                        directory_offset_y: self
                            .column_viewports
                            .get(directory)
                            .map_or(0.0, |viewport| viewport.offset_y),
                    }
                }
                SelectionMarqueeSource::PaneBlank
                | SelectionMarqueeSource::IconGridPanel { .. } => {
                    SelectionMarqueeScrollAnchor::ColumnBrowser {
                        pane_id,
                        offset_x: self.column_browser_viewport.offset_x,
                    }
                }
            },
        }
    }

    pub(in crate::app) fn sync_selection_marquee_scroll(
        &mut self,
        region: ScrollbarRegion,
        offset: f32,
    ) -> Task<Message> {
        let Some(marquee) = self.selection_marquee.as_mut() else {
            return Task::none();
        };
        let changed = marquee.sync_scroll_offset(&region, offset);
        if changed && marquee.is_selecting() {
            crate::column_entry_bounds::column_entry_bounds_command()
        } else {
            Task::none()
        }
    }

    pub(in crate::app) fn update_selection_marquee(&mut self, position: Point) -> bool {
        if let Some(marquee) = &mut self.selection_marquee {
            marquee.current = position;
            if marquee.phase == SelectionMarqueePhase::WaitingForMovement
                && selection_marquee_distance_exceeded(marquee)
            {
                marquee.phase = SelectionMarqueePhase::Selecting;
            }
            return marquee.is_selecting();
        }
        false
    }

    pub(in crate::app) fn update_selection_from_column_entry_bounds(
        &mut self,
        bounds: Vec<ColumnEntryBounds>,
    ) -> Task<Message> {
        self.file_entry_bounds = bounds;
        let Some(marquee) = self
            .selection_marquee
            .as_ref()
            .filter(|marquee| marquee.is_selecting())
        else {
            return Task::none();
        };
        let marquee_rectangle = marquee.rectangle();
        let icon_grid_panel_directory = match &marquee.source {
            SelectionMarqueeSource::IconGridPanel { directory } => Some(directory.as_path()),
            SelectionMarqueeSource::PaneBlank if self.view_mode == BrowserViewMode::Icons => {
                Some(self.current_dir.as_path())
            }
            SelectionMarqueeSource::PaneBlank | SelectionMarqueeSource::ColumnBlank { .. } => None,
        };
        let active_pane_id = self.active_pane_id();
        let visible_paths = self
            .visible_entry_paths()
            .into_iter()
            .collect::<HashSet<_>>();
        let mut next_selection = if marquee.preserve_existing {
            marquee.base_selection.clone()
        } else {
            HashSet::new()
        };

        for entry_bounds in &self.file_entry_bounds {
            if entry_bounds.pane_id == active_pane_id
                && visible_paths.contains(&entry_bounds.path)
                && icon_grid_panel_directory
                    .is_none_or(|directory| entry_bounds.path.parent() == Some(directory))
                && rectangles_intersect(marquee_rectangle, entry_bounds.bounds)
            {
                next_selection.insert(entry_bounds.path.clone());
            }
        }

        self.selected_paths = next_selection;
        self.selected = self.last_visible_selected_path();
        if let Some(selected) = self.selected.clone() {
            self.update_rename_input(&selected);
        } else {
            self.rename_input.clear();
        }
        Task::none()
    }
}

fn selection_marquee_distance_exceeded(marquee: &SelectionMarquee) -> bool {
    let delta_x = marquee.current.x - marquee.gesture_origin.x;
    let delta_y = marquee.current.y - marquee.gesture_origin.y;
    delta_x * delta_x + delta_y * delta_y
        >= POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE
}

pub(super) fn rectangles_intersect(first: Rectangle, second: Rectangle) -> bool {
    first.x < second.x + second.width
        && first.x + first.width > second.x
        && first.y < second.y + second.height
        && first.y + first.height > second.y
}
