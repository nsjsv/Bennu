use std::path::PathBuf;

use wayland_client::protocol::wl_data_device_manager::DndAction;

use super::{
    WaylandDndDropOrigin, WaylandDndError, WaylandFileDragSessionId, FILE_DROP_TARGET_MIME_TYPES,
    INTERNAL_FILE_DRAG_MIME, URI_LIST_MIME,
};
use crate::file_clipboard::{
    parse_file_uri_list, parse_gnome_copied_files, serialize_file_uri_list, FileClipboardOperation,
    FileClipboardSelection, GNOME_COPIED_FILES_MIME,
};

pub(super) struct DragPayload {
    uri_list: String,
}

impl DragPayload {
    pub(super) fn new(paths: &[PathBuf]) -> Self {
        let uri_list = serialize_file_uri_list(paths);
        Self {
            uri_list: if uri_list.is_empty() {
                String::new()
            } else {
                format!("{}\r\n", uri_list.replace('\n', "\r\n"))
            },
        }
    }

    pub(super) fn for_mime(&self, mime: &str) -> Option<&str> {
        match mime {
            INTERNAL_FILE_DRAG_MIME
            | URI_LIST_MIME
            | "text/plain;charset=utf-8"
            | "UTF8_STRING"
            | "text/plain" => Some(&self.uri_list),
            _ => None,
        }
    }
}

pub(super) struct ParsedDropSelection {
    pub(super) selection: FileClipboardSelection,
}

pub(super) fn parse_drop_selection(
    mime_type: &str,
    data: &[u8],
) -> Result<ParsedDropSelection, WaylandDndError> {
    let payload = std::str::from_utf8(data).map_err(|source| WaylandDndError::PayloadUtf8 {
        mime: mime_type.to_owned(),
        source,
    })?;
    let paths = match mime_type {
        INTERNAL_FILE_DRAG_MIME => {
            parse_file_uri_list(payload).map_err(|source| WaylandDndError::Payload {
                mime: mime_type.to_owned(),
                source,
            })?
        }
        GNOME_COPIED_FILES_MIME => parse_gnome_copied_files(payload)
            .map(|selection| selection.paths)
            .map_err(|source| WaylandDndError::Payload {
                mime: mime_type.to_owned(),
                source,
            })?,
        URI_LIST_MIME | "text/plain;charset=utf-8" | "UTF8_STRING" | "text/plain" => {
            parse_file_uri_list(payload).map_err(|source| WaylandDndError::Payload {
                mime: mime_type.to_owned(),
                source,
            })?
        }
        _ => Vec::new(),
    };
    let operation = if mime_type == INTERNAL_FILE_DRAG_MIME {
        FileClipboardOperation::Move
    } else {
        FileClipboardOperation::Copy
    };
    Ok(ParsedDropSelection {
        selection: FileClipboardSelection::new(operation, paths),
    })
}

pub(super) fn drop_origin_for_mime(
    mime_type: &str,
    source_session_id: Option<WaylandFileDragSessionId>,
) -> Result<WaylandDndDropOrigin, WaylandDndError> {
    if mime_type == INTERNAL_FILE_DRAG_MIME {
        source_session_id
            .map(WaylandDndDropOrigin::Internal)
            .ok_or(WaylandDndError::InternalDropSessionUnavailable)
    } else {
        Ok(WaylandDndDropOrigin::External)
    }
}

pub(super) fn negotiated_drop_action(mime_type: &str) -> DndAction {
    if mime_type == INTERNAL_FILE_DRAG_MIME {
        DndAction::Move
    } else {
        DndAction::Copy
    }
}

pub(super) fn pick_mime(mime_types: &[String]) -> Option<String> {
    FILE_DROP_TARGET_MIME_TYPES
        .iter()
        .find(|supported| mime_types.iter().any(|mime| mime == **supported))
        .map(|mime| (*mime).to_owned())
}
