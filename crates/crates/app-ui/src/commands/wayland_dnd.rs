use desktop_linux::WaylandDndWindowHandle;
use iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use iced::{window, Task};

use crate::model::Message;

pub(crate) fn wayland_dnd_window_handle_command(window_id: window::Id) -> Task<Message> {
    window::run(window_id, wayland_dnd_window_handle).map(Message::WaylandDndWindowHandleLoaded)
}

fn wayland_dnd_window_handle(
    iced_window: &dyn window::Window,
) -> Result<Option<WaylandDndWindowHandle>, String> {
    let display_handle = iced_window
        .display_handle()
        .map_err(|error| format!("could not read display handle: {error}"))?
        .as_raw();
    let window_handle = iced_window
        .window_handle()
        .map_err(|error| format!("could not read window handle: {error}"))?
        .as_raw();

    match (display_handle, window_handle) {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
            Ok(Some(WaylandDndWindowHandle::new(
                display.display.as_ptr() as usize,
                window.surface.as_ptr() as usize,
            )))
        }
        _ => Ok(None),
    }
}
