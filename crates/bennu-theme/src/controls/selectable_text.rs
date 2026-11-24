//! 只读、能选中复制的文字（信息面板的值）。
//!
//! 看上去就是一段普通文字：没有底色、边框和内边距，字号、行高、折行都跟 `text` 一样。
//! 鼠标拖选、双击选词、Shift+点击或方向键扩选、Ctrl+A 全选、Ctrl+C 复制都是 iced
//! `TextEditor` 自带的；选好的文字照输入框的规矩进 PRIMARY（外面包的 `PrimarySelection`）。

use std::fmt;

use iced::advanced::text;
use iced::widget::text_editor::{self, Action, Content, TextEditor};
use iced::{Background, Border, Color, Element, Theme};

use super::fields::selection_color;
use super::primary_selection::{PrimarySelection, TextEditorSelection};
use crate::styles::base_text_color;

/// 只读文字的内容。
///
/// 只读是这个类型自己的约束，不靠调用方各自记得过滤：`Content` 藏在里面，改它的唯一入口是
/// [`perform`](Self::perform)，那里丢掉所有编辑动作。过滤只能放在这一层：`TextEditor` 的键盘绑定
/// 把 Delete、Backspace、Enter、粘贴、输入法提交都变成 `Action::Edit` 交给应用，在控件里
/// 拦按键拦不住输入法；而聚焦着的控件仍要吞掉这些键，否则文件区会把 Delete 当成“删文件”。
/// 所以编辑动作照常发给调用方，由 `perform` 变成什么都不做。
///
/// 内容建好后不会再变，文字原样存一份：调用方拿它和新算出的文字比较，不用每次从编辑器里拼。
pub struct SelectableTextContent<Renderer: text::Renderer = iced::Renderer> {
    text: String,
    content: Content<Renderer>,
}

impl<Renderer: text::Renderer> SelectableTextContent<Renderer> {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            content: Content::with_text(text),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Option<String> {
        self.content.selection()
    }

    /// 应用控件发来的动作：选择、移动、滚动照做，编辑一律不做。
    pub fn perform(&mut self, action: Action) {
        if action.is_edit() {
            return;
        }
        self.content.perform(action);
    }
}

impl<Renderer: text::Renderer> fmt::Debug for SelectableTextContent<Renderer> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelectableTextContent")
            .field("text", &self.text)
            .finish()
    }
}

/// `on_action` 收到的动作要交给 [`SelectableTextContent::perform`]。
pub fn selectable_text<'a, Message, Renderer>(
    content: &'a SelectableTextContent<Renderer>,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Renderer: text::Renderer + 'a,
{
    let editor = TextEditor::new(&content.content)
        .padding(0.0)
        .style(selectable_text_style)
        .on_action(on_action);
    Element::new(PrimarySelection {
        inner: editor.into(),
        selection: Some(Box::new(TextEditorSelection {
            content: &content.content,
        })),
        // 只读：中键粘贴没有意义，也不能把落在这段文字上的中键吞掉。
        accepts_paste: false,
    })
}

fn selectable_text_style(theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    let value = base_text_color(theme);
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: value,
        value,
        selection: selection_color(theme),
    }
}

#[cfg(test)]
#[path = "selectable_text_tests.rs"]
mod tests;
