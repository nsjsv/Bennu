use super::atoms::{read_property, X11Atoms, XDND_VERSION};
use super::lifecycle::ProxyLifecycle;
use super::protocol::{
    finished_data, offered_types, source_conflicts_with_target_windows, status_data,
    unpack_signed_root_position, TargetSession,
};
use super::selection::{
    PropertyPayload, SelectionProgress, SelectionTransfer, MAX_SELECTION_BYTES,
};
use super::{
    X11DndController, X11DndDropPosition, X11DndEvent, X11DndFileDrop, X11DndWindowHandle,
    X11FileDropTargetEvent, X11FileDropTargetSessionId,
};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _,
    CreateWindowAux, EventMask, GetPropertyReply, Property, SelectionNotifyEvent, Window,
    WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
const EVENT_LOOP_INTERVAL: Duration = Duration::from_millis(5);
const SELECTION_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug, Error)]
pub enum X11DndError {
    #[error("could not start X11 drag-and-drop worker: {source}")]
    ThreadSpawn {
        #[source]
        source: std::io::Error,
    },
    #[error("could not initialize X11 drag-and-drop at {stage}: {details}")]
    Setup {
        stage: &'static str,
        details: String,
    },
}

impl X11DndError {
    pub(super) fn request(stage: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Setup {
            stage,
            details: error.to_string(),
        }
    }

    pub(super) fn reply(stage: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Setup {
            stage,
            details: error.to_string(),
        }
    }
}

struct SelectionState {
    target_session_id: X11FileDropTargetSessionId,
    request: SelectionRequestIdentity,
    transfer: SelectionTransfer,
    selection_notified: bool,
    reading_incr: bool,
    started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectionRequestIdentity {
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: u32,
}

impl SelectionRequestIdentity {
    pub(super) fn new(
        requestor: Window,
        selection: Atom,
        target: Atom,
        property: Atom,
        time: u32,
    ) -> Self {
        Self {
            requestor,
            selection,
            target,
            property,
            time,
        }
    }

    pub(super) fn matches_notify(self, event: SelectionNotifyEvent) -> bool {
        event.requestor == self.requestor
            && event.selection == self.selection
            && event.target == self.target
            && event.time == self.time
            && (event.property == self.property || event.property == AtomEnum::NONE.into())
    }
}

struct X11FileDndRuntime {
    conn: RustConnection,
    root: Window,
    atoms: X11Atoms,
    lifecycle: ProxyLifecycle,
    controller: Arc<X11DndController>,
    event_sender: UnboundedSender<X11DndEvent>,
    active: Option<TargetSession>,
    selection: Option<SelectionState>,
}

pub(super) fn run_x11_file_dnd(
    window_handle: X11DndWindowHandle,
    controller: Arc<X11DndController>,
    event_sender: UnboundedSender<X11DndEvent>,
    shutdown_receiver: mpsc::Receiver<()>,
) -> Result<(), X11DndError> {
    let (conn, default_screen) =
        x11rb::connect(None).map_err(|error| X11DndError::request("connect", error))?;
    if window_handle.screen >= conn.setup().roots.len() {
        return Err(X11DndError::Setup {
            stage: "screen",
            details: format!("X11 screen {} is unavailable", window_handle.screen),
        });
    }
    if default_screen >= conn.setup().roots.len() {
        return Err(X11DndError::Setup {
            stage: "default-screen",
            details: "X11 default screen is unavailable".to_owned(),
        });
    }
    let root = conn.setup().roots[window_handle.screen].root;
    let geometry = conn
        .get_geometry(window_handle.window_xid)
        .map_err(|error| X11DndError::request("main-geometry", error))?
        .reply()
        .map_err(|error| X11DndError::reply("main-geometry", error))?;
    if geometry.root != root {
        return Err(X11DndError::Setup {
            stage: "main-root",
            details: "main window does not belong to the supplied X11 screen".to_owned(),
        });
    }

    let atoms = X11Atoms::intern(&conn)?;
    let lifecycle = ProxyLifecycle::install(&conn, root, window_handle.window_xid, &atoms)?;
    let mut runtime = X11FileDndRuntime {
        conn,
        root,
        atoms,
        lifecycle,
        controller,
        event_sender,
        active: None,
        selection: None,
    };
    let _ = runtime.event_sender.send(X11DndEvent::RuntimeReady);

    let loop_result = runtime.run_loop(shutdown_receiver);
    runtime
        .lifecycle
        .unpublish_main_proxy(&runtime.conn, &runtime.atoms);
    let send_finished = runtime
        .active
        .as_ref()
        .is_some_and(|session| session.drop_requested);
    runtime.terminate_active("X11 drag-and-drop runtime stopped", send_finished, true);
    runtime.lifecycle.teardown(&runtime.conn, &runtime.atoms);
    loop_result
}

impl X11FileDndRuntime {
    fn run_loop(&mut self, shutdown_receiver: mpsc::Receiver<()>) -> Result<(), X11DndError> {
        loop {
            match shutdown_receiver.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            while let Some(event) = self
                .conn
                .poll_for_event()
                .map_err(|error| X11DndError::request("poll-event", error))?
            {
                if matches!(
                    event,
                    Event::DestroyNotify(ref destroyed)
                        if destroyed.window == self.lifecycle.main_window
                ) {
                    let _ = self.event_sender.send(X11DndEvent::MainWindowDestroyed);
                    return Ok(());
                }
                self.handle_event(event)?;
            }
            self.expire_selection();
            thread::sleep(EVENT_LOOP_INTERVAL);
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<(), X11DndError> {
        match event {
            Event::ClientMessage(event) if event.format == 32 => {
                if event.type_ == self.atoms.xdnd_enter {
                    self.handle_enter(event);
                } else if event.type_ == self.atoms.xdnd_position {
                    self.handle_position(event);
                } else if event.type_ == self.atoms.xdnd_leave {
                    self.handle_leave(event);
                } else if event.type_ == self.atoms.xdnd_drop {
                    self.handle_drop(event);
                }
            }
            Event::SelectionNotify(event) => self.handle_selection_notify(event),
            Event::PropertyNotify(event)
                if event.state == Property::NEW_VALUE
                    && self.selection.as_ref().is_some_and(|selection| {
                        selection.request.requestor == event.window
                            && selection.request.property == event.atom
                    }) =>
            {
                self.handle_selection_chunk(event.atom)
            }
            Event::DestroyNotify(event) => {
                if event.window == self.lifecycle.proxy_window() {
                    return Err(X11DndError::Setup {
                        stage: "window-destroyed",
                        details: "X11 drag-and-drop proxy window was destroyed".to_owned(),
                    });
                }
                if self
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.request.requestor == event.window)
                {
                    self.terminate_active("X11 selection requestor was destroyed", true, true);
                } else if self
                    .active
                    .as_ref()
                    .is_some_and(|session| session.source == event.window)
                {
                    self.terminate_active("X11 drag source was destroyed", false, true);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_enter(&mut self, event: ClientMessageEvent) {
        if event.window != self.lifecycle.main_window
            && event.window != self.lifecycle.proxy_window()
        {
            return;
        }
        let data = event.data.as_data32();
        let source = data[0];
        let version = (data[1] >> 24) as u8;
        if source == 0
            || source_conflicts_with_target_windows(
                source,
                self.lifecycle.main_window,
                self.lifecycle.proxy_window(),
            )
            || version < 3
            || self.validate_source(source).is_err()
        {
            return;
        }

        let inline_types = [data[2], data[3], data[4]];
        let offered_types = if data[1] & 1 != 0 {
            self.read_offered_type_list(source)
                .ok()
                .flatten()
                .map(|types| offered_types(inline_types, Some(&types)))
                .unwrap_or_default()
        } else {
            offered_types(inline_types, None)
        };
        let accepts_uri_list = offered_types
            .into_iter()
            .any(|atom| atom == self.atoms.text_uri_list);

        if self.active.is_some() {
            let reading_selection = self.selection.is_some();
            self.terminate_active(
                "X11 drag target session was replaced",
                reading_selection,
                reading_selection,
            );
        }
        self.active = Some(TargetSession::new(
            source,
            event.window,
            version.min(XDND_VERSION as u8),
            accepts_uri_list,
        ));
    }

    fn handle_position(&mut self, event: ClientMessageEvent) {
        let data = event.data.as_data32();
        let source = data[0];
        let matches = self
            .active
            .as_ref()
            .is_some_and(|session| session.matches(source, event.window));
        if !matches || self.selection.is_some() {
            return;
        }

        let (root_x, root_y) = unpack_signed_root_position(data[2]);
        let translated = self
            .conn
            .translate_coordinates(self.root, self.lifecycle.main_window, root_x, root_y)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .filter(|reply| reply.same_screen);
        let accepted = translated.is_some()
            && self
                .active
                .as_ref()
                .is_some_and(|session| session.accepts_uri_list);
        if self.send_status(source, event.window, accepted).is_err() {
            self.terminate_active("could not reply to the X11 drag source", false, true);
            return;
        }
        let Some(translated) = translated.filter(|_| accepted) else {
            return;
        };
        let position = X11DndDropPosition {
            root_x,
            root_y,
            client_x: translated.dst_x,
            client_y: translated.dst_y,
            timestamp: data[3],
            scale_generation: self.controller.scale_generation(),
        };
        let Some(session) = &mut self.active else {
            return;
        };
        if !session.acknowledge_position(position) {
            return;
        }
        let event = if session.ui_entered {
            X11FileDropTargetEvent::Moved {
                target_session_id: session.id,
                position,
            }
        } else {
            session.ui_entered = true;
            X11FileDropTargetEvent::Entered {
                target_session_id: session.id,
                position,
            }
        };
        let _ = self.event_sender.send(X11DndEvent::FileDropTarget(event));
    }

    fn handle_leave(&mut self, event: ClientMessageEvent) {
        let source = event.data.as_data32()[0];
        let matches = self
            .active
            .as_ref()
            .is_some_and(|session| session.matches(source, event.window));
        if !matches || self.selection.is_some() {
            return;
        }
        if let Some(session) = self.active.take().filter(|session| session.ui_entered) {
            let _ =
                self.event_sender
                    .send(X11DndEvent::FileDropTarget(X11FileDropTargetEvent::Left {
                        target_session_id: session.id,
                    }));
        }
    }

    fn handle_drop(&mut self, event: ClientMessageEvent) {
        if self.selection.is_some() {
            return;
        }
        let data = event.data.as_data32();
        let source = data[0];
        let position = self.active.as_mut().and_then(|session| {
            if !session.matches(source, event.window) {
                return None;
            }
            session.mark_drop_requested();
            session.freeze_drop(source, event.window, self.controller.scale_generation())
        });
        let Some(position) = position else {
            if self
                .active
                .as_ref()
                .is_some_and(|session| session.matches(source, event.window))
            {
                self.terminate_active("X11 drop has no current acknowledged position", true, true);
            }
            return;
        };
        let Some(session) = &self.active else {
            return;
        };
        let property = match self.atoms.selection_property(&self.conn, session.id) {
            Ok(property) => property,
            Err(error) => {
                self.terminate_active(&error.to_string(), true, true);
                return;
            }
        };
        let target_session_id = session.id;
        let _ = self.event_sender.send(X11DndEvent::FileDropTarget(
            X11FileDropTargetEvent::Dropped {
                target_session_id,
                position,
            },
        ));
        let requestor = match self.create_selection_requestor() {
            Ok(requestor) => requestor,
            Err(details) => {
                self.terminate_active(&details, true, true);
                return;
            }
        };

        let conversion = (|| -> Result<(), String> {
            self.conn
                .delete_property(requestor, property)
                .map_err(|error| error.to_string())?
                .check()
                .map_err(|error| error.to_string())?;
            self.conn
                .convert_selection(
                    requestor,
                    self.atoms.xdnd_selection,
                    self.atoms.text_uri_list,
                    property,
                    data[2],
                )
                .map_err(|error| error.to_string())?
                .check()
                .map_err(|error| error.to_string())?;
            self.conn.flush().map_err(|error| error.to_string())
        })();
        if let Err(error) = conversion {
            self.destroy_selection_requestor(requestor, property);
            self.terminate_active(
                &format!("could not request X11 file drop selection: {error}"),
                true,
                true,
            );
            return;
        }
        self.selection = Some(SelectionState {
            target_session_id,
            request: SelectionRequestIdentity::new(
                requestor,
                self.atoms.xdnd_selection,
                self.atoms.text_uri_list,
                property,
                data[2],
            ),
            transfer: SelectionTransfer::new(self.atoms.text_uri_list, self.atoms.incr),
            selection_notified: false,
            reading_incr: false,
            started_at: Instant::now(),
        });
    }

    fn handle_selection_notify(&mut self, event: x11rb::protocol::xproto::SelectionNotifyEvent) {
        let Some(selection) = &self.selection else {
            return;
        };
        if selection.selection_notified {
            return;
        }
        let request = selection.request;
        if !request.matches_notify(event) {
            return;
        }
        if event.property == AtomEnum::NONE.into() {
            self.terminate_active("X11 drag source refused text/uri-list", true, true);
            return;
        }
        let payload = match self.read_selection_property(request.requestor, request.property) {
            Ok(payload) => payload,
            Err(details) => {
                self.terminate_active(&details, true, true);
                return;
            }
        };
        let progress = self
            .selection
            .as_mut()
            .expect("matching X11 selection")
            .transfer
            .accept_initial(payload);
        if let Some(selection) = &mut self.selection {
            selection.selection_notified = true;
            selection.reading_incr = matches!(progress, Ok(SelectionProgress::ReadingIncr));
        }
        self.accept_selection_progress(progress, request.requestor, request.property);
    }

    fn handle_selection_chunk(&mut self, property: Atom) {
        let Some(request) = self.selection.as_ref().and_then(|selection| {
            (selection.request.property == property && selection.reading_incr)
                .then_some(selection.request)
        }) else {
            return;
        };
        let payload = match self.read_selection_property(request.requestor, property) {
            Ok(payload) => payload,
            Err(details) => {
                self.terminate_active(&details, true, true);
                return;
            }
        };
        let progress = self
            .selection
            .as_mut()
            .expect("matching X11 selection")
            .transfer
            .accept_chunk(payload);
        self.accept_selection_progress(progress, request.requestor, property);
    }

    fn accept_selection_progress(
        &mut self,
        progress: Result<SelectionProgress, String>,
        requestor: Window,
        property: Atom,
    ) {
        let _ = self.conn.delete_property(requestor, property);
        let _ = self.conn.flush();
        match progress {
            Ok(SelectionProgress::ReadingIncr) => {}
            Ok(SelectionProgress::Complete(paths)) => self.complete_selection(paths),
            Err(details) => self.terminate_active(&details, true, true),
        }
    }

    fn complete_selection(&mut self, paths: Vec<std::path::PathBuf>) {
        let Some(selection) = self.selection.take() else {
            return;
        };
        self.destroy_selection_requestor(selection.request.requestor, selection.request.property);
        let Some(mut session) = self.active.take() else {
            return;
        };
        if session.id != selection.target_session_id || !session.finish_once() {
            return;
        }
        let _ = self
            .event_sender
            .send(X11DndEvent::FilesDropped(X11DndFileDrop {
                target_session_id: session.id,
                paths,
            }));
        let _ = self.send_finished(&session, true);
    }

    fn expire_selection(&mut self) {
        if self.selection.as_ref().is_some_and(|selection| {
            selection_timeout_elapsed(selection.started_at, Instant::now())
        }) {
            self.terminate_active("X11 file drop selection timed out", true, true);
        }
    }

    fn terminate_active(&mut self, details: &str, send_finished: bool, report_failure: bool) {
        if let Some(selection) = self.selection.take() {
            self.destroy_selection_requestor(
                selection.request.requestor,
                selection.request.property,
            );
        }
        let Some(mut session) = self.active.take() else {
            return;
        };
        if !session.finish_once() {
            return;
        }
        if report_failure {
            let _ = self.event_sender.send(X11DndEvent::FileDropFailed {
                target_session_id: session.id,
                details: details.to_owned(),
            });
        } else if session.ui_entered {
            let _ =
                self.event_sender
                    .send(X11DndEvent::FileDropTarget(X11FileDropTargetEvent::Left {
                        target_session_id: session.id,
                    }));
        }
        if send_finished {
            let _ = self.send_finished(&session, false);
        }
        let _ = self.conn.flush();
    }

    fn send_status(
        &self,
        source: Window,
        protocol_target: Window,
        accepted: bool,
    ) -> Result<(), X11DndError> {
        let event = ClientMessageEvent::new(
            32,
            source,
            self.atoms.xdnd_status,
            status_data(protocol_target, accepted, self.atoms.xdnd_action_copy),
        );
        self.conn
            .send_event(false, source, EventMask::NO_EVENT, event)
            .map_err(|error| X11DndError::request("send-status", error))?
            .check()
            .map_err(|error| X11DndError::reply("send-status", error))?;
        self.conn
            .flush()
            .map_err(|error| X11DndError::request("flush-status", error))
    }

    fn send_finished(&self, session: &TargetSession, success: bool) -> Result<(), X11DndError> {
        let event = ClientMessageEvent::new(
            32,
            session.source,
            self.atoms.xdnd_finished,
            finished_data(
                session.protocol_target,
                session.version,
                success,
                self.atoms.xdnd_action_copy,
            ),
        );
        self.conn
            .send_event(false, session.source, EventMask::NO_EVENT, event)
            .map_err(|error| X11DndError::request("send-finished", error))?
            .check()
            .map_err(|error| X11DndError::reply("send-finished", error))?;
        self.conn
            .flush()
            .map_err(|error| X11DndError::request("flush-finished", error))
    }

    fn validate_source(&self, source: Window) -> Result<(), X11DndError> {
        self.conn
            .get_geometry(source)
            .map_err(|error| X11DndError::request("source-geometry", error))?
            .reply()
            .map_err(|error| X11DndError::reply("source-geometry", error))?;
        self.conn
            .change_window_attributes(
                source,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|error| X11DndError::request("watch-source", error))?
            .check()
            .map_err(|error| X11DndError::reply("watch-source", error))
    }

    fn create_selection_requestor(&self) -> Result<Window, String> {
        let requestor = self
            .conn
            .generate_id()
            .map_err(|error| format!("could not allocate X11 selection requestor: {error}"))?;
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                requestor,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|error| format!("could not create X11 selection requestor: {error}"))?
            .check()
            .map_err(|error| format!("could not create X11 selection requestor: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("could not publish X11 selection requestor: {error}"))?;
        Ok(requestor)
    }

    fn destroy_selection_requestor(&self, requestor: Window, property: Atom) {
        let _ = self.conn.delete_property(requestor, property);
        let _ = self.conn.destroy_window(requestor);
        let _ = self.conn.flush();
    }

    fn read_offered_type_list(&self, source: Window) -> Result<Option<Vec<Atom>>, X11DndError> {
        let Some(property) = read_property(&self.conn, source, self.atoms.xdnd_type_list)? else {
            return Ok(None);
        };
        if property.type_ != AtomEnum::ATOM.into()
            || property.format != 32
            || property.value.len() % 4 != 0
        {
            return Ok(None);
        }
        Ok(Some(
            property
                .value
                .chunks_exact(4)
                .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four bytes")))
                .collect(),
        ))
    }

    fn read_selection_property(
        &self,
        requestor: Window,
        property: Atom,
    ) -> Result<PropertyPayload, String> {
        let long_length = MAX_SELECTION_BYTES.div_ceil(4) as u32;
        let reply: GetPropertyReply = self
            .conn
            .get_property(false, requestor, property, AtomEnum::ANY, 0, long_length)
            .map_err(|error| format!("could not read X11 file drop property: {error}"))?
            .reply()
            .map_err(|error| format!("could not read X11 file drop property: {error}"))?;
        Ok(PropertyPayload {
            type_: reply.type_,
            format: reply.format,
            bytes_after: reply.bytes_after,
            value: reply.value,
        })
    }
}

pub(super) fn selection_timeout_elapsed(started_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(started_at) >= SELECTION_TIMEOUT
}
