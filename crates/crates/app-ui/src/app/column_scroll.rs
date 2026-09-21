use std::path::Path;

use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Operation, Scrollable as ScrollableOperation};
use iced::widget::scrollable;
use iced::{Rectangle, Task, Vector};

use super::FileBrowser;
use crate::model::{BrowserPaneId, ColumnBrowserViewport, Message, ScrollbarRegion};
use crate::three_column_view::{
    column_directories, sidebar_underlay_width_for_pane, COLUMN_RESIZE_DIVIDER_WIDTH,
};
use crate::view::column_browser_scroll_id;

struct ColumnBrowserRevealColumn {
    target: advanced_widget::Id,
    column_left_x: f32,
    column_right_x: f32,
    real_content_width: f32,
    viewport_underlay_width: f32,
}

impl ColumnBrowserRevealColumn {
    fn new(
        target: iced::widget::Id,
        column_left_x: f32,
        column_right_x: f32,
        real_content_width: f32,
        viewport_underlay_width: f32,
    ) -> Self {
        Self {
            target: target.into(),
            column_left_x,
            column_right_x,
            real_content_width,
            viewport_underlay_width,
        }
    }
}

impl Operation<Message> for ColumnBrowserRevealColumn {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&advanced_widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn ScrollableOperation,
    ) {
        if id == Some(&self.target) {
            let viewport_width =
                programmatic_viewport_width(bounds.width, self.viewport_underlay_width);
            let max_offset_x = (content_bounds.width - bounds.width).max(0.0);
            let current_offset_x = translation.x.max(0.0);
            let target_offset_x = reveal_horizontal_range_scroll_offset(
                current_offset_x,
                viewport_width,
                max_offset_x,
                self.real_content_width,
                self.column_left_x,
                self.column_right_x,
            );
            state.scroll_to(scrollable::AbsoluteOffset {
                x: Some(target_offset_x),
                y: Some(translation.y.max(0.0)),
            });
        }
    }
}

impl FileBrowser {
    pub(super) fn handle_column_browser_scrolled(
        &mut self,
        pane_id: BrowserPaneId,
        offset_x: f32,
        width: f32,
    ) -> Task<Message> {
        let Some(viewport) = column_browser_viewport_from_scroll(offset_x, width) else {
            return Task::none();
        };
        if pane_id == self.active_pane_id() {
            self.column_browser_viewport = viewport;
        } else if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.column_browser_viewport = viewport;
        } else {
            return Task::none();
        }
        Task::batch([
            self.sync_selection_marquee_scroll(ScrollbarRegion::ColumnBrowser(pane_id), offset_x),
            self.request_browser_session_save(),
        ])
    }

    pub(super) fn restore_visible_column_browser_viewports(&self) -> Task<Message> {
        let commands = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .map(|pane_id| self.restore_column_browser_viewport(pane_id))
            .collect::<Vec<_>>();
        Task::batch(commands)
    }

    fn restore_column_browser_viewport(&self, pane_id: BrowserPaneId) -> Task<Message> {
        let Some(viewport) = self.column_browser_viewport_for_pane(pane_id) else {
            return Task::none();
        };
        if viewport.offset_x <= f32::EPSILON || !viewport.offset_x.is_finite() {
            return Task::none();
        }
        iced::widget::operation::scroll_to(
            column_browser_scroll_id(pane_id),
            scrollable::AbsoluteOffset {
                x: viewport.offset_x,
                y: 0.0,
            },
        )
    }

    fn column_browser_viewport_for_pane(
        &self,
        pane_id: BrowserPaneId,
    ) -> Option<ColumnBrowserViewport> {
        if pane_id == self.active_pane_id() {
            return Some(self.column_browser_viewport);
        }
        self.pane_by_id(pane_id)
            .map(|pane| pane.column_browser_viewport)
    }

    pub(super) fn focus_latest_column(&self) -> Task<Message> {
        let real_column_count = column_directories(self).len();
        self.reveal_column_index(real_column_count.saturating_sub(1), real_column_count)
    }

    pub(super) fn focus_column_containing_path(&self, path: &Path) -> Task<Message> {
        let Some(column_index) = self.column_index_containing_path(path) else {
            return Task::none();
        };
        let real_column_count = column_directories(self).len();
        self.reveal_column_index(column_index, real_column_count)
    }

    fn column_index_containing_path(&self, path: &Path) -> Option<usize> {
        let directory = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.current_dir.clone());
        column_directories(self)
            .iter()
            .position(|column_directory| column_directory == &directory)
    }

    fn column_scroll_offset(&self, start_column_index: usize) -> f32 {
        (0..start_column_index)
            .map(|column_index| self.column_width(column_index) + COLUMN_RESIZE_DIVIDER_WIDTH)
            .sum()
    }

    fn reveal_column_index(&self, column_index: usize, real_column_count: usize) -> Task<Message> {
        let pane_id = self.active_pane_id();
        let column_left_x = self.column_scroll_offset(column_index);
        let column_right_x = column_left_x + self.column_width(column_index);
        let real_content_width = real_column_content_width(real_column_count, |column_index| {
            self.column_width(column_index)
        });
        advanced_widget::operate(ColumnBrowserRevealColumn::new(
            column_browser_scroll_id(pane_id),
            column_left_x,
            column_right_x,
            real_content_width,
            sidebar_underlay_width_for_pane(self, pane_id),
        ))
    }
}

fn reveal_horizontal_range_offset(
    current_offset_x: f32,
    viewport_width: f32,
    range_left_x: f32,
    range_right_x: f32,
) -> f32 {
    if range_left_x < current_offset_x {
        range_left_x
    } else if range_right_x > current_offset_x + viewport_width {
        range_right_x - viewport_width
    } else {
        current_offset_x
    }
}

fn reveal_horizontal_range_scroll_offset(
    current_offset_x: f32,
    viewport_width: f32,
    content_max_offset_x: f32,
    real_content_width: f32,
    range_left_x: f32,
    range_right_x: f32,
) -> f32 {
    let target_offset_x = reveal_horizontal_range_offset(
        current_offset_x,
        viewport_width,
        range_left_x,
        range_right_x,
    );
    let max_offset_x = if target_offset_x == current_offset_x {
        content_max_offset_x
    } else {
        content_max_offset_x.min(programmatic_max_scroll_offset(
            real_content_width,
            viewport_width,
        ))
    };
    target_offset_x.clamp(0.0, max_offset_x)
}

fn real_column_content_width(
    real_column_count: usize,
    column_width_at: impl Fn(usize) -> f32,
) -> f32 {
    let column_widths = (0..real_column_count).map(column_width_at).sum::<f32>();
    let divider_widths = real_column_count.saturating_sub(1) as f32 * COLUMN_RESIZE_DIVIDER_WIDTH;
    column_widths + divider_widths
}

fn programmatic_max_scroll_offset(real_content_width: f32, viewport_width: f32) -> f32 {
    (real_content_width - viewport_width).max(0.0)
}

fn programmatic_viewport_width(scrollable_bounds_width: f32, viewport_underlay_width: f32) -> f32 {
    (scrollable_bounds_width - viewport_underlay_width).max(0.0)
}

fn column_browser_viewport_from_scroll(offset_x: f32, width: f32) -> Option<ColumnBrowserViewport> {
    if !offset_x.is_finite() || !width.is_finite() {
        return None;
    }
    Some(ColumnBrowserViewport {
        offset_x: offset_x.max(0.0),
        width: width.max(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_browser_scroll_updates_viewport_and_requests_session_save() {
        let mut config = crate::config::ui_thread_startup_config();
        config.startup_location_policy = crate::config::StartupLocationPolicy::PreviousSession;
        config.save_view_state = config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config);

        drop(browser.handle_column_browser_scrolled(BrowserPaneId::PRIMARY, 245.0, 820.0));

        assert_eq!(
            browser.column_browser_viewport,
            ColumnBrowserViewport {
                offset_x: 245.0,
                width: 820.0,
            }
        );
        assert!(browser.pending_browser_session_save);
    }

    #[test]
    fn real_column_content_width_counts_only_real_columns_and_dividers() {
        assert_eq!(real_column_content_width(0, |_| 180.0), 0.0);
        assert_eq!(real_column_content_width(1, |_| 180.0), 180.0);
        assert_eq!(
            real_column_content_width(4, |_| 180.0),
            4.0 * 180.0 + 3.0 * COLUMN_RESIZE_DIVIDER_WIDTH
        );
    }

    #[test]
    fn programmatic_max_scroll_offset_stops_at_real_content_end() {
        let default_viewport = 4.0 * 180.0 + 3.0 * COLUMN_RESIZE_DIVIDER_WIDTH;
        let five_columns = 5.0 * 180.0 + 4.0 * COLUMN_RESIZE_DIVIDER_WIDTH;

        assert_eq!(
            programmatic_max_scroll_offset(default_viewport, default_viewport),
            0.0
        );
        assert_eq!(
            programmatic_max_scroll_offset(five_columns, default_viewport),
            180.0 + COLUMN_RESIZE_DIVIDER_WIDTH
        );
        assert_eq!(programmatic_max_scroll_offset(five_columns, 500.0), 420.0);
        assert_eq!(programmatic_max_scroll_offset(five_columns, 1000.0), 0.0);
    }

    #[test]
    fn programmatic_viewport_width_excludes_sidebar_underlay() {
        assert_eq!(programmatic_viewport_width(1000.0, 200.0), 800.0);
        assert_eq!(programmatic_viewport_width(100.0, 200.0), 0.0);
    }

    #[test]
    fn reveal_horizontal_range_scrolls_right_until_target_is_visible() {
        assert_eq!(
            reveal_horizontal_range_offset(0.0, 720.0, 740.0, 920.0),
            200.0
        );
    }

    #[test]
    fn reveal_horizontal_range_keeps_visible_target_stable() {
        assert_eq!(
            reveal_horizontal_range_offset(200.0, 720.0, 740.0, 920.0),
            200.0
        );
    }

    #[test]
    fn reveal_horizontal_range_scrolls_left_when_target_is_before_viewport() {
        assert_eq!(
            reveal_horizontal_range_offset(500.0, 720.0, 240.0, 420.0),
            240.0
        );
    }

    #[test]
    fn reveal_horizontal_range_scroll_offset_keeps_blank_space_when_target_fills_it() {
        assert_eq!(
            reveal_horizontal_range_scroll_offset(555.0, 740.0, 740.0, 925.0, 745.0, 925.0),
            555.0
        );
    }

    #[test]
    fn reveal_horizontal_range_scroll_offset_scrolls_after_blank_space_is_filled() {
        assert_eq!(
            reveal_horizontal_range_scroll_offset(555.0, 740.0, 1295.0, 1480.0, 1300.0, 1480.0),
            740.0
        );
    }

    #[test]
    fn reveal_horizontal_range_scroll_offset_avoids_programmatic_blank_space() {
        assert_eq!(
            reveal_horizontal_range_scroll_offset(0.0, 740.0, 1295.0, 1480.0, 1300.0, 1480.0),
            740.0
        );
    }
}
