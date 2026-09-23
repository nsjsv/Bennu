//! 地址输入框焦点探查 operation（从 main.rs 拆出控制行数）：遍历控件
//! 树读取目标输入框的焦点态（主软件 windows.rs 的 TextInputFocusCheck
//! 同构；Id 按请求路径命名空间保证只命中本窗口的输入框）。

use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::{window, Rectangle};

use crate::Message;

pub(crate) struct AddressInputFocusCheck {
    window: window::Id,
    target: iced::widget::Id,
    is_focused: bool,
}

impl AddressInputFocusCheck {
    pub(crate) fn new(window: window::Id, target: iced::widget::Id) -> Self {
        Self {
            window,
            target,
            is_focused: false,
        }
    }
}

impl Operation<Message> for AddressInputFocusCheck {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if id == Some(&self.target) {
            self.is_focused = state.is_focused();
        }
    }

    fn finish(&self) -> Outcome<Message> {
        Outcome::Some(Message::AddressInputFocusChecked {
            window: self.window,
            is_focused: self.is_focused,
        })
    }
}
