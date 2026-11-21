use std::path::Path;

use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Operation, Scrollable as ScrollableOperation};
use iced::widget::scrollable;
use iced::{Rectangle, Task, Vector};

use super::FileBrowser;
use crate::model::{BrowserPaneId, ColumnBrowserViewport, Message, ScrollbarRegion};
use crate::three_column_view::{column_directories, COLUMN_RESIZE_DIVIDER_WIDTH};
use crate::view::column_browser_scroll_id;

/// 让一栏整个露出来，滚得最少：在左边外面就贴左边，在右边外面就贴右边，已经看得见就不动。
/// 内容到最后一栏为止，所以怎么滚屏上都有栏。
struct ColumnBrowserRevealColumn {
    target: advanced_widget::Id,
    column_left_x: f32,
    column_right_x: f32,
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
            let max_offset_x = (content_bounds.width - bounds.width).max(0.0);
            let target_offset_x = reveal_horizontal_range_offset(
                translation.x.max(0.0),
                bounds.width,
                self.column_left_x,
                self.column_right_x,
            )
            .clamp(0.0, max_offset_x);
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
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return Task::none();
        };
        pane.column_browser_viewport = viewport;
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
        self.pane_by_id(pane_id)
            .map(|pane| pane.column_browser_viewport)
    }

    pub(super) fn focus_latest_column(&self) -> Task<Message> {
        self.reveal_column_index(column_directories(self).len().saturating_sub(1))
    }

    pub(super) fn focus_column_containing_path(&self, path: &Path) -> Task<Message> {
        let Some(column_index) = self.column_index_containing_path(path) else {
            return Task::none();
        };
        self.reveal_column_index(column_index)
    }

    fn column_index_containing_path(&self, path: &Path) -> Option<usize> {
        let directory = path.parent().or(self.tab().current_directory())?;
        column_directories(self)
            .iter()
            .position(|column_directory| column_directory == directory)
    }

    fn column_scroll_offset(&self, start_column_index: usize) -> f32 {
        (0..start_column_index)
            .map(|column_index| self.column_width(column_index) + COLUMN_RESIZE_DIVIDER_WIDTH)
            .sum()
    }

    fn reveal_column_index(&self, column_index: usize) -> Task<Message> {
        let column_left_x = self.column_scroll_offset(column_index);
        advanced_widget::operate(ColumnBrowserRevealColumn {
            target: column_browser_scroll_id(self.active_pane_id()).into(),
            column_left_x,
            column_right_x: column_left_x + self.column_width(column_index),
        })
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
            browser.pane().column_browser_viewport,
            ColumnBrowserViewport {
                offset_x: 245.0,
                width: 820.0,
            }
        );
        assert!(browser.pending_browser_session_save);
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
}
