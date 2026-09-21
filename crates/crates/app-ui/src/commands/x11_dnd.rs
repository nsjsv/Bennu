use desktop_linux::X11DndWindowHandle;
use iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use iced::{window, Task};

use crate::model::{Message, X11DndMessage};

pub(crate) fn x11_dnd_window_handle_command(window_id: window::Id) -> Task<Message> {
    window::scale_factor(window_id).then(move |scale_factor| {
        window::run(window_id, x11_dnd_window_handle).map(move |handle| {
            Message::X11Dnd(X11DndMessage::WindowHandleLoaded {
                handle,
                scale_factor,
            })
        })
    })
}

fn x11_dnd_window_handle(
    iced_window: &dyn window::Window,
) -> Result<Option<X11DndWindowHandle>, String> {
    let display_handle = iced_window
        .display_handle()
        .map_err(|error| format!("could not read display handle: {error}"))?
        .as_raw();
    let window_handle = iced_window
        .window_handle()
        .map_err(|error| format!("could not read window handle: {error}"))?
        .as_raw();
    x11_dnd_handle_from_raw(display_handle, window_handle)
}

fn x11_dnd_handle_from_raw(
    display_handle: RawDisplayHandle,
    window_handle: RawWindowHandle,
) -> Result<Option<X11DndWindowHandle>, String> {
    match (display_handle, window_handle) {
        (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
            if display.display.is_none() {
                return Err("Xlib display handle has no display pointer".to_owned());
            }
            let screen = usize::try_from(display.screen)
                .map_err(|_| format!("Xlib screen {} is invalid", display.screen))?;
            let window_xid = u32::try_from(window.window)
                .map_err(|_| format!("Xlib window XID {} is out of range", window.window))?;
            if window_xid == 0 {
                return Err("Xlib window XID is zero".to_owned());
            }
            Ok(Some(X11DndWindowHandle::new(window_xid, screen)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use iced::window::raw_window_handle::{
        WaylandWindowHandle, XlibDisplayHandle, XlibWindowHandle,
    };

    use super::*;

    #[test]
    fn xlib_handles_are_classified_without_retaining_the_display_pointer() {
        let display = RawDisplayHandle::Xlib(XlibDisplayHandle::new(
            Some(NonNull::<c_void>::dangling()),
            2,
        ));
        let window = RawWindowHandle::Xlib(XlibWindowHandle::new(55));
        assert_eq!(
            x11_dnd_handle_from_raw(display, window).expect("Xlib handle"),
            Some(X11DndWindowHandle::new(55, 2))
        );
    }

    #[test]
    fn unsupported_or_invalid_handle_pairs_fail_closed() {
        let display = RawDisplayHandle::Xlib(XlibDisplayHandle::new(
            Some(NonNull::<c_void>::dangling()),
            0,
        ));
        let wayland =
            RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::<c_void>::dangling()));
        assert_eq!(
            x11_dnd_handle_from_raw(display, wayland).expect("unsupported pair"),
            None
        );
        assert!(
            x11_dnd_handle_from_raw(display, RawWindowHandle::Xlib(XlibWindowHandle::new(0)))
                .is_err()
        );
    }
}
