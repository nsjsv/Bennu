//! PRIMARY 选区包装的不变量：选区定下来才写、写过不重写、密码框不写；
//! 中键把 PRIMARY 经输入框自己的编辑消息粘贴进去。
//!
//! 用 iced 的空渲染器 `()` 直接驱动 `Widget::update`。空渲染器的文字命中测试永远没有结果，
//! 点击和拖选落在第 0 个字符：测试里点哪儿都等于点到最前面，所以“点击处”只能验证成
//! “光标确实被点击挪到了点击处”（粘贴出现在最前面而不是原光标处）。空渲染器的多行编辑器
//! 读不出选区，多行输入区只验证粘贴消息和事件通路。

use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::widget::Tree;
use iced::advanced::{layout, mouse, Layout, Shell};
use iced::keyboard::{self, key};
use iced::widget::text_editor::{self, Content};
use iced::widget::text_input;
use iced::{window, Element, Event, Length, Point, Rectangle, Size, Theme};

use super::{TextArea, TextField};

#[derive(Debug, Clone, PartialEq)]
enum Message {
    Input(String),
    Edit(text_editor::Action),
}

#[derive(Default)]
struct FakeClipboard {
    standard: Option<String>,
    primary: Option<String>,
    primary_writes: Vec<String>,
}

impl Clipboard for FakeClipboard {
    fn read(&self, kind: clipboard::Kind) -> Option<String> {
        match kind {
            clipboard::Kind::Standard => self.standard.clone(),
            clipboard::Kind::Primary => self.primary.clone(),
        }
    }

    fn write(&mut self, kind: clipboard::Kind, contents: String) {
        match kind {
            clipboard::Kind::Standard => self.standard = Some(contents),
            clipboard::Kind::Primary => {
                self.primary_writes.push(contents.clone());
                self.primary = Some(contents);
            }
        }
    }
}

struct Harness<'a> {
    element: Element<'a, Message, Theme, ()>,
    tree: Tree,
    node: layout::Node,
    clipboard: FakeClipboard,
    messages: Vec<Message>,
}

impl<'a> Harness<'a> {
    fn new(element: impl Into<Element<'a, Message, Theme, ()>>) -> Self {
        let mut element = element.into();
        let mut tree = Tree::new(&element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 120.0));
        let node = element.as_widget_mut().layout(&mut tree, &(), &limits);
        Self {
            element,
            tree,
            node,
            clipboard: FakeClipboard::default(),
            messages: Vec::new(),
        }
    }

    fn bounds(&self) -> Rectangle {
        self.node.bounds()
    }

    fn inside(&self) -> mouse::Cursor {
        mouse::Cursor::Available(self.bounds().center())
    }

    fn outside(&self) -> mouse::Cursor {
        mouse::Cursor::Available(Point::new(self.bounds().width + 50.0, 0.0))
    }

    /// 返回这次事件是否被捕获。
    fn send(&mut self, event: Event, cursor: mouse::Cursor) -> bool {
        self.dispatch(event, cursor, false)
    }

    /// 前面的控件已经捕获了这次事件。
    fn send_already_captured(&mut self, event: Event, cursor: mouse::Cursor) {
        self.dispatch(event, cursor, true);
    }

    fn dispatch(&mut self, event: Event, cursor: mouse::Cursor, already_captured: bool) -> bool {
        let bounds = self.bounds();
        let mut shell = Shell::new(&mut self.messages);
        if already_captured {
            shell.capture_event();
        }
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            cursor,
            &(),
            &mut self.clipboard,
            &mut shell,
            &bounds,
        );
        shell.is_event_captured()
    }

    fn click(&mut self) {
        let cursor = self.inside();
        self.send(press(mouse::Button::Left), cursor);
        self.send(release(mouse::Button::Left), cursor);
    }

    /// 按住 `modifiers` 按一下 `character`（输入框自己记修饰键，所以要先送修饰键变化）。
    fn chord(&mut self, modifiers: keyboard::Modifiers, character: &str) {
        let cursor = self.inside();
        self.send(
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
            cursor,
        );
        let key = keyboard::Key::Character(character.into());
        self.send(
            Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: key.clone(),
                key,
                physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            }),
            cursor,
        );
        self.send(
            Event::Keyboard(keyboard::Event::ModifiersChanged(
                keyboard::Modifiers::default(),
            )),
            cursor,
        );
    }

    fn press_arrow_right(&mut self) {
        let cursor = self.inside();
        let arrow = keyboard::Key::Named(key::Named::ArrowRight);
        self.send(
            Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: arrow.clone(),
                key: arrow,
                physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::default(),
                text: None,
                repeat: false,
            }),
            cursor,
        );
    }

    /// 下一次重绘：定下来的选区在这时读出来。
    fn redraw(&mut self) {
        let cursor = self.inside();
        self.send(
            Event::Window(window::Event::RedrawRequested(iced::time::Instant::now())),
            cursor,
        );
    }

    /// 应用处理完消息、按新值重建视图：控件树的状态原样保留。
    fn rebuild(&mut self, element: impl Into<Element<'a, Message, Theme, ()>>) {
        self.element = element.into();
        self.tree.diff(&self.element);
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 120.0));
        self.node = self
            .element
            .as_widget_mut()
            .layout(&mut self.tree, &(), &limits);
    }

    fn text_input_state(&mut self) -> &mut text_input::State<()> {
        self.tree.children[0]
            .state
            .downcast_mut::<text_input::State<()>>()
    }
}

fn press(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(button))
}

fn release(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(button))
}

fn field<'a>(value: &str) -> Harness<'a> {
    Harness::new(TextField::<Message, ()>::new("", value).on_input(Message::Input))
}

#[test]
fn a_settled_selection_is_written_to_primary_once() {
    let mut harness = field("hello world");
    harness.click();
    harness.redraw();
    assert!(harness.clipboard.primary_writes.is_empty());

    harness.chord(keyboard::Modifiers::COMMAND, "a");
    harness.redraw();
    assert_eq!(harness.clipboard.primary_writes, ["hello world"]);

    harness.chord(keyboard::Modifiers::COMMAND, "c");
    harness.redraw();
    assert_eq!(harness.clipboard.standard.as_deref(), Some("hello world"));
    assert_eq!(
        harness.clipboard.primary_writes.len(),
        1,
        "the same selection must not be written again"
    );
}

#[test]
fn a_selection_restored_after_being_cleared_is_written_again() {
    let mut harness = field("hello world");
    harness.click();
    harness.chord(keyboard::Modifiers::COMMAND, "a");
    harness.redraw();
    // 方向键收起选区；收起不会清空 PRIMARY。
    harness.press_arrow_right();
    harness.redraw();
    assert_eq!(harness.clipboard.primary.as_deref(), Some("hello world"));
    // 其间别的程序可能抢走了 PRIMARY，再选同一段文字必须写回去。
    harness.chord(keyboard::Modifiers::COMMAND, "a");
    harness.redraw();
    assert_eq!(harness.clipboard.primary_writes.len(), 2);
}

#[test]
fn dragging_writes_only_after_the_button_is_released() {
    let mut harness = field("hello world");
    let cursor = harness.inside();
    harness.send(press(mouse::Button::Left), cursor);
    harness.redraw();
    harness.text_input_state().select_range(3, 8);
    for step in 0..3 {
        let moved = Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(10.0 + step as f32, 10.0),
        });
        harness.send(moved, cursor);
        harness.redraw();
    }
    assert!(
        harness.clipboard.primary_writes.is_empty(),
        "no write while the pointer is still dragging"
    );

    harness.send(release(mouse::Button::Left), cursor);
    harness.redraw();
    assert_eq!(harness.clipboard.primary_writes.len(), 1);
    assert!("hello world".contains(&harness.clipboard.primary_writes[0]));
}

#[test]
fn secure_fields_never_write_primary() {
    let mut harness = Harness::new(
        TextField::<Message, ()>::new("", "hunter2")
            .secure(true)
            .on_input(Message::Input),
    );
    harness.click();
    harness.chord(keyboard::Modifiers::COMMAND, "a");
    harness.redraw();
    assert!(harness.clipboard.primary_writes.is_empty());
}

#[test]
fn a_field_that_lost_focus_leaves_primary_alone() {
    let mut harness = field("hello world");
    harness.click();
    harness.chord(keyboard::Modifiers::COMMAND, "a");
    harness.redraw();
    harness.clipboard.primary = Some("from elsewhere".to_owned());
    // 点在输入框外：失焦，残留的选区不是用户刚选的。
    let outside = harness.outside();
    harness.send(press(mouse::Button::Left), outside);
    harness.send(release(mouse::Button::Left), outside);
    harness.redraw();
    assert_eq!(harness.clipboard.primary.as_deref(), Some("from elsewhere"));
}

#[test]
fn middle_click_pastes_primary_through_on_input_at_the_click() {
    let mut harness = field("abc");
    harness.text_input_state().move_cursor_to_end();
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();

    let captured = harness.send(press(mouse::Button::Middle), cursor);

    assert!(captured);
    // 空渲染器把点击算在第 0 个字符：粘贴出现在最前面，说明光标是被点击挪过去的。
    assert_eq!(harness.messages, [Message::Input("PASTEDabc".to_owned())]);
    assert!(harness.text_input_state().is_focused());
    assert!(harness.clipboard.primary_writes.is_empty());
}

#[test]
fn a_real_paste_after_a_middle_click_reads_the_standard_clipboard() {
    let mut harness = field("abc");
    harness.clipboard.primary = Some("PASTED".to_owned());
    harness.clipboard.standard = Some("STD".to_owned());
    let cursor = harness.inside();
    harness.send(press(mouse::Button::Middle), cursor);

    harness.chord(keyboard::Modifiers::COMMAND, "v");

    assert_eq!(
        harness.messages.last(),
        Some(&Message::Input("PASTEDSTDabc".to_owned())),
        "the middle click must not leave the field believing a paste is still in progress"
    );
}

#[test]
fn middle_click_restores_the_real_modifiers() {
    let mut harness = field("abc");
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();
    harness.send(
        Event::Keyboard(keyboard::Event::ModifiersChanged(
            keyboard::Modifiers::SHIFT,
        )),
        cursor,
    );
    harness.send(press(mouse::Button::Middle), cursor);
    harness.rebuild(TextField::<Message, ()>::new("", "PASTEDabc").on_input(Message::Input));

    // 粘贴后光标在 PASTED 之后；Shift 仍按着，Shift+End 要选中后面的 abc。
    let shift_end = Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(key::Named::End),
        modified_key: keyboard::Key::Named(key::Named::End),
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::SHIFT,
        text: None,
        repeat: false,
    });
    harness.send(shift_end, cursor);
    harness.redraw();

    assert_eq!(harness.clipboard.primary_writes, ["abc"]);
}

#[test]
fn a_middle_click_elsewhere_takes_focus_away_like_a_left_click() {
    let mut harness = field("abc");
    harness.click();
    assert!(harness.text_input_state().is_focused());
    // 中键点在别的输入框上：那个输入框靠合成点击聚焦，这个看不到，必须自己失焦。
    let outside = harness.outside();
    let captured = harness.send(press(mouse::Button::Middle), outside);
    assert!(!harness.text_input_state().is_focused());
    assert!(!captured);
}

#[test]
fn middle_click_pastes_into_secure_fields_without_exposing_them() {
    let mut harness = Harness::new(
        TextField::<Message, ()>::new("", "abc")
            .secure(true)
            .on_input(Message::Input),
    );
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();
    harness.send(press(mouse::Button::Middle), cursor);
    harness.redraw();
    assert_eq!(harness.messages, [Message::Input("PASTEDabc".to_owned())]);
    assert!(harness.clipboard.primary_writes.is_empty());
}

#[test]
fn middle_click_does_nothing_when_primary_is_empty_or_the_cursor_is_outside() {
    for primary in [None, Some(String::new())] {
        let mut harness = field("abc");
        harness.clipboard.primary = primary;
        let cursor = harness.inside();
        let captured = harness.send(press(mouse::Button::Middle), cursor);
        assert!(harness.messages.is_empty());
        assert!(!harness.text_input_state().is_focused());
        assert!(
            captured,
            "an editable field owns the middle click even with nothing to paste"
        );
    }

    let mut harness = field("abc");
    harness.clipboard.primary = Some("PASTED".to_owned());
    let outside = harness.outside();
    let captured = harness.send(press(mouse::Button::Middle), outside);
    assert!(harness.messages.is_empty());
    assert!(!captured);
}

#[test]
fn middle_click_already_taken_by_another_widget_is_left_alone() {
    let mut harness = field("abc");
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();
    harness.send_already_captured(press(mouse::Button::Middle), cursor);
    assert!(harness.messages.is_empty());
}

#[test]
fn middle_click_does_nothing_in_a_disabled_field() {
    let mut harness = Harness::new(TextField::<Message, ()>::new("", "abc"));
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();
    let captured = harness.send(press(mouse::Button::Middle), cursor);
    assert!(harness.messages.is_empty());
    assert!(!captured);
    assert!(!harness.text_input_state().is_focused());
}

#[test]
fn text_area_middle_click_pastes_primary_through_on_action() {
    let content = Content::<()>::with_text("abc");
    let mut harness = Harness::new(
        TextArea::<Message, ()>::new(&content)
            .on_action(Message::Edit)
            .height(Length::Fixed(80.0)),
    );
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();

    let captured = harness.send(press(mouse::Button::Middle), cursor);

    assert!(captured);
    assert!(matches!(
        harness.messages.as_slice(),
        [
            Message::Edit(text_editor::Action::Click(_)),
            Message::Edit(text_editor::Action::Edit(text_editor::Edit::Paste(pasted))),
        ] if pasted.as_str() == "PASTED"
    ));
    // 聚焦后的选区读取路径要能取到多行输入区自己的状态（类型对不上会 panic）。
    harness.click();
    harness.redraw();
}

#[test]
fn text_area_without_on_action_ignores_the_middle_click() {
    let content = Content::<()>::with_text("abc");
    let mut harness = Harness::new(TextArea::<Message, ()>::new(&content));
    harness.clipboard.primary = Some("PASTED".to_owned());
    let cursor = harness.inside();
    let captured = harness.send(press(mouse::Button::Middle), cursor);
    assert!(harness.messages.is_empty());
    assert!(!captured);
}
