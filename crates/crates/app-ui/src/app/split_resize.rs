use iced::{Point, Task};

use crate::model::{BrowserPaneLayout, Message, SplitAxis};

use super::FileBrowser;

#[derive(Debug, Clone, Copy)]
pub(super) struct SplitResizeDrag {
    axis: SplitAxis,
    pointer_at_press: f32,
    portion_at_press: u16,
    axis_extent: f32,
    changed: bool,
}

impl FileBrowser {
    pub(super) fn start_split_resize(&mut self, position: Point) -> Task<Message> {
        let BrowserPaneLayout::Split { axis, .. } = self.pane_layout else {
            return Task::none();
        };
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }
        let axis_extent = self.split_axis_extent(axis);
        let pointer_at_press = axis.pointer_coordinate(position);
        let portion_at_press = self.pane_layout.effective_split_portions(axis_extent).0;

        self.clear_pointer_driven_interaction_state();
        self.clear_preview();
        self.context_menu = None;
        self.split_resize_drag = Some(SplitResizeDrag {
            axis,
            pointer_at_press,
            portion_at_press,
            axis_extent,
            changed: false,
        });
        Task::none()
    }

    pub(super) fn update_split_resize(&mut self, position: Point) {
        let Some(drag) = self.split_resize_drag else {
            return;
        };
        let usable_extent = (drag.axis_extent - crate::model::SPLIT_DIVIDER_WIDTH).max(1.0);
        let delta = drag.axis.pointer_coordinate(position) - drag.pointer_at_press;
        let requested = drag.portion_at_press as f32
            + delta / usable_extent * crate::model::SPLIT_PORTION_TOTAL as f32;
        let next = self
            .pane_layout
            .effective_split_portions(drag.axis_extent)
            .0;
        let requested = requested
            .round()
            .clamp(1.0, (crate::model::SPLIT_PORTION_TOTAL - 1) as f32)
            as u16;
        let effective = self
            .pane_layout
            .with_first_portion(requested)
            .effective_split_portions(drag.axis_extent)
            .0;
        if effective != next {
            self.pane_layout = self.pane_layout.with_first_portion(effective);
            if let Some(active_drag) = &mut self.split_resize_drag {
                active_drag.changed = true;
            }
        }
    }

    pub(super) fn finish_split_resize(&mut self) -> Task<Message> {
        let Some(drag) = self.split_resize_drag.take() else {
            return Task::none();
        };
        if drag.changed {
            self.request_browser_session_save()
        } else {
            Task::none()
        }
    }

    pub(crate) fn split_axis_extent(&self, axis: SplitAxis) -> f32 {
        match axis {
            SplitAxis::Horizontal => (self.main_window_width - self.sidebar_width).max(0.0),
            SplitAxis::Vertical => self.main_window_height.max(0.0),
        }
    }
}

impl SplitAxis {
    fn pointer_coordinate(self, position: Point) -> f32 {
        match self {
            Self::Horizontal => position.x,
            Self::Vertical => position.y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BrowserPaneId;

    fn split_browser_with_config(
        axis: SplitAxis,
        config: crate::config::UserConfig,
    ) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config);
        browser.main_window_width = 1_000.0;
        browser.main_window_height = 800.0;
        browser.sidebar_width = 200.0;
        browser.pane_layout = BrowserPaneLayout::Split {
            axis,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion: 500,
        };
        browser
    }

    fn split_browser(axis: SplitAxis) -> FileBrowser {
        split_browser_with_config(axis, crate::config::default_user_config())
    }

    #[test]
    fn split_resize_updates_both_axes() {
        let mut horizontal = split_browser(SplitAxis::Horizontal);
        drop(horizontal.start_split_resize(Point::new(600.0, 400.0)));
        horizontal.update_split_resize(Point::new(700.0, 400.0));
        assert!(horizontal.pane_layout.first_portion() > 500);

        let mut vertical = split_browser(SplitAxis::Vertical);
        drop(vertical.start_split_resize(Point::new(600.0, 400.0)));
        vertical.update_split_resize(Point::new(600.0, 500.0));
        assert!(vertical.pane_layout.first_portion() > 500);
    }

    #[test]
    fn split_resize_clears_competing_pointer_interaction() {
        let mut browser = split_browser(SplitAxis::Horizontal);
        drop(browser.start_sidebar_resize_drag());
        assert!(browser.sidebar_resize_drag.is_some());

        drop(browser.start_split_resize(Point::new(600.0, 400.0)));

        assert!(browser.sidebar_resize_drag.is_none());
        assert!(browser.split_resize_drag.is_some());
    }

    #[test]
    fn split_resize_does_not_save_when_view_state_persistence_is_disabled() {
        let mut browser = split_browser(SplitAxis::Horizontal);

        drop(browser.start_split_resize(Point::new(600.0, 400.0)));
        browser.update_split_resize(Point::new(700.0, 400.0));
        drop(browser.finish_split_resize());

        assert!(!browser.pending_browser_session_save);
    }

    #[test]
    fn split_resize_requests_session_save_only_after_change() {
        let mut config = crate::config::ui_thread_startup_config();
        config.startup_location_policy = crate::config::StartupLocationPolicy::PreviousSession;
        config.save_view_state = config.startup_location_policy.saves_view_state();
        let mut browser = split_browser_with_config(SplitAxis::Horizontal, config);

        drop(browser.start_split_resize(Point::new(600.0, 400.0)));
        drop(browser.finish_split_resize());
        assert!(!browser.pending_browser_session_save);

        drop(browser.start_split_resize(Point::new(600.0, 400.0)));
        browser.update_split_resize(Point::new(700.0, 400.0));
        drop(browser.finish_split_resize());
        assert!(browser.pending_browser_session_save);
    }
}
