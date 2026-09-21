use iced::{window, Task};

use super::FileBrowser;
use crate::model::{Message, WindowFrameState};

impl FileBrowser {
    pub(crate) fn main_window_id(&self) -> window::Id {
        self.main_window
    }

    pub(crate) fn window_frame_state(&self, window: window::Id) -> WindowFrameState {
        if self.maximized_windows.contains(&window) {
            WindowFrameState::Maximized
        } else {
            WindowFrameState::Restored
        }
    }

    fn application_window_is_open(&self, window: window::Id) -> bool {
        self.application_shutdown_phase.is_running()
            && (window == self.main_window
                || self.settings_window == Some(window)
                || self.properties_window == Some(window)
                || self.preview_window == Some(window))
    }

    pub(super) fn minimize_window(&self, window: window::Id) -> Task<Message> {
        window::minimize(window, true)
    }

    pub(super) fn toggle_window_maximized(&self, window: window::Id) -> Task<Message> {
        window::toggle_maximize(window).chain(self.observe_window_maximized(window))
    }

    pub(super) fn observe_window_maximized(&self, window: window::Id) -> Task<Message> {
        window::is_maximized(window).map(move |is_maximized| {
            let frame_state = if is_maximized {
                WindowFrameState::Maximized
            } else {
                WindowFrameState::Restored
            };
            Message::WindowMaximizedObserved(window, frame_state)
        })
    }

    pub(super) fn accept_window_maximized_observation(
        &mut self,
        window: window::Id,
        frame_state: WindowFrameState,
    ) -> Task<Message> {
        if !self.application_window_is_open(window) {
            return Task::none();
        }
        match frame_state {
            WindowFrameState::Restored => {
                self.maximized_windows.remove(&window);
            }
            WindowFrameState::Maximized => {
                self.maximized_windows.insert(window);
            }
        }
        Task::none()
    }

    pub(super) fn start_window_drag(&mut self, window: window::Id) -> Task<Message> {
        if self.preview_window == Some(window) {
            self.cancel_preview_window_initial_chrome_hide();
            self.preview_window_drag_active = true;
            self.preview_window_chrome.start_reveal();
        }
        window::drag(window)
    }

    pub(super) fn start_window_resize(
        &self,
        window: window::Id,
        direction: window::Direction,
    ) -> Task<Message> {
        match self.window_frame_state(window) {
            WindowFrameState::Restored => window::drag_resize(window, direction),
            WindowFrameState::Maximized => Task::none(),
        }
    }
}
