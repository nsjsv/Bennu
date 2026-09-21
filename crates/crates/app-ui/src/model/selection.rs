use std::collections::HashSet;
use std::path::PathBuf;

use iced::{Point, Rectangle};

use super::{BrowserPaneId, ScrollbarRegion};

#[derive(Debug, Clone)]
pub(crate) struct ColumnEntryBounds {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) path: PathBuf,
    pub(crate) bounds: Rectangle,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectionMarquee {
    pub(crate) gesture_origin: Point,
    pub(crate) start: Point,
    pub(crate) current: Point,
    pub(crate) source: SelectionMarqueeSource,
    pub(crate) phase: SelectionMarqueePhase,
    pub(crate) scroll_anchor: SelectionMarqueeScrollAnchor,
    pub(crate) base_selection: HashSet<PathBuf>,
    pub(crate) preserve_existing: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum SelectionMarqueeScrollAnchor {
    List {
        pane_id: BrowserPaneId,
        offset_y: f32,
    },
    Icons {
        pane_id: BrowserPaneId,
        offset_y: f32,
    },
    ColumnBrowser {
        pane_id: BrowserPaneId,
        offset_x: f32,
    },
    Column {
        pane_id: BrowserPaneId,
        directory: PathBuf,
        browser_offset_x: f32,
        directory_offset_y: f32,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum SelectionMarqueeSource {
    PaneBlank,
    ColumnBlank { directory: PathBuf },
    IconGridPanel { directory: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionMarqueePhase {
    WaitingForMovement,
    Selecting,
}

impl SelectionMarquee {
    pub(crate) fn top_left(&self) -> Point {
        Point::new(
            self.start.x.min(self.current.x),
            self.start.y.min(self.current.y),
        )
    }

    pub(crate) fn width(&self) -> f32 {
        (self.current.x - self.start.x).abs().max(1.0)
    }

    pub(crate) fn height(&self) -> f32 {
        (self.current.y - self.start.y).abs().max(1.0)
    }

    pub(crate) fn rectangle(&self) -> Rectangle {
        let top_left = self.top_left();
        Rectangle {
            x: top_left.x,
            y: top_left.y,
            width: self.width(),
            height: self.height(),
        }
    }

    pub(crate) fn is_selecting(&self) -> bool {
        self.phase == SelectionMarqueePhase::Selecting
    }

    pub(crate) fn sync_scroll_offset(&mut self, region: &ScrollbarRegion, offset: f32) -> bool {
        if !offset.is_finite() {
            return false;
        }
        let offset = offset.max(0.0);

        let previous_offset = match (&mut self.scroll_anchor, region) {
            (
                SelectionMarqueeScrollAnchor::List { pane_id, offset_y },
                ScrollbarRegion::PaneList(scrolled_pane_id),
            ) if pane_id == scrolled_pane_id => offset_y,
            (
                SelectionMarqueeScrollAnchor::Icons { pane_id, offset_y },
                ScrollbarRegion::PaneIcons(scrolled_pane_id),
            ) if pane_id == scrolled_pane_id => offset_y,
            (
                SelectionMarqueeScrollAnchor::ColumnBrowser { pane_id, offset_x },
                ScrollbarRegion::ColumnBrowser(scrolled_pane_id),
            ) if pane_id == scrolled_pane_id => offset_x,
            (
                SelectionMarqueeScrollAnchor::Column {
                    pane_id,
                    browser_offset_x,
                    ..
                },
                ScrollbarRegion::ColumnBrowser(scrolled_pane_id),
            ) if pane_id == scrolled_pane_id => browser_offset_x,
            (
                SelectionMarqueeScrollAnchor::Column {
                    pane_id,
                    directory,
                    directory_offset_y,
                    ..
                },
                ScrollbarRegion::Column {
                    pane_id: scrolled_pane_id,
                    directory: scrolled_directory,
                },
            ) if pane_id == scrolled_pane_id && directory == scrolled_directory => {
                directory_offset_y
            }
            _ => return false,
        };
        let delta = offset - *previous_offset;
        *previous_offset = offset;
        match region {
            ScrollbarRegion::ColumnBrowser(_) => self.start.x -= delta,
            ScrollbarRegion::PaneList(_)
            | ScrollbarRegion::PaneIcons(_)
            | ScrollbarRegion::Column { .. } => self.start.y -= delta,
            _ => return false,
        }
        delta.abs() > f32::EPSILON
    }
}
