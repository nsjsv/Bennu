use desktop_linux::{X11DndEvent, X11DndWindowHandle};
use iced::window;

#[derive(Debug, Clone)]
pub(crate) enum X11DndMessage {
    WindowHandleLoaded {
        handle: Result<Option<X11DndWindowHandle>, String>,
        scale_factor: f32,
    },
    RuntimeEvent {
        runtime_id: u64,
        event: X11DndEvent,
    },
    ScaleFactorChanged {
        window: window::Id,
        scale_factor: f32,
    },
}
