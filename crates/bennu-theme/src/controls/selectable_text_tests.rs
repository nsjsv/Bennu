//! 只读文字的不变量：编辑动作永远改不了内容、选择和移动照常；聚焦着的控件吞掉编辑键和文件区
//! 用的键，并向 widget operation 报告自己在用键盘。
//!
//! 内容的测试用真实渲染器（空渲染器的编辑器读不出选区）；控件的事件测试用空渲染器，
//! 靠 operation 聚焦（空渲染器量出来的文字高是 0，点不中）。

use std::any::Any;

use iced::advanced::clipboard::Clipboard;
use iced::advanced::widget::operation::{Focusable, Operation};
use iced::advanced::widget::{self, Tree};
use iced::advanced::{layout, mouse, Layout, Shell};
use iced::keyboard::{self, key};
use iced::widget::text_editor::{Action, Edit, Motion};
use iced::{Element, Event, Rectangle, Size, Theme};

use super::*;
use crate::controls::FocusedTextControl;

#[derive(Debug, Clone, PartialEq)]
struct Performed(Action);

fn content(text: &str) -> SelectableTextContent {
    SelectableTextContent::new(text)
}

#[test]
fn edit_actions_never_change_the_content() {
    let edits = [
        Edit::Insert('x'),
        Edit::Paste(std::sync::Arc::new("pasted".to_owned())),
        Edit::Enter,
        Edit::Indent,
        Edit::Unindent,
        Edit::Backspace,
        Edit::Delete,
    ];
    for edit in edits {
        let mut value = content("hello world");
        value.perform(Action::SelectAll);
        value.perform(Action::Edit(edit.clone()));
        assert_eq!(value.content.text(), "hello world", "{edit:?}");
        assert_eq!(value.text(), "hello world", "{edit:?}");
        assert_eq!(
            value.selection().as_deref(),
            Some("hello world"),
            "{edit:?} left the selection alone"
        );
    }
}

#[test]
fn selection_and_movement_actions_still_apply() {
    let mut value = content("hello world");
    value.perform(Action::SelectAll);
    assert_eq!(value.selection().as_deref(), Some("hello world"));

    value.perform(Action::Move(Motion::Home));
    assert_eq!(value.selection(), None);
    value.perform(Action::Select(Motion::Right));
    value.perform(Action::Select(Motion::Right));
    assert_eq!(value.selection().as_deref(), Some("he"));

    value.perform(Action::Move(Motion::Home));
    value.perform(Action::SelectWord);
    assert_eq!(value.selection().as_deref(), Some("hello"));

    value.perform(Action::SelectLine);
    assert_eq!(value.selection().as_deref(), Some("hello world"));
}

/// 把传进来的所有元素聚焦的 operation：空渲染器点不中控件，靠它代替点击。
struct FocusEverything;

impl Operation for FocusEverything {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        state.focus();
    }
}

#[derive(Default)]
struct FocusReport {
    reported: bool,
}

impl Operation for FocusReport {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Any) {
        self.reported |= state.is::<FocusedTextControl>();
    }
}

struct Harness<'a> {
    element: Element<'a, Performed, Theme, ()>,
    tree: Tree,
    node: layout::Node,
    messages: Vec<Performed>,
}

struct NoClipboard;

impl Clipboard for NoClipboard {
    fn read(&self, _kind: iced::advanced::clipboard::Kind) -> Option<String> {
        None
    }

    fn write(&mut self, _kind: iced::advanced::clipboard::Kind, _contents: String) {}
}

impl<'a> Harness<'a> {
    fn new(content: &'a SelectableTextContent<()>) -> Self {
        let mut element = selectable_text(content, Performed);
        let mut tree = Tree::new(&element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 120.0));
        let node = element.as_widget_mut().layout(&mut tree, &(), &limits);
        Self {
            element,
            tree,
            node,
            messages: Vec::new(),
        }
    }

    fn operate(&mut self, operation: &mut dyn Operation) {
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &(),
            operation,
        );
    }

    fn focus(&mut self) {
        self.operate(&mut FocusEverything);
    }

    fn reports_focus(&mut self) -> bool {
        let mut report = FocusReport::default();
        self.operate(&mut report);
        report.reported
    }

    /// 按下一个键，返回这个键有没有被控件捕获（捕获了文件区就不再收到）。
    fn press(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> bool {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: keyboard::Location::Standard,
            modifiers,
            text: text.map(Into::into),
            repeat: false,
        });
        let bounds = Rectangle::with_size(self.node.size());
        let mut shell = Shell::new(&mut self.messages);
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            mouse::Cursor::Unavailable,
            &(),
            &mut NoClipboard,
            &mut shell,
            &bounds,
        );
        shell.is_event_captured()
    }
}

fn named(key: key::Named) -> keyboard::Key {
    keyboard::Key::Named(key)
}

fn character(character: &str) -> keyboard::Key {
    keyboard::Key::Character(character.into())
}

#[test]
fn a_focused_value_takes_the_keys_the_file_area_would_act_on() {
    let value = SelectableTextContent::<()>::new("hello");
    let mut harness = Harness::new(&value);
    harness.focus();
    let none = keyboard::Modifiers::default();

    let taken = [
        (
            "Delete",
            harness.press(named(key::Named::Delete), none, None),
        ),
        (
            "Backspace",
            harness.press(named(key::Named::Backspace), none, None),
        ),
        ("Enter", harness.press(named(key::Named::Enter), none, None)),
        ("a letter", harness.press(character("n"), none, Some("n"))),
        (
            "Space",
            harness.press(named(key::Named::Space), none, Some(" ")),
        ),
        (
            "Ctrl+A",
            harness.press(character("a"), keyboard::Modifiers::CTRL, None),
        ),
        (
            "Ctrl+C",
            harness.press(character("c"), keyboard::Modifiers::CTRL, None),
        ),
        (
            "an arrow",
            harness.press(named(key::Named::ArrowRight), none, None),
        ),
    ];
    for (name, captured) in taken {
        assert!(captured, "{name} must not reach the file area");
    }
}

#[test]
fn an_unfocused_value_leaves_every_key_to_the_file_area() {
    let value = SelectableTextContent::<()>::new("hello");
    let mut harness = Harness::new(&value);

    assert!(!harness.press(
        named(key::Named::Delete),
        keyboard::Modifiers::default(),
        None
    ));
    assert!(!harness.press(character("n"), keyboard::Modifiers::default(), Some("n")));
    assert!(harness.messages.is_empty());
}

#[test]
fn focus_is_reported_to_operations_only_while_focused() {
    let value = SelectableTextContent::<()>::new("hello");
    let mut harness = Harness::new(&value);
    assert!(!harness.reports_focus());

    harness.focus();
    assert!(harness.reports_focus());
}

#[test]
fn escape_in_a_focused_value_leaves_it_and_takes_the_key() {
    let value = SelectableTextContent::<()>::new("hello");
    let mut harness = Harness::new(&value);
    harness.focus();

    let captured = harness.press(
        named(key::Named::Escape),
        keyboard::Modifiers::default(),
        None,
    );

    assert!(captured, "the app must not treat this Esc as its own");
    assert!(!harness.reports_focus());
}

#[test]
fn escape_in_an_unfocused_value_is_left_to_the_app() {
    let value = SelectableTextContent::<()>::new("hello");
    let mut harness = Harness::new(&value);

    let captured = harness.press(
        named(key::Named::Escape),
        keyboard::Modifiers::default(),
        None,
    );

    assert!(!captured);
}
