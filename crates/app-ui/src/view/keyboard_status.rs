//! 底栏上的键盘状态：打字跳转正在打的字。

use iced::widget::text;
use iced::{Element, Theme};

use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::model::Message;

pub(crate) fn keyboard_status_label(
    browser: &FileBrowser,
    size: f32,
) -> Option<Element<'static, Message>> {
    let typed = browser.type_ahead.typed();
    if typed.is_empty() {
        return None;
    }
    Some(
        text(tr!("Jump to: %1", typed))
            .size(size)
            .style(|theme: &Theme| text::Style {
                color: Some(muted_text_color(theme)),
            })
            .into(),
    )
}
