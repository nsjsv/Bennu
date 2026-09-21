use desktop_linux::{
    WaylandDndDropOrigin, WaylandDndDropPosition, WaylandDndFileDrop, WaylandFileDragSessionId,
    WaylandFileDragSourceEvent, WaylandFileDropTargetEvent, WaylandFileDropTargetSessionId,
};
use iced::{Point, Task};

use crate::app::FileBrowser;
use crate::file_drag_hit_test_bounds::{
    file_drag_hit_test_bounds_command, FileDragHitTestBoundsRequest,
};
use crate::model::{
    FileDragNativeDndState, FileDropLayoutState, FileDropOrigin, FileDropSessionIdentity,
    FileDropSessionPhase, FileDropSessionState, InternalFileDragSnapshot, Message,
};

impl FileBrowser {
    pub(in crate::app) fn accept_wayland_target_event(
        &mut self,
        event: WaylandFileDropTargetEvent,
    ) -> Task<Message> {
        match event {
            WaylandFileDropTargetEvent::Entered {
                target_session_id,
                origin,
                position,
            } => self.begin_wayland_file_drop_session(target_session_id, origin, position),
            WaylandFileDropTargetEvent::Moved {
                target_session_id,
                position,
            } => self.move_wayland_file_drop_session(target_session_id, position),
            WaylandFileDropTargetEvent::Left { target_session_id } => {
                self.leave_wayland_file_drop_session(target_session_id);
                Task::none()
            }
            WaylandFileDropTargetEvent::Dropped {
                target_session_id,
                position,
            } => self.drop_wayland_file_drop_session(target_session_id, position),
        }
    }

    pub(in crate::app) fn accept_wayland_file_drop(
        &mut self,
        drop: WaylandDndFileDrop,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Wayland(drop.target_session_id);
        if drop.origin == WaylandDndDropOrigin::External {
            return self.accept_external_file_drop_payload(identity, drop.selection);
        }
        let origin_matches = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity
                && file_drop_origin_matches_wayland(&session.origin, drop.origin)
        });
        if !origin_matches {
            return Task::none();
        }
        if let Some(session) = &mut self.file_drop_session {
            session.pending_payload = Some(drop.selection);
        }
        self.consume_ready_file_drop()
    }

    pub(in crate::app) fn accept_wayland_drop_failure(
        &mut self,
        target_session_id: WaylandFileDropTargetSessionId,
        details: String,
    ) -> Task<Message> {
        self.accept_native_file_drop_failure(
            FileDropSessionIdentity::Wayland(target_session_id),
            details,
        )
    }

    pub(in crate::app) fn accept_wayland_source_event(
        &mut self,
        event: WaylandFileDragSourceEvent,
    ) -> Task<Message> {
        match event {
            WaylandFileDragSourceEvent::Started(session_id) => {
                if let Some(file_drag) = &mut self.file_drag {
                    if file_drag.native_dnd == FileDragNativeDndState::Requested(session_id) {
                        file_drag.native_dnd = FileDragNativeDndState::Started(session_id);
                    }
                }
            }
            WaylandFileDragSourceEvent::Dropped(session_id) => {
                if let Some(file_drag) = &mut self.file_drag {
                    if matches!(
                        file_drag.native_dnd,
                        FileDragNativeDndState::Requested(id)
                            | FileDragNativeDndState::Started(id)
                            if id == session_id
                    ) {
                        file_drag.native_dnd = FileDragNativeDndState::Dropped(session_id);
                        // 落地后不再有 Moved 事件,边缘滚计划必须清掉。
                        self.stop_file_drag_edge_scroll();
                    }
                }
            }
            WaylandFileDragSourceEvent::Finished(session_id)
            | WaylandFileDragSourceEvent::Cancelled(session_id) => {
                self.finish_wayland_file_drag_source(session_id);
            }
            WaylandFileDragSourceEvent::Rejected {
                session_id,
                details,
            } => {
                if self.finish_wayland_file_drag_source(session_id) {
                    self.show_global_error(format!("Could not start Wayland file drag: {details}"));
                }
            }
        }
        Task::none()
    }

    fn begin_wayland_file_drop_session(
        &mut self,
        target_session_id: WaylandFileDropTargetSessionId,
        wayland_origin: WaylandDndDropOrigin,
        position: WaylandDndDropPosition,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Wayland(target_session_id);
        let origin = match wayland_origin {
            WaylandDndDropOrigin::External => {
                return self.begin_native_external_file_drop_session(
                    identity,
                    Point::new(position.x as f32, position.y as f32),
                );
            }
            WaylandDndDropOrigin::Internal(source_session_id) => {
                let Some(file_drag) = self.file_drag.as_ref().filter(|file_drag| {
                    file_drag.is_dragging()
                        && file_drag.native_dnd.session_id() == Some(source_session_id)
                }) else {
                    return Task::none();
                };
                FileDropOrigin::Internal(InternalFileDragSnapshot {
                    source_session_id: Some(source_session_id),
                    sources: file_drag.sources.clone(),
                    bookmark_source: file_drag.bookmark_source.clone(),
                })
            }
        };
        self.clear_file_drop_visuals();
        let request =
            self.next_file_drop_layout_request(identity, self.active_pane_id(), self.active_tab_id);
        self.file_drop_session = Some(FileDropSessionState {
            identity,
            origin,
            phase: FileDropSessionPhase::Hovering,
            layout: FileDropLayoutState::Pending(request),
            position: Some(Point::new(position.x as f32, position.y as f32)),
            hovered_target: None,
            tab_hover: None,
            hover_generation: 0,
            pending_payload: None,
            frozen_drop_target: None,
            scroll_refresh_in_flight: None,
        });

        file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::FileDropLayout(request))
    }

    fn move_wayland_file_drop_session(
        &mut self,
        target_session_id: WaylandFileDropTargetSessionId,
        position: WaylandDndDropPosition,
    ) -> Task<Message> {
        self.move_native_file_drop_session(
            FileDropSessionIdentity::Wayland(target_session_id),
            Point::new(position.x as f32, position.y as f32),
        )
    }

    fn leave_wayland_file_drop_session(
        &mut self,
        target_session_id: WaylandFileDropTargetSessionId,
    ) {
        self.leave_native_file_drop_session(FileDropSessionIdentity::Wayland(target_session_id));
    }

    fn drop_wayland_file_drop_session(
        &mut self,
        target_session_id: WaylandFileDropTargetSessionId,
        position: Option<WaylandDndDropPosition>,
    ) -> Task<Message> {
        self.drop_native_file_drop_session(
            FileDropSessionIdentity::Wayland(target_session_id),
            wayland_drop_position(position),
        )
    }

    fn finish_wayland_file_drag_source(
        &mut self,
        source_session_id: WaylandFileDragSessionId,
    ) -> bool {
        let source_matches = self
            .file_drag
            .as_ref()
            .is_some_and(|file_drag| file_drag.native_dnd.session_id() == Some(source_session_id));
        if source_matches {
            self.file_drag = None;
            self.drag_selection_anchor = None;
            // 拖拽终态后不再有 Moved 事件,边缘滚计划不能残留。
            self.stop_file_drag_edge_scroll();
        }
        let clear_target = self.file_drop_session.as_ref().is_some_and(|session| {
            session.phase == FileDropSessionPhase::Hovering
                && matches!(
                    &session.origin,
                    FileDropOrigin::Internal(snapshot)
                        if snapshot.source_session_id == Some(source_session_id)
                )
        });
        if clear_target {
            self.file_drop_session = None;
            self.clear_file_drop_visuals();
        }
        source_matches || clear_target
    }
}

fn file_drop_origin_matches_wayland(
    origin: &FileDropOrigin,
    wayland_origin: WaylandDndDropOrigin,
) -> bool {
    match (origin, wayland_origin) {
        (FileDropOrigin::External, WaylandDndDropOrigin::External) => true,
        (FileDropOrigin::Internal(snapshot), WaylandDndDropOrigin::Internal(source_session_id)) => {
            snapshot.source_session_id == Some(source_session_id)
        }
        _ => false,
    }
}

fn wayland_drop_position(position: Option<WaylandDndDropPosition>) -> Option<Point> {
    position.map(|position| Point::new(position.x as f32, position.y as f32))
}
