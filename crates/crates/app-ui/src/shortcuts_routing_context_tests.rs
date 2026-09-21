use iced::keyboard::{key, Key};

use super::{FileBrowserShortcutOwnership, ShortcutAction, ShortcutRoutingContext};

#[test]
fn application_shortcuts_keep_application_routing() {
    for action in [
        ShortcutAction::Refresh,
        ShortcutAction::Escape,
        ShortcutAction::FocusPathInput,
        ShortcutAction::NavigateBack,
        ShortcutAction::NavigateForward,
        ShortcutAction::NavigateUp,
    ] {
        assert_eq!(
            action.routing_context(&Key::Character("x".into())),
            ShortcutRoutingContext::Application
        );
    }
}

#[test]
fn native_delete_relies_on_widget_capture_before_file_routing() {
    assert_eq!(
        ShortcutAction::Delete.routing_context(&Key::Named(key::Named::Delete)),
        ShortcutRoutingContext::FileBrowserContent(
            FileBrowserShortcutOwnership::CapturedDeleteEvent,
        )
    );
}

#[test]
fn rebound_delete_probes_focused_text_input() {
    assert_eq!(
        ShortcutAction::Delete.routing_context(&Key::Character("d".into())),
        ShortcutRoutingContext::FileBrowserContent(
            FileBrowserShortcutOwnership::FocusedTextInputProbe,
        )
    );
}

#[test]
fn other_file_editing_shortcuts_probe_focused_text_input() {
    for action in [
        ShortcutAction::SelectAll,
        ShortcutAction::Copy,
        ShortcutAction::Paste,
        ShortcutAction::Cut,
        ShortcutAction::Undo,
        ShortcutAction::Redo,
    ] {
        assert_eq!(
            action.routing_context(&Key::Character("x".into())),
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::FocusedTextInputProbe,
            )
        );
    }
}
