use x11rb::protocol::xproto::{Atom, Window};

use super::{X11DndDropPosition, X11FileDropTargetSessionId};

#[derive(Debug, Clone)]
pub(super) struct TargetSession {
    pub id: X11FileDropTargetSessionId,
    pub source: Window,
    pub protocol_target: Window,
    pub version: u8,
    pub accepts_uri_list: bool,
    pub ui_entered: bool,
    pub last_position: Option<X11DndDropPosition>,
    pub drop_requested: bool,
    pub finished: bool,
}

impl TargetSession {
    pub fn new(
        source: Window,
        protocol_target: Window,
        version: u8,
        accepts_uri_list: bool,
    ) -> Self {
        Self {
            id: X11FileDropTargetSessionId::unique(),
            source,
            protocol_target,
            version,
            accepts_uri_list,
            ui_entered: false,
            last_position: None,
            drop_requested: false,
            finished: false,
        }
    }

    pub fn matches(&self, source: Window, protocol_target: Window) -> bool {
        self.source == source && self.protocol_target == protocol_target
    }

    pub fn acknowledge_position(&mut self, position: X11DndDropPosition) -> bool {
        if self.finished || !self.accepts_uri_list {
            return false;
        }
        self.last_position = Some(position);
        true
    }

    pub fn mark_drop_requested(&mut self) {
        self.drop_requested = true;
    }

    pub fn freeze_drop(
        &self,
        source: Window,
        protocol_target: Window,
        current_scale_generation: u64,
    ) -> Option<X11DndDropPosition> {
        self.matches(source, protocol_target)
            .then_some(self.last_position)
            .flatten()
            .filter(|position| position.scale_generation == current_scale_generation)
    }

    pub fn finish_once(&mut self) -> bool {
        if self.finished {
            false
        } else {
            self.finished = true;
            true
        }
    }
}

pub(super) fn source_conflicts_with_target_windows(
    source: Window,
    main_window: Window,
    proxy_window: Window,
) -> bool {
    source == main_window || source == proxy_window
}

pub(super) fn unpack_signed_root_position(packed: u32) -> (i16, i16) {
    ((packed >> 16) as u16 as i16, packed as u16 as i16)
}

pub(super) fn offered_types(inline_types: [Atom; 3], type_list: Option<&[Atom]>) -> Vec<Atom> {
    type_list
        .map_or_else(|| inline_types.to_vec(), <[Atom]>::to_vec)
        .into_iter()
        .filter(|atom| *atom != 0)
        .collect()
}

pub(super) fn status_data(protocol_target: Window, accepted: bool, copy_action: Atom) -> [u32; 5] {
    [
        protocol_target,
        if accepted { 0b11 } else { 0 },
        0,
        0,
        if accepted { copy_action } else { 0 },
    ]
}

pub(super) fn finished_data(
    protocol_target: Window,
    version: u8,
    success: bool,
    copy_action: Atom,
) -> [u32; 5] {
    [
        protocol_target,
        if version >= 5 && success { 1 } else { 0 },
        if success { copy_action } else { 0 },
        0,
        0,
    ]
}
