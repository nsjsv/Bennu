use iced::keyboard;
use iced::{event, mouse, window, Event, Theme};

use crate::matugen_theme::{fallback_theme, AppearanceMode};
use crate::model::{Message, X11DndMessage};

pub(super) fn global_event_message(
    event: Event,
    status: event::Status,
    window: window::Id,
) -> Option<Message> {
    if let Event::Window(window_event) = &event {
        return match window_event {
            iced::window::Event::CloseRequested => {
                Some(Message::AuxiliaryWindowCloseRequested(window))
            }
            iced::window::Event::Closed => Some(Message::ApplicationWindowClosed(window)),
            iced::window::Event::Resized(size) => Some(Message::AuxiliaryWindowResized(
                window,
                size.width,
                size.height,
            )),
            iced::window::Event::Rescaled(scale_factor) => {
                Some(Message::X11Dnd(X11DndMessage::ScaleFactorChanged {
                    window,
                    scale_factor: *scale_factor,
                }))
            }
            iced::window::Event::Focused => Some(Message::WindowFocused(window)),
            iced::window::Event::Unfocused => Some(Message::WindowUnfocused(window)),
            _ => None,
        };
    }

    if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = &event {
        return Some(Message::KeyboardModifiersChanged(*modifiers));
    }

    if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = &event {
        return Some(Message::KeyboardKeyPressed {
            key: key.clone(),
            modifiers: *modifiers,
            status,
        });
    }

    if let Event::Mouse(mouse::Event::CursorMoved { position }) = &event {
        return Some(Message::CursorMoved {
            window,
            position: *position,
        });
    }

    if matches!(&event, Event::Mouse(mouse::Event::CursorLeft)) {
        return Some(Message::CursorLeft { window });
    }

    if let Some(message) = pointer_pressed_message(&event, status, window) {
        return Some(message);
    }

    if matches!(
        &event,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
    ) {
        return Some(Message::WindowPointerReleased { window, status });
    }

    if matches!(status, event::Status::Captured) {
        return None;
    }

    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)) => Some(Message::Back),
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Forward)) => Some(Message::Forward),
        _ => None,
    }
}

fn pointer_pressed_message(
    event: &Event,
    status: event::Status,
    window: window::Id,
) -> Option<Message> {
    let Event::Mouse(mouse::Event::ButtonPressed(button)) = event else {
        return None;
    };

    match button {
        mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle => {
            Some(Message::WindowPointerPressed {
                window,
                button: *button,
                status,
            })
        }
        _ => None,
    }
}

pub(super) fn system_theme() -> Theme {
    let mode = match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => AppearanceMode::Dark,
        Ok(dark_light::Mode::Light) | Ok(dark_light::Mode::Unspecified) | Err(_) => {
            AppearanceMode::Light
        }
    };
    fallback_theme(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{self, key, Key};

    fn route_event(event: Event, status: event::Status) -> Option<Message> {
        route_event_with_window(event, status, iced::window::Id::unique())
    }

    fn route_event_with_window(
        event: Event,
        status: event::Status,
        window: window::Id,
    ) -> Option<Message> {
        global_event_message(event, status, window)
    }

    fn key_pressed(key: Key, modifiers: keyboard::Modifiers) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            key,
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn requested_and_observed_window_close_have_distinct_messages() {
        let window = iced::window::Id::unique();
        assert!(matches!(
            route_event_with_window(
                Event::Window(iced::window::Event::CloseRequested),
                event::Status::Ignored,
                window,
            ),
            Some(Message::AuxiliaryWindowCloseRequested(routed_window))
                if routed_window == window
        ));
        assert!(matches!(
            route_event_with_window(
                Event::Window(iced::window::Event::Closed),
                event::Status::Ignored,
                window,
            ),
            Some(Message::ApplicationWindowClosed(routed_window))
                if routed_window == window
        ));
    }

    #[test]
    fn key_press_reports_raw_keyboard_boundary_message() {
        let event = key_pressed(Key::Character("l".into()), keyboard::Modifiers::CTRL);

        let message = route_event(event, event::Status::Captured);

        assert!(matches!(
            message,
            Some(Message::KeyboardKeyPressed {
                key: Key::Character(value),
                modifiers,
                status: event::Status::Captured,
            }) if value == "l" && modifiers.control()
        ));
    }

    #[test]
    fn captured_left_release_reports_source_window_and_status() {
        let window = window::Id::unique();
        let event = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));

        let message = route_event_with_window(event, event::Status::Captured, window);

        assert!(matches!(
            message,
            Some(Message::WindowPointerReleased {
                window: received_window,
                status: event::Status::Captured,
            }) if received_window == window
        ));
    }

    #[test]
    fn captured_cursor_move_updates_cursor_position() {
        let window = window::Id::unique();
        let position = iced::Point::new(12.0, 24.0);
        let event = Event::Mouse(mouse::Event::CursorMoved { position });

        let message = route_event_with_window(event, event::Status::Captured, window);

        let Some(Message::CursorMoved {
            window: received_window,
            position: received,
        }) = message
        else {
            panic!("expected cursor movement message");
        };
        assert_eq!(received_window, window);
        assert_eq!(received.x, position.x);
        assert_eq!(received.y, position.y);
    }

    #[test]
    fn cursor_left_routes_to_window_message() {
        let window = window::Id::unique();
        let event = Event::Mouse(mouse::Event::CursorLeft);

        let message = route_event_with_window(event, event::Status::Ignored, window);

        assert!(matches!(
            message,
            Some(Message::CursorLeft {
                window: received_window
            }) if received_window == window
        ));
    }

    #[test]
    fn window_scale_change_routes_generation_boundary() {
        let window = window::Id::unique();
        let message = route_event_with_window(
            Event::Window(iced::window::Event::Rescaled(1.25)),
            event::Status::Ignored,
            window,
        );

        assert!(matches!(
            message,
            Some(Message::X11Dnd(X11DndMessage::ScaleFactorChanged {
                window: received_window,
                scale_factor,
            })) if received_window == window && scale_factor == 1.25
        ));
    }

    #[test]
    fn iced_external_file_events_have_no_operation_entry() {
        let path = std::path::PathBuf::from("/tmp/external.txt");
        for event in [
            iced::window::Event::FileHovered(path.clone()),
            iced::window::Event::FileDropped(path),
            iced::window::Event::FilesHoveredLeft,
        ] {
            assert!(route_event(Event::Window(event), event::Status::Ignored).is_none());
        }
    }

    #[test]
    fn pointer_press_reports_source_window() {
        let window = window::Id::unique();
        let event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));

        let message = route_event_with_window(event, event::Status::Ignored, window);

        assert!(matches!(
            message,
            Some(Message::WindowPointerPressed {
                window: received_window,
                button: mouse::Button::Left,
                status: event::Status::Ignored,
            }) if received_window == window
        ));
    }

    #[test]
    fn ignored_mouse_navigation_buttons_route_history_navigation() {
        assert!(matches!(
            route_event(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back)),
                event::Status::Ignored,
            ),
            Some(Message::Back)
        ));
        assert!(matches!(
            route_event(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Forward)),
                event::Status::Ignored,
            ),
            Some(Message::Forward)
        ));
    }

    #[test]
    fn modifier_change_updates_keyboard_modifier_state() {
        let event = Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::ALT));

        let message = route_event(event, event::Status::Ignored);

        assert!(matches!(
            message,
            Some(Message::KeyboardModifiersChanged(modifiers)) if modifiers.alt()
        ));
    }

    #[test]
    fn named_key_press_keeps_event_status_for_stateful_router() {
        let event = key_pressed(
            Key::Named(key::Named::ArrowDown),
            keyboard::Modifiers::default(),
        );

        let message = route_event(event, event::Status::Ignored);

        assert!(matches!(
            message,
            Some(Message::KeyboardKeyPressed {
                key: Key::Named(key::Named::ArrowDown),
                status: event::Status::Ignored,
                ..
            })
        ));
    }
}
