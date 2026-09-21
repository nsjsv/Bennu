use std::sync::Arc;

use desktop_linux::{X11DndController, X11DndEvent, X11DndWindowHandle};
use iced::{window, Task};

use super::FileBrowser;
use crate::model::{Message, X11DndMessage};

#[derive(Clone, Debug)]
pub(super) struct X11DndRuntime {
    pub(super) window_handle: X11DndWindowHandle,
    pub(super) controller: Arc<X11DndController>,
    pub(super) scale_factor: f32,
    pub(super) scale_generation: u64,
    pub(super) ready: bool,
}

impl X11DndRuntime {
    fn new(window_handle: X11DndWindowHandle, scale_factor: f32) -> Self {
        let scale_generation = 1;
        Self {
            window_handle,
            controller: X11DndController::new(scale_generation),
            scale_factor: valid_scale_factor(scale_factor),
            scale_generation,
            ready: false,
        }
    }
}

impl FileBrowser {
    pub(in crate::app) fn accept_x11_dnd_message(
        &mut self,
        message: X11DndMessage,
    ) -> Task<Message> {
        match message {
            X11DndMessage::WindowHandleLoaded {
                handle,
                scale_factor,
            } => self.accept_x11_dnd_handle(handle, scale_factor),
            X11DndMessage::RuntimeEvent { runtime_id, event } => {
                self.accept_x11_dnd_event(runtime_id, event)
            }
            X11DndMessage::ScaleFactorChanged {
                window,
                scale_factor,
            } => self.accept_x11_scale_factor(window, scale_factor),
        }
    }

    pub(in crate::app) fn accept_x11_dnd_handle(
        &mut self,
        handle: Result<Option<X11DndWindowHandle>, String>,
        scale_factor: f32,
    ) -> Task<Message> {
        self.cancel_x11_file_drop_session();
        self.x11_dnd = None;
        match handle {
            Ok(Some(handle)) => {
                tracing::debug!(
                    window_xid = handle.window_xid,
                    screen = handle.screen,
                    "X11 drag-and-drop handle loaded"
                );
                self.x11_dnd = Some(X11DndRuntime::new(handle, scale_factor));
                let main_window = self.main_window;
                return window::scale_factor(main_window).map(move |current_scale_factor| {
                    Message::X11Dnd(X11DndMessage::ScaleFactorChanged {
                        window: main_window,
                        scale_factor: current_scale_factor,
                    })
                });
            }
            Ok(None) => {
                tracing::debug!("X11 drag-and-drop unavailable for this window backend");
                self.x11_dnd = None;
            }
            Err(error) => self.show_global_error(error),
        }
        Task::none()
    }

    pub(in crate::app) fn accept_x11_dnd_event(
        &mut self,
        runtime_id: u64,
        event: X11DndEvent,
    ) -> Task<Message> {
        let runtime_matches = self
            .x11_dnd
            .as_ref()
            .is_some_and(|runtime| runtime.controller.id() == runtime_id);
        if !runtime_matches {
            return Task::none();
        }
        match event {
            X11DndEvent::RuntimeReady => {
                if let Some(runtime) = &mut self.x11_dnd {
                    runtime.ready = true;
                }
                Task::none()
            }
            X11DndEvent::FileDropTarget(event) => {
                let runtime = self.x11_dnd.as_ref().expect("matching X11 runtime");
                self.accept_x11_target_event(event, runtime.scale_factor, runtime.scale_generation)
            }
            X11DndEvent::FilesDropped(drop) => self.accept_x11_file_drop(drop),
            X11DndEvent::FileDropFailed {
                target_session_id,
                details,
            } => self.accept_x11_drop_failure(target_session_id, details),
            X11DndEvent::MainWindowDestroyed => {
                self.x11_dnd = None;
                self.cancel_x11_file_drop_session();
                self.accept_application_window_closed(self.main_window)
            }
            X11DndEvent::RuntimeFailed(error) => {
                self.x11_dnd = None;
                self.cancel_x11_file_drop_session();
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    pub(in crate::app) fn accept_x11_scale_factor(
        &mut self,
        window: window::Id,
        scale_factor: f32,
    ) -> Task<Message> {
        if window != self.main_window {
            return Task::none();
        }
        let Some(runtime) = &mut self.x11_dnd else {
            return Task::none();
        };
        let scale_factor = valid_scale_factor(scale_factor);
        if runtime.scale_factor == scale_factor {
            return Task::none();
        }
        runtime.scale_generation = runtime.scale_generation.wrapping_add(1);
        runtime.scale_factor = scale_factor;
        runtime
            .controller
            .set_scale_generation(runtime.scale_generation);
        self.invalidate_x11_file_drop_for_scale_change()
    }
}

fn valid_scale_factor(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn main_window_destroy_event_enters_owned_shutdown() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(7, 0))), 1.0));
        let runtime_id = browser.x11_dnd.as_ref().unwrap().controller.id();

        drop(browser.accept_x11_dnd_event(runtime_id, X11DndEvent::MainWindowDestroyed));

        assert!(browser.x11_dnd.is_none());
        assert!(!browser.application_shutdown_phase.is_running());
    }
}
