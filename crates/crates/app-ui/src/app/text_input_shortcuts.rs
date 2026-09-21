use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome, TextInput};
use iced::{Rectangle, Task};

use crate::model::Message;
use crate::shortcuts::ShortcutAction;
use crate::view::rename_input_id;

pub(super) fn route_ignored_file_content_shortcut(action: ShortcutAction) -> Task<Message> {
    advanced_widget::operate(FocusedTextInputShortcutRoute::new(action))
}

struct FocusedWidget {
    state_address: usize,
    id: Option<advanced_widget::Id>,
}

struct FocusedTextInputShortcutRoute {
    action: ShortcutAction,
    focused_widgets: Vec<FocusedWidget>,
    text_input_state_addresses: Vec<usize>,
}

impl FocusedTextInputShortcutRoute {
    fn new(action: ShortcutAction) -> Self {
        Self {
            action,
            focused_widgets: Vec::new(),
            text_input_state_addresses: Vec::new(),
        }
    }

    fn focused_text_input(&self) -> Option<&FocusedWidget> {
        self.focused_widgets.iter().find(|focused_widget| {
            self.text_input_state_addresses
                .contains(&focused_widget.state_address)
        })
    }
}

impl Operation<Message> for FocusedTextInputShortcutRoute {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&advanced_widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if state.is_focused() {
            self.focused_widgets.push(FocusedWidget {
                state_address: widget_state_address(state),
                id: id.cloned(),
            });
        }
    }

    fn text_input(
        &mut self,
        _id: Option<&advanced_widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn TextInput,
    ) {
        self.text_input_state_addresses
            .push(widget_state_address(state));
    }

    fn finish(&self) -> Outcome<Message> {
        let Some(focused_text_input) = self.focused_text_input() else {
            return Outcome::Some(Message::FileContentShortcutRouted(self.action));
        };
        let rename_id: advanced_widget::Id = rename_input_id().into();

        if focused_text_input.id.as_ref() == Some(&rename_id) {
            return match self.action {
                ShortcutAction::Undo => Outcome::Some(Message::RenameInputUndoRequested),
                ShortcutAction::Redo => Outcome::Some(Message::RenameInputRedoRequested),
                _ => Outcome::None,
            };
        }

        Outcome::None
    }
}

fn widget_state_address<State: ?Sized>(state: &mut State) -> usize {
    // 同一输入框会以两个 trait 视图被访问，数据指针用于在回调顺序未知时关联同一份 widget 状态。
    std::ptr::from_mut(state).cast::<()>() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TextInputFocusProbe {
        focused: bool,
    }

    impl TextInput for TextInputFocusProbe {
        fn text(&self) -> &str {
            ""
        }

        fn move_cursor_to_front(&mut self) {}

        fn move_cursor_to_end(&mut self) {}

        fn move_cursor_to(&mut self, _position: usize) {}

        fn select_all(&mut self) {}

        fn select_range(&mut self, _start: usize, _end: usize) {}
    }

    impl Focusable for TextInputFocusProbe {
        fn is_focused(&self) -> bool {
            self.focused
        }

        fn focus(&mut self) {
            self.focused = true;
        }

        fn unfocus(&mut self) {
            self.focused = false;
        }
    }

    #[test]
    fn focused_text_input_consumes_file_shortcut_when_text_input_is_visited_first() {
        let mut operation = FocusedTextInputShortcutRoute::new(ShortcutAction::SelectAll);
        let input_id: advanced_widget::Id = iced::widget::Id::new("focused-input").into();
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_text_input_consumes_file_shortcut_when_focusable_is_visited_first() {
        let mut operation = FocusedTextInputShortcutRoute::new(ShortcutAction::SelectAll);
        let input_id: advanced_widget::Id = iced::widget::Id::new("focused-input").into();
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_text_input_without_id_still_consumes_file_shortcut() {
        let mut operation = FocusedTextInputShortcutRoute::new(ShortcutAction::Undo);
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.text_input(None, Rectangle::default(), &mut input_probe);
        operation.focusable(None, Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_rename_input_routes_undo_and_redo_to_text_history() {
        let rename_id: advanced_widget::Id = rename_input_id().into();
        let mut input_probe = TextInputFocusProbe { focused: true };
        let mut undo_operation = FocusedTextInputShortcutRoute::new(ShortcutAction::Undo);
        let mut redo_operation = FocusedTextInputShortcutRoute::new(ShortcutAction::Redo);

        undo_operation.text_input(Some(&rename_id), Rectangle::default(), &mut input_probe);
        undo_operation.focusable(Some(&rename_id), Rectangle::default(), &mut input_probe);
        redo_operation.text_input(Some(&rename_id), Rectangle::default(), &mut input_probe);
        redo_operation.focusable(Some(&rename_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(
            undo_operation.finish(),
            Outcome::Some(Message::RenameInputUndoRequested)
        ));
        assert!(matches!(
            redo_operation.finish(),
            Outcome::Some(Message::RenameInputRedoRequested)
        ));
    }

    #[test]
    fn missing_focused_text_input_routes_original_file_shortcut() {
        let mut operation = FocusedTextInputShortcutRoute::new(ShortcutAction::Undo);
        let input_id: advanced_widget::Id = iced::widget::Id::new("unfocused-input").into();
        let mut input_probe = TextInputFocusProbe::default();

        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(
            operation.finish(),
            Outcome::Some(Message::FileContentShortcutRouted(ShortcutAction::Undo))
        ));
    }
}
