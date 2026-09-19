use std::path::PathBuf;

use iced::{Element, Task};

use crate::file_drag_hit_test_bounds::{
    file_drag_hit_test_bounds_command, FileDragHitTestBoundsRequest, FileDragHitTestMarker,
};
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::model::{BrowserPaneId, Message};

pub(crate) fn track_breadcrumb_viewport<'a>(
    content: impl Into<Element<'a, Message>>,
    pane_id: BrowserPaneId,
) -> Element<'a, Message> {
    track_file_drag_hit_test_marker(
        content,
        FileDragHitTestMarker::BreadcrumbViewport { pane_id },
    )
}

pub(crate) fn track_breadcrumb_drop_target<'a>(
    content: impl Into<Element<'a, Message>>,
    pane_id: BrowserPaneId,
    directory: PathBuf,
) -> Element<'a, Message> {
    track_file_drag_hit_test_marker(
        content,
        FileDragHitTestMarker::BreadcrumbDirectory { pane_id, directory },
    )
}

pub(crate) fn breadcrumb_drop_target_bounds_command(generation: u64) -> Task<Message> {
    file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::Breadcrumbs(generation))
}
