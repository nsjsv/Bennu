mod drag_icon;
mod handlers;
mod payload;
mod source_session;
mod target_session;
#[cfg(test)]
mod tests;

use payload::{parse_drop_selection, DragPayload};
use source_session::WaylandDndCommand;
pub use source_session::{
    WaylandDndCommandError, WaylandDndController, WaylandFileDragIcon, WaylandFileDragIconError,
    WaylandFileDragSessionId, WaylandFileDragSourceEvent,
};
pub use target_session::WaylandFileDropTargetSessionId;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::data_device_manager::{
    data_device::DataDevice, data_offer::DragOffer, data_source::DragSource, DataDeviceManagerState,
};
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, PostAction};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shm::{slot::SlotPool, Shm};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{
    wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_pointer::WlPointer,
    wl_seat::WlSeat, wl_surface,
};
use wayland_client::{Connection, Proxy, QueueHandle};

use self::drag_icon::{WaylandDragIconSurface, INITIAL_DRAG_ICON_POOL_BYTES};
use crate::file_clipboard::{
    FileClipboardPayloadError, FileClipboardSelection, GNOME_COPIED_FILES_MIME, URI_LIST_MIME,
};

const FILE_DRAG_SOURCE_MIME_TYPES: &[&str] = &[
    INTERNAL_FILE_DRAG_MIME,
    URI_LIST_MIME,
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
];
const FILE_DROP_TARGET_MIME_TYPES: &[&str] = &[
    INTERNAL_FILE_DRAG_MIME,
    GNOME_COPIED_FILES_MIME,
    URI_LIST_MIME,
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
];
const FILE_DRAG_SOURCE_ACTIONS: DndAction = DndAction::Copy.union(DndAction::Move);
const INTERNAL_FILE_DRAG_MIME: &str = "application/x-bennu-internal-dnd";
const DRAG_REQUEST_TTL: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandDndWindowHandle {
    pub display_ptr: usize,
    pub surface_ptr: usize,
}

impl WaylandDndWindowHandle {
    pub fn new(display_ptr: usize, surface_ptr: usize) -> Self {
        Self {
            display_ptr,
            surface_ptr,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WaylandDndEvent {
    FilesDropped(WaylandDndFileDrop),
    FileDropFailed {
        target_session_id: WaylandFileDropTargetSessionId,
        details: String,
    },
    FileDragSource(WaylandFileDragSourceEvent),
    FileDropTarget(WaylandFileDropTargetEvent),
    RuntimeFailed(String),
}

#[derive(Debug, Clone)]
pub struct WaylandDndFileDrop {
    pub target_session_id: WaylandFileDropTargetSessionId,
    pub selection: FileClipboardSelection,
    pub origin: WaylandDndDropOrigin,
    pub position: Option<WaylandDndDropPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaylandDndDropOrigin {
    External,
    Internal(WaylandFileDragSessionId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaylandDndDropPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaylandFileDropTargetEvent {
    Entered {
        target_session_id: WaylandFileDropTargetSessionId,
        origin: WaylandDndDropOrigin,
        position: WaylandDndDropPosition,
    },
    Moved {
        target_session_id: WaylandFileDropTargetSessionId,
        position: WaylandDndDropPosition,
    },
    Left {
        target_session_id: WaylandFileDropTargetSessionId,
    },
    Dropped {
        target_session_id: WaylandFileDropTargetSessionId,
        position: Option<WaylandDndDropPosition>,
    },
}

impl WaylandFileDropTargetEvent {
    pub fn target_session_id(self) -> WaylandFileDropTargetSessionId {
        match self {
            Self::Entered {
                target_session_id, ..
            }
            | Self::Moved {
                target_session_id, ..
            }
            | Self::Left { target_session_id }
            | Self::Dropped {
                target_session_id, ..
            } => target_session_id,
        }
    }
}

#[derive(Debug, Error)]
pub enum WaylandDndError {
    #[error("could not start Wayland drag-and-drop worker: {source}")]
    ThreadSpawn {
        #[source]
        source: std::io::Error,
    },
    #[error("could not initialize Wayland drag-and-drop at {stage}: {details}")]
    Setup {
        stage: &'static str,
        details: String,
    },
    #[error("Wayland drag payload for {mime} is not UTF-8: {source}")]
    PayloadUtf8 {
        mime: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("Wayland drag payload for {mime} is invalid: {source}")]
    Payload {
        mime: String,
        #[source]
        source: FileClipboardPayloadError,
    },
    #[error("Wayland internal drop has no matching source session")]
    InternalDropSessionUnavailable,
}

pub fn spawn_wayland_file_dnd(
    window_handle: WaylandDndWindowHandle,
    controller: Arc<WaylandDndController>,
    event_sender: UnboundedSender<WaylandDndEvent>,
    shutdown_receiver: mpsc::Receiver<()>,
) -> Result<thread::JoinHandle<()>, WaylandDndError> {
    thread::Builder::new()
        .name("wayland-file-dnd".to_owned())
        .spawn(move || {
            if let Err(error) = run_wayland_file_dnd(
                window_handle,
                controller,
                event_sender.clone(),
                shutdown_receiver,
            ) {
                let _ = event_sender.send(WaylandDndEvent::RuntimeFailed(error.to_string()));
            }
        })
        .map_err(|source| WaylandDndError::ThreadSpawn { source })
}

struct WaylandFileDnd {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm_state: Shm,
    data_device_state: DataDeviceManagerState,
    drag_icon_pool: SlotPool,
    surface: wl_surface::WlSurface,
    seat_objects: Vec<SeatObject>,
    drag_sources: Vec<DragSession>,
    drop_reads: Vec<DropRead>,
    active_drop_target: Option<ActiveDropTarget>,
    loop_handle: LoopHandle<'static, WaylandFileDnd>,
    pending_file_drag: Option<PendingFileDrag>,
    active_left_press: Option<PointerPress>,
    event_sender: UnboundedSender<WaylandDndEvent>,
}

struct SeatObject {
    seat: WlSeat,
    pointer: Option<WlPointer>,
    data_device: DataDevice,
}

struct DragSession {
    session_id: WaylandFileDragSessionId,
    source: DragSource,
    payload: DragPayload,
    _icon_surface: WaylandDragIconSurface,
}

#[derive(Clone)]
struct ActiveDropTarget {
    target_session_id: WaylandFileDropTargetSessionId,
    origin: WaylandDndDropOrigin,
    mime_type: String,
    position: WaylandDndDropPosition,
}

struct PendingFileDrag {
    session_id: WaylandFileDragSessionId,
    paths: Vec<PathBuf>,
    icon: WaylandFileDragIcon,
    requested_at: Instant,
}

#[derive(Clone)]
struct PointerPress {
    pointer: WlPointer,
    surface: wl_surface::WlSurface,
    serial: u32,
}

struct DropRead {
    offer: DragOffer,
    target_session_id: WaylandFileDropTargetSessionId,
    mime_type: String,
    origin: WaylandDndDropOrigin,
    data: Vec<u8>,
    position: Option<WaylandDndDropPosition>,
}

fn run_wayland_file_dnd(
    window_handle: WaylandDndWindowHandle,
    controller: Arc<WaylandDndController>,
    event_sender: UnboundedSender<WaylandDndEvent>,
    shutdown_receiver: mpsc::Receiver<()>,
) -> Result<(), WaylandDndError> {
    let mut command_receiver = controller
        .take_command_receiver()
        .ok_or_else(|| setup_error("command-receiver", "command receiver was already taken"))?;
    let (conn, surface) = unsafe { wayland_connection_from_raw_handle(window_handle) }?;
    let (globals, event_queue) =
        registry_queue_init(&conn).map_err(|error| setup_error("registry", error))?;
    let qh = event_queue.handle();
    let mut event_loop = EventLoop::try_new().map_err(|error| setup_error("event-loop", error))?;
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|error| setup_error("wayland-source", error))?;

    let data_device_state = DataDeviceManagerState::bind(&globals, &qh)
        .map_err(|error| setup_error("data-device-manager", format!("{error:?}")))?;
    let compositor_state = CompositorState::bind(&globals, &qh)
        .map_err(|error| setup_error("compositor", format!("{error:?}")))?;
    let output_state = OutputState::new(&globals, &qh);
    let shm_state = Shm::bind(&globals, &qh)
        .map_err(|error| setup_error("shared-memory", format!("{error:?}")))?;
    let drag_icon_pool = SlotPool::new(INITIAL_DRAG_ICON_POOL_BYTES, &shm_state)
        .map_err(|error| setup_error("drag-icon-pool", error))?;
    let mut dnd = WaylandFileDnd {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state,
        compositor_state,
        shm_state,
        data_device_state,
        drag_icon_pool,
        surface,
        seat_objects: Vec::new(),
        drag_sources: Vec::new(),
        drop_reads: Vec::new(),
        active_drop_target: None,
        loop_handle: event_loop.handle(),
        pending_file_drag: None,
        active_left_press: None,
        event_sender,
    };

    tracing::debug!(
        controller_id = controller.id(),
        "Wayland file drag-and-drop worker started"
    );

    loop {
        match shutdown_receiver.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        dnd.receive_commands(&mut command_receiver, &qh);
        event_loop
            .dispatch(Duration::from_millis(16), &mut dnd)
            .map_err(|error| setup_error("dispatch", error))?;
        dnd.receive_commands(&mut command_receiver, &qh);
    }

    Ok(())
}

unsafe fn wayland_connection_from_raw_handle(
    window_handle: WaylandDndWindowHandle,
) -> Result<(Connection, wl_surface::WlSurface), WaylandDndError> {
    // raw-window-handle 的指针生命周期由 Iced/winit 拥有，这里只在同一 wl_display 上创建代理。
    let backend = unsafe { Backend::from_foreign_display(window_handle.display_ptr as *mut _) };
    let conn = Connection::from_backend(backend);
    let surface_id = unsafe {
        ObjectId::from_ptr(
            wl_surface::WlSurface::interface(),
            window_handle.surface_ptr as *mut _,
        )
    }
    .map_err(|error| setup_error("surface-id", error))?;
    let surface = wl_surface::WlSurface::from_id(&conn, surface_id)
        .map_err(|error| setup_error("surface-proxy", error))?;
    Ok((conn, surface))
}

fn setup_error(stage: &'static str, error: impl std::fmt::Display) -> WaylandDndError {
    WaylandDndError::Setup {
        stage,
        details: error.to_string(),
    }
}

impl WaylandFileDnd {
    fn receive_commands(
        &mut self,
        command_receiver: &mut UnboundedReceiver<WaylandDndCommand>,
        qh: &QueueHandle<Self>,
    ) {
        while let Ok(command) = command_receiver.try_recv() {
            match command {
                WaylandDndCommand::StartFileDrag {
                    session_id,
                    paths,
                    icon,
                } => {
                    tracing::debug!(
                        %session_id,
                        path_count = paths.len(),
                        "Wayland file drag request received"
                    );
                    let request = PendingFileDrag {
                        session_id,
                        paths,
                        icon,
                        requested_at: Instant::now(),
                    };
                    if let Some(replaced) = self.pending_file_drag.replace(request) {
                        self.reject_file_drag_request(
                            replaced,
                            "Wayland file drag request was replaced before it started",
                        );
                    }
                    self.start_pending_file_drag(qh);
                }
            }
        }
        self.expire_pending_file_drag();
    }

    fn ensure_seat_object(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat) -> usize {
        if let Some(index) = self
            .seat_objects
            .iter()
            .position(|seat_object| seat_object.seat == *seat)
        {
            return index;
        }

        let data_device = self.data_device_state.get_data_device(qh, seat);
        self.seat_objects.push(SeatObject {
            seat: seat.clone(),
            pointer: None,
            data_device,
        });
        self.seat_objects.len() - 1
    }

    fn data_device_for(&self, data_device: &WlDataDevice) -> Option<&DataDevice> {
        self.seat_objects
            .iter()
            .find(|seat_object| seat_object.data_device.inner() == data_device)
            .map(|seat_object| &seat_object.data_device)
    }

    fn remember_pointer_press(
        &mut self,
        qh: &QueueHandle<Self>,
        pointer: &WlPointer,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) {
        self.active_left_press = Some(PointerPress {
            pointer: pointer.clone(),
            surface: surface.clone(),
            serial,
        });
        tracing::debug!(serial, "Wayland pointer press serial captured");
        self.start_pending_file_drag(qh);
    }

    fn clear_pointer_press(&mut self, serial: u32) {
        tracing::debug!(serial, "Wayland pointer release serial observed");
        self.active_left_press = None;
    }

    fn start_pending_file_drag(&mut self, qh: &QueueHandle<Self>) {
        let Some(press) = self.active_left_press.clone() else {
            return;
        };
        if press.surface != self.surface {
            return;
        }
        if self
            .pending_file_drag
            .as_ref()
            .is_some_and(|request| request.requested_at.elapsed() > DRAG_REQUEST_TTL)
        {
            let expired = self.pending_file_drag.take().expect("expired drag request");
            self.reject_file_drag_request(
                expired,
                "Wayland file drag request expired before a usable pointer press was available",
            );
            return;
        }
        let Some(request) = self.pending_file_drag.take() else {
            return;
        };
        if let Some(active_session_id) = self.active_file_drag_session_id() {
            self.reject_file_drag_request(
                request,
                format!("Wayland file drag source {active_session_id} is still active"),
            );
            return;
        }
        let Some(seat_index) = self
            .seat_objects
            .iter()
            .position(|seat_object| seat_object.pointer.as_ref() == Some(&press.pointer))
        else {
            self.reject_file_drag_request(
                request,
                "Wayland file drag could not identify the seat that owns the pointer",
            );
            return;
        };

        let icon_surface = match WaylandDragIconSurface::create(
            &self.compositor_state,
            &mut self.drag_icon_pool,
            qh,
            &request.icon,
        ) {
            Ok(icon_surface) => icon_surface,
            Err(error) => {
                self.reject_file_drag_request(request, error.to_string());
                return;
            }
        };
        let payload = DragPayload::new(&request.paths);
        let source = self.data_device_state.create_drag_and_drop_source(
            qh,
            FILE_DRAG_SOURCE_MIME_TYPES.to_vec(),
            FILE_DRAG_SOURCE_ACTIONS,
        );
        source.start_drag(
            &self.seat_objects[seat_index].data_device,
            &press.surface,
            Some(icon_surface.wl_surface()),
            press.serial,
        );
        tracing::debug!(
            %request.session_id,
            serial = press.serial,
            path_count = request.paths.len(),
            "Wayland file drag started"
        );
        let session_id = request.session_id;
        self.drag_sources.push(DragSession {
            session_id,
            source,
            payload,
            _icon_surface: icon_surface,
        });
        self.emit_file_drag_source_event(WaylandFileDragSourceEvent::Started(session_id));
    }

    fn expire_pending_file_drag(&mut self) {
        let request_is_fresh = self
            .pending_file_drag
            .as_ref()
            .is_none_or(|request| request.requested_at.elapsed() <= DRAG_REQUEST_TTL);
        if request_is_fresh {
            return;
        }
        let expired = self.pending_file_drag.take().expect("expired drag request");
        self.reject_file_drag_request(
            expired,
            "Wayland file drag request expired before a usable pointer press was available",
        );
    }

    fn reject_file_drag_request(&self, request: PendingFileDrag, details: impl Into<String>) {
        let details = details.into();
        tracing::warn!(
            %request.session_id,
            path_count = request.paths.len(),
            %details,
            "Wayland file drag request rejected"
        );
        self.emit_file_drag_source_event(WaylandFileDragSourceEvent::Rejected {
            session_id: request.session_id,
            details,
        });
    }

    fn emit_file_drag_source_event(&self, event: WaylandFileDragSourceEvent) {
        let _ = self
            .event_sender
            .send(WaylandDndEvent::FileDragSource(event));
    }

    fn active_file_drag_session_id(&self) -> Option<WaylandFileDragSessionId> {
        self.drag_sources
            .first()
            .map(|drag_session| drag_session.session_id)
    }

    fn emit_file_drop_target_event(&self, event: WaylandFileDropTargetEvent) {
        let _ = self
            .event_sender
            .send(WaylandDndEvent::FileDropTarget(event));
    }

    fn finish_active_drop_target_as_left(&mut self) {
        let Some(target) = self.active_drop_target.take() else {
            return;
        };
        self.emit_file_drop_target_event(WaylandFileDropTargetEvent::Left {
            target_session_id: target.target_session_id,
        });
    }

    fn emit_file_drop_failed(
        &self,
        target_session_id: WaylandFileDropTargetSessionId,
        details: impl Into<String>,
    ) {
        let _ = self.event_sender.send(WaylandDndEvent::FileDropFailed {
            target_session_id,
            details: details.into(),
        });
    }

    fn register_drop_read(&mut self, offer: DragOffer, target: ActiveDropTarget) {
        let ActiveDropTarget {
            target_session_id,
            origin,
            mime_type,
            position,
        } = target;
        let read_pipe = match offer.receive(mime_type.clone()) {
            Ok(read_pipe) => read_pipe,
            Err(error) => {
                self.emit_file_drop_failed(
                    target_session_id,
                    format!("could not receive Wayland drag payload for {mime_type}: {error:?}"),
                );
                offer.finish();
                offer.destroy();
                return;
            }
        };

        self.drop_reads.push(DropRead {
            offer: offer.clone(),
            target_session_id,
            mime_type,
            origin,
            data: Vec::new(),
            position: Some(position),
        });

        let offer_key = offer.clone();
        if let Err(error) = self
            .loop_handle
            .insert_source(read_pipe, move |_, file, state| {
                state.read_drop_payload(&offer_key, file)
            })
        {
            self.drop_reads.retain(|read| read.offer != offer);
            self.emit_file_drop_failed(
                target_session_id,
                format!("could not register Wayland drag payload reader: {error}"),
            );
            offer.finish();
            offer.destroy();
        }
    }

    fn read_drop_payload(
        &mut self,
        offer_key: &DragOffer,
        file: &mut smithay_client_toolkit::reexports::calloop::generic::NoIoDrop<fs::File>,
    ) -> PostAction {
        let Some(position) = self
            .drop_reads
            .iter()
            .position(|read| &read.offer == offer_key)
        else {
            return PostAction::Continue;
        };
        let mut drop_read = self.drop_reads.remove(position);
        let mut consumed = 0;

        let read_outcome = {
            let file = unsafe { file.get_mut() };
            let mut reader = BufReader::new(file);
            match reader.fill_buf() {
                Ok([]) => DropReadOutcome::Finished,
                Ok(buffer) => {
                    drop_read.data.extend_from_slice(buffer);
                    consumed = buffer.len();
                    DropReadOutcome::Continue
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    DropReadOutcome::Continue
                }
                Err(error) => DropReadOutcome::Failed(error),
            }
            .also_consume(&mut reader, consumed)
        };

        match read_outcome {
            DropReadOutcome::Continue => {
                self.drop_reads.push(drop_read);
                PostAction::Continue
            }
            DropReadOutcome::Finished => {
                self.finish_drop_read(drop_read);
                PostAction::Remove
            }
            DropReadOutcome::Failed(error) => {
                self.emit_file_drop_failed(
                    drop_read.target_session_id,
                    format!("could not read Wayland drag payload: {error}"),
                );
                drop_read.offer.finish();
                drop_read.offer.destroy();
                PostAction::Remove
            }
        }
    }

    fn finish_drop_read(&mut self, drop_read: DropRead) {
        match parse_drop_selection(&drop_read.mime_type, &drop_read.data) {
            Ok(parsed_drop) => {
                let _ = self
                    .event_sender
                    .send(WaylandDndEvent::FilesDropped(WaylandDndFileDrop {
                        target_session_id: drop_read.target_session_id,
                        selection: parsed_drop.selection,
                        origin: drop_read.origin,
                        position: drop_read.position,
                    }));
            }
            Err(error) => {
                self.emit_file_drop_failed(drop_read.target_session_id, error.to_string());
            }
        }
        drop_read.offer.finish();
        drop_read.offer.destroy();
    }

    fn file_drag_session_id_for_source(
        &self,
        source: &wayland_client::protocol::wl_data_source::WlDataSource,
    ) -> Option<WaylandFileDragSessionId> {
        self.drag_sources
            .iter()
            .find(|drag_session| drag_session.source.inner() == source)
            .map(|drag_session| drag_session.session_id)
    }

    fn take_file_drag_session(
        &mut self,
        source: &wayland_client::protocol::wl_data_source::WlDataSource,
    ) -> Option<DragSession> {
        let position = self
            .drag_sources
            .iter()
            .position(|drag_session| drag_session.source.inner() == source)?;
        let drag_session = self.drag_sources.remove(position);
        Some(drag_session)
    }
}

enum DropReadOutcome {
    Continue,
    Finished,
    Failed(std::io::Error),
}

impl DropReadOutcome {
    fn also_consume<R: BufRead>(self, reader: &mut R, consumed: usize) -> Self {
        if consumed > 0 {
            reader.consume(consumed);
        }
        self
    }
}
