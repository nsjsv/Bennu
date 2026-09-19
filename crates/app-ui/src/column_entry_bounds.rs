use std::path::PathBuf;

use iced::{Element, Task};

use crate::file_drag_hit_test_bounds::{
    file_drag_hit_test_bounds_command, FileDragHitTestBoundsRequest, FileDragHitTestMarker,
};
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::model::{BrowserPaneId, Message};

pub(crate) fn track_column_entry_bounds<'a>(
    content: impl Into<Element<'a, Message>>,
    pane_id: BrowserPaneId,
    path: PathBuf,
) -> Element<'a, Message> {
    track_file_drag_hit_test_marker(
        content,
        FileDragHitTestMarker::ColumnEntry { pane_id, path },
    )
}

pub(crate) fn column_entry_bounds_command() -> Task<Message> {
    file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::SelectionMarquee)
}
