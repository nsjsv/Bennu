use std::io::Write;

use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::data_device_manager::{
    data_device::DataDeviceHandler, data_offer::DataOfferHandler, data_source::DataSourceHandler,
    WritePipe,
};
use smithay_client_toolkit::delegate_compositor;
use smithay_client_toolkit::delegate_data_device;
use smithay_client_toolkit::delegate_output;
use smithay_client_toolkit::delegate_pointer;
use smithay_client_toolkit::delegate_registry;
use smithay_client_toolkit::delegate_seat;
use smithay_client_toolkit::delegate_shm;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::seat::{
    pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT},
    Capability, SeatHandler,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use wayland_client::protocol::{
    wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_data_source::WlDataSource,
    wl_output, wl_pointer::WlPointer, wl_seat::WlSeat, wl_surface,
};
use wayland_client::{Connection, QueueHandle};

use super::payload::{drop_origin_for_mime, negotiated_drop_action, pick_mime};
use super::{
    ActiveDropTarget, WaylandFileDnd, WaylandFileDragSourceEvent, WaylandFileDropTargetEvent,
    WaylandFileDropTargetSessionId,
};
use crate::wayland_dnd::WaylandDndDropPosition;

impl SeatHandler for WaylandFileDnd {
    fn seat_state(&mut self) -> &mut smithay_client_toolkit::seat::SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let seat_index = self.ensure_seat_object(qh, &seat);
        if capability == Capability::Pointer && self.seat_objects[seat_index].pointer.is_none() {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                self.seat_objects[seat_index].pointer = Some(pointer);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer {
            return;
        }

        if let Some(seat_object) = self
            .seat_objects
            .iter_mut()
            .find(|seat_object| seat_object.seat == seat)
        {
            if let Some(pointer) = seat_object.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: WlSeat) {
        self.seat_objects.retain_mut(|seat_object| {
            if seat_object.seat != seat {
                return true;
            }
            if let Some(pointer) = seat_object.pointer.take() {
                pointer.release();
            }
            false
        });
    }
}

impl PointerHandler for WaylandFileDnd {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if event.surface != self.surface {
                continue;
            }

            if let PointerEventKind::Press { button, serial, .. } = event.kind {
                if button == BTN_LEFT {
                    self.remember_pointer_press(qh, pointer, &event.surface, serial);
                }
            }
            if let PointerEventKind::Release { button, serial, .. } = event.kind {
                if button == BTN_LEFT {
                    self.clear_pointer_press(serial);
                }
            }
        }
    }
}

impl DataDeviceHandler for WaylandFileDnd {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        data_device: &WlDataDevice,
        x: f64,
        y: f64,
        surface: &wl_surface::WlSurface,
    ) {
        let Some(data_device) = self.data_device_for(data_device) else {
            return;
        };
        let Some(offer) = data_device.data().drag_offer() else {
            return;
        };
        self.finish_active_drop_target_as_left();
        if surface != &self.surface {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
            return;
        }
        let Some(mime_type) = offer.with_mime_types(pick_mime) else {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
            return;
        };
        let target_session_id = WaylandFileDropTargetSessionId::unique();
        let origin = match drop_origin_for_mime(&mime_type, self.active_file_drag_session_id()) {
            Ok(origin) => origin,
            Err(error) => {
                self.emit_file_drop_failed(target_session_id, error.to_string());
                offer.accept_mime_type(offer.serial, None);
                offer.set_actions(DndAction::empty(), DndAction::empty());
                return;
            }
        };
        let position = WaylandDndDropPosition { x, y };
        self.active_drop_target = Some(ActiveDropTarget {
            target_session_id,
            origin,
            mime_type: mime_type.clone(),
            position,
        });
        self.emit_file_drop_target_event(WaylandFileDropTargetEvent::Entered {
            target_session_id,
            origin,
            position,
        });

        let action = negotiated_drop_action(&mime_type);
        offer.accept_mime_type(offer.serial, Some(mime_type));
        offer.set_actions(action, action);
    }

    fn leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _data_device: &WlDataDevice) {
        self.finish_active_drop_target_as_left();
    }

    fn motion(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
        x: f64,
        y: f64,
    ) {
        let Some(target) = &mut self.active_drop_target else {
            return;
        };
        let position = WaylandDndDropPosition { x, y };
        target.position = position;
        let target_session_id = target.target_session_id;
        self.emit_file_drop_target_event(WaylandFileDropTargetEvent::Moved {
            target_session_id,
            position,
        });
    }

    fn selection(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
    ) {
    }

    fn drop_performed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        data_device: &WlDataDevice,
    ) {
        let Some(data_device) = self.data_device_for(data_device) else {
            return;
        };
        let Some(offer) = data_device.data().drag_offer() else {
            return;
        };
        let Some(target) = self.active_drop_target.take() else {
            offer.finish();
            offer.destroy();
            return;
        };
        self.emit_file_drop_target_event(WaylandFileDropTargetEvent::Dropped {
            target_session_id: target.target_session_id,
            position: Some(target.position),
        });
        let action = negotiated_drop_action(&target.mime_type);
        offer.accept_mime_type(offer.serial, Some(target.mime_type.clone()));
        offer.set_actions(action, action);
        self.register_drop_read(offer, target);
    }
}

impl DataOfferHandler for WaylandFileDnd {
    fn source_actions(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        offer: &mut smithay_client_toolkit::data_device_manager::data_offer::DragOffer,
        _actions: DndAction,
    ) {
        let action = offer
            .with_mime_types(pick_mime)
            .map_or(DndAction::empty(), |mime_type| {
                negotiated_drop_action(&mime_type)
            });
        offer.set_actions(action, action);
    }

    fn selected_action(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _offer: &mut smithay_client_toolkit::data_device_manager::data_offer::DragOffer,
        _actions: DndAction,
    ) {
    }
}

impl DataSourceHandler for WaylandFileDnd {
    fn accept_mime(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
        _mime: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        source: &WlDataSource,
        mime: String,
        mut write_pipe: WritePipe,
    ) {
        let known_source = self
            .drag_sources
            .iter()
            .find(|drag_session| drag_session.source.inner() == source);
        let Some(drag_session) = known_source else {
            return;
        };

        if let Some(payload) = drag_session.payload.for_mime(&mime) {
            tracing::debug!(mime, "Wayland drag target requested payload");
            let _ = write_pipe.write_all(payload.as_bytes());
        }
    }

    fn cancelled(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, source: &WlDataSource) {
        let Some(drag_session) = self.take_file_drag_session(source) else {
            return;
        };
        tracing::debug!(
            session_id = %drag_session.session_id,
            "Wayland file drag source cancelled"
        );
        self.emit_file_drag_source_event(WaylandFileDragSourceEvent::Cancelled(
            drag_session.session_id,
        ));
    }

    fn dnd_dropped(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, source: &WlDataSource) {
        let Some(session_id) = self.file_drag_session_id_for_source(source) else {
            return;
        };
        tracing::debug!(%session_id, "Wayland file drag source dropped");
        self.emit_file_drag_source_event(WaylandFileDragSourceEvent::Dropped(session_id));
    }

    fn dnd_finished(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, source: &WlDataSource) {
        let Some(drag_session) = self.take_file_drag_session(source) else {
            return;
        };
        tracing::debug!(
            session_id = %drag_session.session_id,
            "Wayland file drag source finished"
        );
        self.emit_file_drag_source_event(WaylandFileDragSourceEvent::Finished(
            drag_session.session_id,
        ));
    }

    fn action(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
        _action: DndAction,
    ) {
    }
}

impl CompositorHandler for WaylandFileDnd {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for WaylandFileDnd {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for WaylandFileDnd {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl ProvidesRegistryState for WaylandFileDnd {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, smithay_client_toolkit::seat::SeatState];
}

delegate_compositor!(WaylandFileDnd);
delegate_output!(WaylandFileDnd);
delegate_shm!(WaylandFileDnd);
delegate_seat!(WaylandFileDnd);
delegate_pointer!(WaylandFileDnd);
delegate_data_device!(WaylandFileDnd);
delegate_registry!(WaylandFileDnd);
