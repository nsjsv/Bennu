//! 底栏上的键盘状态：打字跳转正在打的字，vim 的 `/` 跳转、待定的第一个键、范围选择，
//! 以及按了不适用的动作快捷键时的说明。

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
    let now = std::time::Instant::now();
    let status = if let Some(hint) = &browser.action_hint {
        hint.text.clone()
    } else if browser.vim.is_jumping(now) {
        format!("/{typed}")
    } else if let Some(first) = browser.vim.pending_key() {
        format!("{first}…")
    } else if !typed.is_empty() {
        tr!("Jump to: %1", typed)
    } else if browser.vim.is_visual() {
        tr!("-- Select --")
    } else {
        return None;
    };
    Some(
        text(status)
            .size(size)
            .style(|theme: &Theme| text::Style {
                color: Some(muted_text_color(theme)),
            })
            .into(),
    )
}
