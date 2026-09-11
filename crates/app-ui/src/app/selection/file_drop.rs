mod wayland;
mod x11;

#[cfg(test)]
mod column_tab_tests;

use std::path::PathBuf;
use std::time::Duration;

use desktop_linux::{FileClipboardOperation, FileClipboardSelection};
use iced::{Point, Task};

use super::super::FileBrowser;
use super::drag::{resolve_file_drag_target, safe_file_drop_target};
use super::file_drop_target::{freeze_file_drop_hit_test_bounds, resolve_file_drop_target};
use crate::breadcrumb_drop_target_bounds::breadcrumb_drop_target_bounds_command;
use crate::file_drag_hit_test_bounds::{
    file_drag_hit_test_bounds_command, FileDragHitTestBoundsRequest,
};
use crate::model::{
    BreadcrumbDropTargetBounds, FileDragHitTestBounds, FileDragState, FileDropLayoutRequest,
    FileDropLayoutState, FileDropOrigin, FileDropSessionIdentity, FileDropSessionPhase,
    FileDropSessionState, FileDropTarget, FrozenFileDropTarget, InternalFileDragSnapshot, Message,
    TabDropDestination, TabDropHover, TabFileDropTarget,
};

const TAB_FILE_DROP_HOVER_DELAY: Duration = Duration::from_millis(500);

impl FileBrowser {
    pub(super) fn begin_native_external_file_drop_session(
        &mut self,
        identity: FileDropSessionIdentity,
        position: Point,
    ) -> Task<Message> {
        self.clear_file_drop_visuals();
        let request =
            self.next_file_drop_layout_request(identity, self.active_pane_id(), self.active_tab_id);
        self.file_drop_session = Some(FileDropSessionState {
            identity,
            origin: FileDropOrigin::External,
            phase: FileDropSessionPhase::Hovering,
            layout: FileDropLayoutState::Pending(request),
            position: Some(position),
            hovered_target: None,
            tab_hover: None,
            hover_generation: 0,
            pending_payload: None,
            frozen_drop_target: None,
        });
        file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::FileDropLayout(request))
    }

    pub(super) fn move_native_file_drop_session(
        &mut self,
        identity: FileDropSessionIdentity,
        position: Point,
    ) -> Task<Message> {
        let Some(session) = &mut self.file_drop_session else {
            return Task::none();
        };
        if session.identity != identity || session.phase != FileDropSessionPhase::Hovering {
            return Task::none();
        }
        session.position = Some(position);
        self.cursor_position = position;
        self.refresh_hovered_target_from_ready_layout()
    }

    pub(super) fn leave_native_file_drop_session(&mut self, identity: FileDropSessionIdentity) {
        let hovering = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity && session.phase == FileDropSessionPhase::Hovering
        });
        if hovering {
            self.file_drop_session = None;
            self.clear_file_drop_visuals();
        }
    }

    pub(super) fn drop_native_file_drop_session(
        &mut self,
        identity: FileDropSessionIdentity,
        position: Option<Point>,
    ) -> Task<Message> {
        let Some(session) = &mut self.file_drop_session else {
            return Task::none();
        };
        if session.identity != identity || session.phase != FileDropSessionPhase::Hovering {
            return Task::none();
        }
        session.phase = FileDropSessionPhase::Dropped;
        session.hovered_target = None;
        session.tab_hover = None;
        if let Some(position) = position {
            session.position = Some(position);
            self.cursor_position = position;
        } else {
            session.position = None;
            session.frozen_drop_target = Some(FrozenFileDropTarget::Rejected);
        }
        self.freeze_file_drop_target_from_ready_layout();
        self.clear_file_drop_visuals();
        self.consume_ready_file_drop()
    }

    pub(super) fn accept_external_file_drop_payload(
        &mut self,
        identity: FileDropSessionIdentity,
        selection: FileClipboardSelection,
    ) -> Task<Message> {
        let matches = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity && session.origin == FileDropOrigin::External
        });
        if !matches {
            return Task::none();
        }
        if let Some(session) = &mut self.file_drop_session {
            session.pending_payload = Some(selection);
        }
        self.consume_ready_file_drop()
    }

    pub(super) fn accept_native_file_drop_failure(
        &mut self,
        identity: FileDropSessionIdentity,
        details: String,
    ) -> Task<Message> {
        if !self.cancel_file_drop_session(identity) {
            return Task::none();
        }
        self.show_global_error(details);
        Task::none()
    }

    pub(super) fn begin_iced_file_drop_session(&mut self) -> Task<Message> {
        let Some(snapshot) = self.internal_file_drag_snapshot() else {
            return Task::none();
        };
        let Some(file_drag) = &self.file_drag else {
            return Task::none();
        };
        let identity = FileDropSessionIdentity::Iced(file_drag.gesture_id);
        let request =
            self.next_file_drop_layout_request(identity, self.active_pane_id(), self.active_tab_id);
        self.file_drop_session = Some(FileDropSessionState {
            identity,
            origin: FileDropOrigin::Internal(snapshot),
            phase: FileDropSessionPhase::Hovering,
            layout: FileDropLayoutState::Pending(request),
            position: Some(self.cursor_position),
            hovered_target: None,
            tab_hover: None,
            hover_generation: 0,
            pending_payload: None,
            frozen_drop_target: None,
        });
        file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::FileDropLayout(request))
    }

    pub(super) fn finish_iced_file_drop(
        &mut self,
        file_drag: FileDragState,
        release_directory: Option<PathBuf>,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Iced(file_drag.gesture_id);
        let session_target = self
            .file_drop_session
            .as_ref()
            .filter(|session| session.identity == identity)
            .and_then(|session| session.hovered_target.clone());
        let fallback = self.file_drag_drop_directory_at_cursor();
        self.file_drop_session = None;
        self.clear_internal_file_drop_visuals();
        let target = resolve_file_drag_target(
            &file_drag.sources,
            release_directory,
            session_target,
            fallback,
        );
        self.dispatch_file_drop(
            FileDropOrigin::Internal(InternalFileDragSnapshot {
                source_session_id: None,
                sources: file_drag.sources.clone(),
                bookmark_source: file_drag.bookmark_source,
            }),
            target,
            file_drag.sources,
        )
    }

    pub(in crate::app) fn accept_drop_layout(
        &mut self,
        request: FileDropLayoutRequest,
        measured: FileDragHitTestBounds,
    ) -> Task<Message> {
        let request_matches = self.file_drop_session.as_ref().is_some_and(|session| {
            matches!(session.layout, FileDropLayoutState::Pending(pending) if pending == request)
        });
        if !request_matches {
            return Task::none();
        }
        if self.active_pane_id() != request.pane_id || self.active_tab_id != request.tab_id {
            self.cancel_file_drop_session(request.identity);
            return Task::none();
        }

        let hit_test_bounds = freeze_file_drop_hit_test_bounds(self, measured);
        if let Some(session) = &mut self.file_drop_session {
            session.layout = FileDropLayoutState::Ready {
                request,
                hit_test_bounds,
            };
        }

        match self.file_drop_session.as_ref().map(|session| session.phase) {
            Some(FileDropSessionPhase::Hovering) => self.refresh_hovered_target_from_ready_layout(),
            Some(FileDropSessionPhase::Dropped) => {
                self.freeze_file_drop_target_from_ready_layout();
                self.consume_ready_file_drop()
            }
            None => Task::none(),
        }
    }

    pub(in crate::app) fn accept_tab_file_drop_entered(
        &mut self,
        gesture_id: crate::model::FileDragGestureId,
        target: TabFileDropTarget,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Iced(gesture_id);
        let is_matching_iced_session = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity && session.phase == FileDropSessionPhase::Hovering
        });
        if !is_matching_iced_session {
            return Task::none();
        }
        self.set_file_drop_tab_target(target)
    }

    pub(in crate::app) fn accept_tab_file_drop_exited(
        &mut self,
        gesture_id: crate::model::FileDragGestureId,
        target: TabFileDropTarget,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Iced(gesture_id);
        let matches = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity
                && session.hovered_target.as_ref() == Some(&FileDropTarget::Tab(target))
        });
        if matches {
            self.set_file_drop_target(None);
        }
        Task::none()
    }

    pub(in crate::app) fn accept_tab_file_drop_released(
        &mut self,
        gesture_id: crate::model::FileDragGestureId,
        target: TabFileDropTarget,
    ) -> Task<Message> {
        let identity = FileDropSessionIdentity::Iced(gesture_id);
        let session_matches = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == identity
                && session.phase == FileDropSessionPhase::Hovering
                && session.hovered_target.as_ref() == Some(&FileDropTarget::Tab(target.clone()))
        });
        let source_matches = self
            .file_drag
            .as_ref()
            .is_some_and(|file_drag| file_drag.gesture_id == gesture_id);
        if !session_matches || !source_matches {
            return Task::none();
        }
        let file_drag = self.file_drag.take().expect("matching Iced file drag");
        self.file_drop_session = None;
        self.clear_internal_file_drop_visuals();
        self.dispatch_file_drop(
            FileDropOrigin::Internal(InternalFileDragSnapshot {
                source_session_id: None,
                sources: file_drag.sources.clone(),
                bookmark_source: file_drag.bookmark_source,
            }),
            Some(FileDropTarget::Tab(target)),
            file_drag.sources,
        )
    }

    pub(in crate::app) fn accept_tab_hover_elapsed(
        &mut self,
        hover: TabDropHover,
    ) -> Task<Message> {
        if !tab_file_drop_hover_delay_elapsed(hover.started_at, std::time::Instant::now()) {
            return Task::none();
        }
        let hover_matches = self.file_drop_session.as_ref().is_some_and(|session| {
            session.identity == hover.identity
                && session.phase == FileDropSessionPhase::Hovering
                && session.tab_hover.as_ref() == Some(&hover)
                && session.hovered_target.as_ref()
                    == Some(&FileDropTarget::Tab(hover.target.clone()))
                && file_drop_layout_request(&session.layout).generation == hover.layout_generation
        });
        if !hover_matches || !self.tab_file_drop_target_is_selectable(&hover.target) {
            return Task::none();
        }
        if self.tab_file_drop_target_is_current(&hover.target) {
            if let Some(session) = &mut self.file_drop_session {
                session.tab_hover = None;
            }
            return Task::none();
        }

        let measure_layout =
            self.request_file_drop_layout_measurement(hover.target.pane_id, hover.target.tab_id);
        let select_tab = self.select_tab_for_file_drop(hover.target.pane_id, hover.target.tab_id);
        Task::batch([select_tab, measure_layout])
    }

    pub(in crate::app) fn invalidate_file_drop_for_tab_close(&mut self) {
        if let Some(identity) = self
            .file_drop_session
            .as_ref()
            .map(|session| session.identity)
        {
            self.cancel_file_drop_session(identity);
        }
    }

    pub(crate) fn set_file_drop_target(&mut self, target: Option<FileDropTarget>) -> bool {
        let Some(session) = &mut self.file_drop_session else {
            return false;
        };
        if session.phase != FileDropSessionPhase::Hovering || session.hovered_target == target {
            return false;
        }
        session.hover_generation = session.hover_generation.wrapping_add(1);
        session.hovered_target = target.clone();
        session.tab_hover = None;
        self.sidebar_bookmark_drop_slot = match target {
            Some(FileDropTarget::SidebarBookmarkSlot(slot)) => Some(slot),
            _ => None,
        };
        true
    }

    pub(in crate::app) fn remeasure_active_file_drop_layout(&mut self) -> Task<Message> {
        self.request_file_drop_layout_measurement(self.active_pane_id(), self.active_tab_id)
    }

    fn request_file_drop_layout_measurement(
        &mut self,
        pane_id: crate::model::BrowserPaneId,
        tab_id: usize,
    ) -> Task<Message> {
        let Some((identity, phase)) = self.file_drop_session.as_ref().and_then(|session| {
            session
                .frozen_drop_target
                .is_none()
                .then_some((session.identity, session.phase))
        }) else {
            return Task::none();
        };
        let request = self.next_file_drop_layout_request(identity, pane_id, tab_id);
        if let Some(session) = &mut self.file_drop_session {
            session.layout = FileDropLayoutState::Pending(request);
            if phase == FileDropSessionPhase::Hovering {
                session.hover_generation = session.hover_generation.wrapping_add(1);
                session.hovered_target = None;
                session.tab_hover = None;
            }
        }
        if phase == FileDropSessionPhase::Hovering {
            self.clear_file_drop_visuals();
        }
        file_drag_hit_test_bounds_command(FileDragHitTestBoundsRequest::FileDropLayout(request))
    }

    pub(in crate::app) fn cancel_file_drag_interaction(&mut self) {
        self.file_drag = None;
        self.file_drop_session = None;
        self.stop_file_drag_edge_scroll();
        self.clear_internal_file_drop_visuals();
    }

    pub(in crate::app) fn request_breadcrumb_drop_target_bounds_measurement(
        &mut self,
    ) -> Task<Message> {
        self.breadcrumb_drop_target_measurement_generation = self
            .breadcrumb_drop_target_measurement_generation
            .wrapping_add(1);
        breadcrumb_drop_target_bounds_command(self.breadcrumb_drop_target_measurement_generation)
    }

    pub(in crate::app) fn accept_breadcrumb_drop_target_bounds(
        &mut self,
        generation: u64,
        bounds: Vec<BreadcrumbDropTargetBounds>,
    ) -> Task<Message> {
        if generation == self.breadcrumb_drop_target_measurement_generation {
            self.breadcrumb_drop_target_bounds = bounds;
        }
        Task::none()
    }

    fn next_file_drop_layout_request(
        &mut self,
        identity: FileDropSessionIdentity,
        pane_id: crate::model::BrowserPaneId,
        tab_id: usize,
    ) -> FileDropLayoutRequest {
        self.file_drop_layout_generation = self.file_drop_layout_generation.wrapping_add(1);
        FileDropLayoutRequest {
            identity,
            pane_id,
            tab_id,
            generation: self.file_drop_layout_generation,
        }
    }

    fn internal_file_drag_snapshot(&self) -> Option<InternalFileDragSnapshot> {
        let file_drag = self
            .file_drag
            .as_ref()?
            .is_dragging()
            .then_some(self.file_drag.as_ref())
            .flatten()?;
        Some(InternalFileDragSnapshot {
            source_session_id: file_drag.native_dnd.session_id(),
            sources: file_drag.sources.clone(),
            bookmark_source: file_drag.bookmark_source.clone(),
        })
    }

    fn refresh_hovered_target_from_ready_layout(&mut self) -> Task<Message> {
        let (target, hovered_entry, hovered_sidebar) = self
            .file_drop_session
            .as_ref()
            .and_then(|session| {
                let position = session.position?;
                let FileDropLayoutState::Ready {
                    hit_test_bounds, ..
                } = &session.layout
                else {
                    return None;
                };
                let target = resolve_file_drop_target(
                    hit_test_bounds,
                    position,
                    internal_bookmark_source(&session.origin),
                );
                let hovered_entry = match &target {
                    Some(FileDropTarget::Directory(directory)) => hit_test_bounds
                        .entries
                        .iter()
                        .rev()
                        .find(|entry| {
                            entry.directory == *directory && entry.bounds.contains(position)
                        })
                        .map(|entry| entry.path.clone()),
                    _ => None,
                };
                let hovered_sidebar = match &target {
                    Some(FileDropTarget::Directory(directory)) => hit_test_bounds
                        .sidebar_directories
                        .iter()
                        .rev()
                        .find(|entry| {
                            entry.directory == *directory && entry.bounds.contains(position)
                        })
                        .map(|entry| entry.directory.clone()),
                    Some(FileDropTarget::Trash) => Some(crate::model::trash_location_path()),
                    _ => None,
                };
                Some((target, hovered_entry, hovered_sidebar))
            })
            .unwrap_or_default();
        self.hovered_entry = hovered_entry;
        self.hovered_sidebar = hovered_sidebar;
        let target_changed = self.set_file_drop_target(target.clone());
        self.update_file_drop_visuals(target.as_ref());
        match target {
            Some(FileDropTarget::Tab(target)) if target_changed => {
                self.schedule_tab_file_drop_hover(target)
            }
            _ => Task::none(),
        }
    }

    fn set_file_drop_tab_target(&mut self, target: TabFileDropTarget) -> Task<Message> {
        self.hovered_entry = None;
        self.hovered_sidebar = None;
        let target_changed = self.set_file_drop_target(Some(FileDropTarget::Tab(target.clone())));
        self.update_file_drop_visuals(Some(&FileDropTarget::Tab(target.clone())));
        if target_changed {
            self.schedule_tab_file_drop_hover(target)
        } else {
            Task::none()
        }
    }

    fn schedule_tab_file_drop_hover(&mut self, target: TabFileDropTarget) -> Task<Message> {
        if self.tab_file_drop_target_is_current(&target)
            || !self.tab_file_drop_target_is_selectable(&target)
        {
            return Task::none();
        }
        let Some(session) = &mut self.file_drop_session else {
            return Task::none();
        };
        let hover = TabDropHover {
            identity: session.identity,
            target,
            layout_generation: file_drop_layout_request(&session.layout).generation,
            hover_generation: session.hover_generation,
            started_at: std::time::Instant::now(),
        };
        session.tab_hover = Some(hover.clone());
        Task::perform(
            async move { tokio::time::sleep(TAB_FILE_DROP_HOVER_DELAY).await },
            move |_| Message::TabFileDropHoverElapsed(hover),
        )
    }

    fn freeze_file_drop_target_from_ready_layout(&mut self) {
        let frozen = self.file_drop_session.as_ref().and_then(|session| {
            let position = session.position?;
            let FileDropLayoutState::Ready {
                hit_test_bounds, ..
            } = &session.layout
            else {
                return None;
            };
            Some(
                match resolve_file_drop_target(
                    hit_test_bounds,
                    position,
                    internal_bookmark_source(&session.origin),
                ) {
                    Some(target) => FrozenFileDropTarget::Target(target),
                    None => FrozenFileDropTarget::Rejected,
                },
            )
        });
        if let (Some(session), Some(frozen)) = (&mut self.file_drop_session, frozen) {
            session.frozen_drop_target = Some(frozen);
        }
    }

    fn consume_ready_file_drop(&mut self) -> Task<Message> {
        let ready = self.file_drop_session.as_ref().is_some_and(|session| {
            session.phase == FileDropSessionPhase::Dropped
                && session.pending_payload.is_some()
                && session.frozen_drop_target.is_some()
        });
        if !ready {
            return Task::none();
        }
        let session = self
            .file_drop_session
            .take()
            .expect("ready file drop session");
        let internal = matches!(session.origin, FileDropOrigin::Internal(_));
        if internal {
            self.clear_internal_file_drop_visuals();
        } else {
            self.clear_file_drop_visuals();
        }
        let payload = session.pending_payload.expect("ready file drop payload");
        let target = match session.frozen_drop_target {
            Some(FrozenFileDropTarget::Target(target)) => Some(target),
            Some(FrozenFileDropTarget::Rejected) | None => None,
        };
        let paths = match &session.origin {
            FileDropOrigin::Internal(snapshot)
                if payload.operation == FileClipboardOperation::Move
                    && payload.paths == snapshot.sources =>
            {
                snapshot.sources.clone()
            }
            FileDropOrigin::Internal(_) => return Task::none(),
            FileDropOrigin::External => payload.paths,
        };
        self.dispatch_file_drop(session.origin, target, paths)
    }

    fn dispatch_file_drop(
        &mut self,
        origin: FileDropOrigin,
        target: Option<FileDropTarget>,
        paths: Vec<PathBuf>,
    ) -> Task<Message> {
        let Some(target) = target.and_then(|target| self.live_file_drop_target(target)) else {
            return Task::none();
        };
        match (origin, target) {
            (FileDropOrigin::Internal(snapshot), FileDropTarget::Directory(directory)) => {
                let Some(FileDropTarget::Directory(directory)) = safe_file_drop_target(
                    &snapshot.sources,
                    Some(FileDropTarget::Directory(directory)),
                ) else {
                    return Task::none();
                };
                self.move_dragged_files(snapshot.sources, directory)
            }
            (FileDropOrigin::Internal(snapshot), FileDropTarget::Trash) => {
                self.trash_explicit_paths(snapshot.sources)
            }
            (FileDropOrigin::Internal(snapshot), FileDropTarget::SidebarBookmarkSlot(slot)) => {
                snapshot.bookmark_source.map_or_else(Task::none, |source| {
                    self.insert_sidebar_bookmark_from_drag(slot, source)
                })
            }
            (FileDropOrigin::External, FileDropTarget::Directory(directory)) => {
                self.request_file_drop_prompt(directory, paths)
            }
            (FileDropOrigin::External, FileDropTarget::Trash) => self.trash_explicit_paths(paths),
            (FileDropOrigin::External, FileDropTarget::SidebarBookmarkSlot(_))
            | (_, FileDropTarget::Tab(_)) => Task::none(),
        }
    }

    fn live_file_drop_target(&self, target: FileDropTarget) -> Option<FileDropTarget> {
        match target {
            FileDropTarget::Tab(tab_target) => self
                .tab_file_drop_target_is_selectable(&tab_target)
                .then_some(match tab_target.destination {
                    TabDropDestination::Directory(directory) => {
                        FileDropTarget::Directory(directory)
                    }
                    TabDropDestination::Trash => FileDropTarget::Trash,
                }),
            target => Some(target),
        }
    }

    fn tab_file_drop_target_is_selectable(&self, target: &TabFileDropTarget) -> bool {
        let tab = if target.pane_id == self.active_pane_id() {
            self.tabs.iter().find(|tab| tab.id == target.tab_id)
        } else {
            self.pane_by_id(target.pane_id)
                .and_then(|pane| pane.tabs.iter().find(|tab| tab.id == target.tab_id))
        };
        tab.is_some_and(|tab| {
            !self.tab_is_closing_for_file_drop(target.tab_id)
                && tab.file_drop_destination() == target.destination
        })
    }

    fn tab_file_drop_target_is_current(&self, target: &TabFileDropTarget) -> bool {
        target.pane_id == self.active_pane_id() && target.tab_id == self.active_tab_id
    }

    fn update_file_drop_visuals(&mut self, target: Option<&FileDropTarget>) {
        self.sidebar_bookmark_drop_slot = match target {
            Some(FileDropTarget::SidebarBookmarkSlot(slot)) => Some(*slot),
            _ => None,
        };
    }

    fn clear_file_drop_visuals(&mut self) {
        self.hovered_entry = None;
        self.hovered_sidebar = None;
        self.sidebar_bookmark_drop_slot = None;
    }

    fn clear_internal_file_drop_visuals(&mut self) {
        self.clear_file_drop_visuals();
        self.cursor_paste_directory = None;
    }

    fn cancel_file_drop_session(&mut self, identity: FileDropSessionIdentity) -> bool {
        let matches = self
            .file_drop_session
            .as_ref()
            .is_some_and(|session| session.identity == identity);
        if matches {
            let internal = self
                .file_drop_session
                .as_ref()
                .is_some_and(|session| matches!(&session.origin, FileDropOrigin::Internal(_)));
            self.file_drop_session = None;
            if internal {
                self.clear_internal_file_drop_visuals();
            } else {
                self.clear_file_drop_visuals();
            }
        }
        matches
    }
}

pub(super) fn tab_file_drop_hover_delay_elapsed(
    started_at: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.saturating_duration_since(started_at) >= TAB_FILE_DROP_HOVER_DELAY
}

fn internal_bookmark_source(origin: &FileDropOrigin) -> Option<&std::path::Path> {
    match origin {
        FileDropOrigin::Internal(snapshot) => snapshot.bookmark_source.as_deref(),
        FileDropOrigin::External => None,
    }
}

fn file_drop_layout_request(layout: &FileDropLayoutState) -> FileDropLayoutRequest {
    match layout {
        FileDropLayoutState::Pending(request) | FileDropLayoutState::Ready { request, .. } => {
            *request
        }
    }
}
