//! 测试用：把一个键按到聚焦着的真实文字控件上，看控件有没有捕获，再把捕获状态原样交给快捷键路由。
//!
//! 控件在空渲染器里排版（没有显卡也行），聚焦靠 operation 代替点击（空渲染器量出来的文字高是 0，
//! 点不中）。

use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::widget::operation::{Focusable, Operation};
use iced::advanced::widget::{Id, Tree};
use iced::advanced::{layout, mouse, Layout, Shell};
use iced::keyboard::{self, key};
use iced::{event, Element, Event, Rectangle, Size, Theme};

use crate::model::Message;

struct FocusEverything;

impl Operation for FocusEverything {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        state.focus();
    }
}

struct NoClipboard;

impl Clipboard for NoClipboard {
    fn read(&self, _kind: clipboard::Kind) -> Option<String> {
        None
    }

    fn write(&mut self, _kind: clipboard::Kind, _contents: String) {}
}

/// 聚焦 `control`，按一下 `key`。返回控件是否捕获了它，以及控件发出的消息。
pub(super) fn press_key_in_focused_control(
    mut control: Element<'_, Message, Theme, ()>,
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    text: Option<&str>,
) -> (event::Status, Vec<Message>) {
    let mut tree = Tree::new(&control);
    let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 120.0));
    let node = control.as_widget_mut().layout(&mut tree, &(), &limits);
    control
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), &(), &mut FocusEverything);

    let event = Event::Keyboard(keyboard::Event::KeyPressed {
        modified_key: key.clone(),
        key,
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    });
    let mut messages = Vec::new();
    let mut shell = Shell::new(&mut messages);
    control.as_widget_mut().update(
        &mut tree,
        &event,
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &(),
        &mut NoClipboard,
        &mut shell,
        &Rectangle::with_size(node.size()),
    );
    let status = if shell.is_event_captured() {
        event::Status::Captured
    } else {
        event::Status::Ignored
    };
    drop(shell);
    (status, messages)
}
