use desktop_linux::{
    FileClipboardOperation, FileClipboardSelection, X11DndDropPosition, X11DndFileDrop,
    X11FileDropTargetEvent,
};
use iced::{Point, Task};

use crate::app::FileBrowser;
use crate::model::{FileDropSessionIdentity, FileDropSessionPhase, Message};

impl FileBrowser {
    pub(in crate::app) fn accept_x11_target_event(
        &mut self,
        event: X11FileDropTargetEvent,
        scale_factor: f32,
        scale_generation: u64,
    ) -> Task<Message> {
        match event {
            X11FileDropTargetEvent::Entered {
                target_session_id,
                position,
            } => {
                let Some(position) = logical_position(position, scale_factor, scale_generation)
                else {
                    return Task::none();
                };
                self.begin_native_external_file_drop_session(
                    FileDropSessionIdentity::X11(target_session_id),
                    position,
                )
            }
            X11FileDropTargetEvent::Moved {
                target_session_id,
                position,
            } => {
                let Some(position) = logical_position(position, scale_factor, scale_generation)
                else {
                    return Task::none();
                };
                self.move_native_file_drop_session(
                    FileDropSessionIdentity::X11(target_session_id),
                    position,
                )
            }
            X11FileDropTargetEvent::Left { target_session_id } => {
                self.leave_native_file_drop_session(FileDropSessionIdentity::X11(
                    target_session_id,
                ));
                Task::none()
            }
            X11FileDropTargetEvent::Dropped {
                target_session_id,
                position,
            } => {
                let identity = FileDropSessionIdentity::X11(target_session_id);
                let position = logical_position(position, scale_factor, scale_generation);
                if position.is_none() {
                    self.cancel_file_drop_session(identity);
                    return Task::none();
                }
                self.drop_native_file_drop_session(identity, position)
            }
        }
    }

    pub(in crate::app) fn accept_x11_file_drop(&mut self, drop: X11DndFileDrop) -> Task<Message> {
        self.accept_external_file_drop_payload(
            FileDropSessionIdentity::X11(drop.target_session_id),
            FileClipboardSelection::new(FileClipboardOperation::Copy, drop.paths),
        )
    }

    pub(in crate::app) fn accept_x11_drop_failure(
        &mut self,
        target_session_id: desktop_linux::X11FileDropTargetSessionId,
        details: String,
    ) -> Task<Message> {
        self.accept_native_file_drop_failure(
            FileDropSessionIdentity::X11(target_session_id),
            details,
        )
    }

    pub(in crate::app) fn invalidate_x11_file_drop_for_scale_change(&mut self) -> Task<Message> {
        let Some((identity, phase)) = self.file_drop_session.as_ref().and_then(|session| {
            matches!(session.identity, FileDropSessionIdentity::X11(_))
                .then_some((session.identity, session.phase))
        }) else {
            return Task::none();
        };
        if phase == FileDropSessionPhase::Dropped {
            self.cancel_file_drop_session(identity);
            return Task::none();
        }
        if let Some(session) = &mut self.file_drop_session {
            session.position = None;
            session.hovered_target = None;
            session.tab_hover = None;
            session.frozen_drop_target = None;
        }
        self.request_file_drop_layout_measurement(self.active_pane_id(), self.active_tab_id)
    }

    pub(in crate::app) fn cancel_x11_file_drop_session(&mut self) {
        let identity = self.file_drop_session.as_ref().and_then(|session| {
            matches!(session.identity, FileDropSessionIdentity::X11(_)).then_some(session.identity)
        });
        if let Some(identity) = identity {
            self.cancel_file_drop_session(identity);
        }
    }
}

fn logical_position(
    position: X11DndDropPosition,
    scale_factor: f32,
    scale_generation: u64,
) -> Option<Point> {
    if position.scale_generation != scale_generation
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
    {
        return None;
    }
    let x = f32::from(position.client_x) / scale_factor;
    let y = f32::from(position.client_y) / scale_factor;
    (x.is_finite() && y.is_finite()).then_some(Point::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_position_uses_matching_scale_generation() {
        let position = X11DndDropPosition {
            root_x: 20,
            root_y: 40,
            client_x: 15,
            client_y: 25,
            timestamp: 7,
            scale_generation: 3,
        };
        assert_eq!(
            logical_position(position, 1.25, 3),
            Some(Point::new(12.0, 20.0))
        );
        assert_eq!(logical_position(position, 1.25, 4), None);
        assert_eq!(logical_position(position, 0.0, 3), None);
    }
}
