use std::any::Any;

use bennu_theme::controls::FocusedTextControl;
use iced::advanced::widget as advanced_widget;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome, TextInput};
use iced::{Rectangle, Task};

use crate::model::Message;
use crate::shortcuts::{FileContentKey, ShortcutAction};
use crate::view::rename_input_id;

pub(super) fn route_ignored_file_content_key(content_key: FileContentKey) -> Task<Message> {
    advanced_widget::operate(FocusedTextInputShortcutRoute::new(content_key))
}

struct FocusedWidget {
    state_address: usize,
    id: Option<advanced_widget::Id>,
}

struct FocusedTextInputShortcutRoute {
    content_key: FileContentKey,
    focused_widgets: Vec<FocusedWidget>,
    text_input_state_addresses: Vec<usize>,
    /// 多行文字控件（`TextEditor`）不报告“是文字输入”，靠 `bennu_theme::controls` 补报的标记认。
    text_control_focused: bool,
}

impl FocusedTextInputShortcutRoute {
    fn new(content_key: FileContentKey) -> Self {
        Self {
            content_key,
            focused_widgets: Vec::new(),
            text_input_state_addresses: Vec::new(),
            text_control_focused: false,
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

    fn custom(
        &mut self,
        _id: Option<&advanced_widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Any,
    ) {
        self.text_control_focused |= state.is::<FocusedTextControl>();
    }

    fn finish(&self) -> Outcome<Message> {
        let Some(focused_text_input) = self.focused_text_input() else {
            return if self.text_control_focused {
                Outcome::None
            } else {
                Outcome::Some(Message::FileContentKeyRouted(self.content_key.clone()))
            };
        };
        let rename_id: advanced_widget::Id = rename_input_id().into();

        if focused_text_input.id.as_ref() == Some(&rename_id) {
            return match self.content_key {
                FileContentKey::Action(ShortcutAction::Undo) => {
                    Outcome::Some(Message::RenameInputUndoRequested)
                }
                FileContentKey::Action(ShortcutAction::Redo) => {
                    Outcome::Some(Message::RenameInputRedoRequested)
                }
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
        let mut operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::SelectAll));
        let input_id: advanced_widget::Id = iced::widget::Id::new("focused-input").into();
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_text_input_consumes_file_shortcut_when_focusable_is_visited_first() {
        let mut operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::SelectAll));
        let input_id: advanced_widget::Id = iced::widget::Id::new("focused-input").into();
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_text_input_without_id_still_consumes_file_shortcut() {
        let mut operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::Undo));
        let mut input_probe = TextInputFocusProbe { focused: true };

        operation.text_input(None, Rectangle::default(), &mut input_probe);
        operation.focusable(None, Rectangle::default(), &mut input_probe);

        assert!(matches!(operation.finish(), Outcome::None));
    }

    #[test]
    fn focused_rename_input_routes_undo_and_redo_to_text_history() {
        let rename_id: advanced_widget::Id = rename_input_id().into();
        let mut input_probe = TextInputFocusProbe { focused: true };
        let mut undo_operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::Undo));
        let mut redo_operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::Redo));

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

    /// 聚焦只读文字：真实控件被 `FocusedTextInputShortcutRoute` 走一遍，看它认不认。
    /// 空渲染器没有显卡也能量出控件，聚焦靠 operation 代替点击。
    fn route_through_selectable_text(
        focused: bool,
        content_key: FileContentKey,
    ) -> Outcome<Message> {
        use bennu_theme::controls::{selectable_text, SelectableTextContent};
        use iced::advanced::widget::operation::{black_box, Focusable};
        use iced::advanced::widget::Tree;
        use iced::advanced::{layout, Layout};
        use iced::{Element, Size, Theme};

        struct FocusEverything;
        impl Operation for FocusEverything {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                _id: Option<&advanced_widget::Id>,
                _bounds: Rectangle,
                state: &mut dyn Focusable,
            ) {
                state.focus();
            }
        }

        let content = SelectableTextContent::<()>::new("value");
        let mut element: Element<'_, (), Theme, ()> = selectable_text(&content, |_| ());
        let mut tree = Tree::new(&element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 120.0));
        let node = element.as_widget_mut().layout(&mut tree, &(), &limits);
        let layout = Layout::new(&node);
        if focused {
            element
                .as_widget_mut()
                .operate(&mut tree, layout, &(), &mut FocusEverything);
        }
        let mut operation = FocusedTextInputShortcutRoute::new(content_key);
        element.as_widget_mut().operate(
            &mut tree,
            layout,
            &(),
            &mut black_box::<Message, ()>(&mut operation),
        );
        operation.finish()
    }

    /// Ctrl+Z 这类编辑器不认、不会捕获的键：多行文字控件聚焦时也得留在文字里，不能落到文件撤销上。
    #[test]
    fn focused_selectable_text_consumes_file_shortcut() {
        for content_key in [
            FileContentKey::Action(ShortcutAction::Undo),
            FileContentKey::Action(ShortcutAction::SelectAll),
        ] {
            assert!(matches!(
                route_through_selectable_text(true, content_key),
                Outcome::None
            ));
        }
    }

    #[test]
    fn unfocused_selectable_text_routes_original_file_shortcut() {
        assert!(matches!(
            route_through_selectable_text(false, FileContentKey::Action(ShortcutAction::Undo)),
            Outcome::Some(Message::FileContentKeyRouted(FileContentKey::Action(
                ShortcutAction::Undo
            )))
        ));
    }

    #[test]
    fn missing_focused_text_input_routes_original_file_shortcut() {
        let mut operation =
            FocusedTextInputShortcutRoute::new(FileContentKey::Action(ShortcutAction::Undo));
        let input_id: advanced_widget::Id = iced::widget::Id::new("unfocused-input").into();
        let mut input_probe = TextInputFocusProbe::default();

        operation.text_input(Some(&input_id), Rectangle::default(), &mut input_probe);
        operation.focusable(Some(&input_id), Rectangle::default(), &mut input_probe);

        assert!(matches!(
            operation.finish(),
            Outcome::Some(Message::FileContentKeyRouted(FileContentKey::Action(
                ShortcutAction::Undo
            )))
        ));
    }
}
