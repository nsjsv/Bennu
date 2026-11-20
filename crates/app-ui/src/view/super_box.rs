//! 超级输入框的浮层：上面是模式按钮和输入框，下面是结果行。行和选中都由模型算好，这里只画。

use std::path::Path;

use iced::widget::{button, container, mouse_area, row, Column, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::{super_box_input_id, SuperBoxMessage};
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use crate::super_box::{BoxMode, BoxRow, NewEntry, SuperBox};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, text_field, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use bennu_theme::styles::{
    context_menu_highlighted_item_button_style, context_menu_item_button_style, context_menu_style,
};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use crate::appearance::muted_text_color;

pub(super) const SUPER_BOX_WIDTH: f32 = 560.0;
/// 框的上沿离内容区顶部的距离。
pub(super) const SUPER_BOX_TOP: f32 = 12.0;
const PATH_MAX_CHARS: usize = 64;

fn message(message: SuperBoxMessage) -> Message {
    Message::SuperBox(message)
}

pub(super) fn super_box_panel<'a>(super_box: &'a SuperBox, home: &Path) -> Element<'a, Message> {
    let mode = super_box.mode();
    let mode_button = iced::widget::tooltip(
        normal_button(readable_text(mode.label())).on_press(message(SuperBoxMessage::ModePressed)),
        tooltip_bubble(tr!("Tab switches the mode")),
        iced::widget::tooltip::Position::Bottom,
    );
    let input = text_field(&placeholder(mode), super_box.input())
        .id(super_box_input_id())
        .on_input(|text| message(SuperBoxMessage::InputChanged(text)))
        .on_submit(message(SuperBoxMessage::Submitted))
        .width(Length::Fill);
    let mut content = Column::new()
        .push(
            row![mode_button, input]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .spacing(6);
    if let Some(notice) = super_box.notice() {
        content = content.push(error_text(notice.to_owned()));
    }
    let rows = super_box.rows();
    if rows.is_empty() {
        content = content.push(muted(empty_hint(mode, super_box.input())));
    }
    for (index, row) in rows.iter().enumerate() {
        content = content.push(result_row(
            super_box,
            index,
            row,
            super_box.selected() == Some(index),
            home,
        ));
    }
    container(content)
        .padding(8)
        .width(Length::Fixed(SUPER_BOX_WIDTH))
        .style(context_menu_style)
        .into()
}

fn placeholder(mode: BoxMode) -> String {
    match mode {
        BoxMode::Search => tr!("Find a feature, action, setting or favorite"),
        BoxMode::Path => tr!("Go to a path; ~ is your home folder"),
        BoxMode::New => tr!("Name of the new file; end it with / for a folder"),
    }
}

/// 没有结果时的一行：框是空的就说这个模式能做什么，打了字就说没找到。
fn empty_hint(mode: BoxMode, input: &str) -> String {
    if !input.trim().is_empty() {
        return tr!("Nothing matches");
    }
    match mode {
        BoxMode::Search => tr!(
            "Type to find what to do. Tab switches to going to a path or making a new item; g: and n: do it too."
        ),
        BoxMode::Path => tr!("Type a path, or start with / or ~ in search."),
        BoxMode::New => tr!("Type a name; end it with / to make a folder"),
    }
}

fn result_row<'a>(
    super_box: &'a SuperBox,
    index: usize,
    row: &'a BoxRow,
    selected: bool,
    home: &Path,
) -> Element<'a, Message> {
    let (icon, title, detail, hint) = match row {
        BoxRow::Entry(entry_index) => {
            let entry = super_box.entry(*entry_index);
            let mut hint = entry.kind.label();
            if let Some(key) = &entry.key_hint {
                hint = format!("{hint} · {key}");
            }
            (None, entry.title.clone(), entry.detail.clone(), Some(hint))
        }
        BoxRow::History(text) => (Some(IconSymbol::RotateCcw), text.clone(), None, None),
        BoxRow::JumpTo(path) => (
            Some(IconSymbol::ArrowRight),
            tr!("Go to %1", short_path(path, home)),
            None,
            None,
        ),
        BoxRow::JumpSuggestion(path) => {
            (Some(IconSymbol::Folder), short_path(path, home), None, None)
        }
        BoxRow::Create(NewEntry::File(name)) => (
            Some(IconSymbol::Plus),
            tr!("New file “%1”", name),
            None,
            None,
        ),
        BoxRow::Create(NewEntry::Folder(name)) => (
            Some(IconSymbol::Plus),
            tr!("New folder “%1”", name),
            None,
            None,
        ),
        BoxRow::Problem(reason) => return error_text(reason.clone()),
    };
    let mut line = row![].spacing(8).align_y(Alignment::Center);
    match icon {
        Some(icon) => {
            line = line.push(themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE));
        }
        None => line = line.push(Space::new().width(Length::Fixed(MENU_ICON_SIZE))),
    }
    let mut label = row![readable_text(title)]
        .spacing(8)
        .align_y(Alignment::Center);
    if let Some(detail) = detail {
        label = label.push(muted(format_middle_ellipsized_text(
            &detail,
            PATH_MAX_CHARS,
        )));
    }
    line = line.push(container(label).width(Length::Fill));
    if let Some(hint) = hint {
        line = line.push(muted(hint));
    }
    let style = if selected {
        context_menu_highlighted_item_button_style()
    } else {
        context_menu_item_button_style()
    };
    mouse_area(
        button(line)
            .on_press(message(SuperBoxMessage::RowPressed(index)))
            .padding([6, 8])
            .width(Length::Fill)
            .style(style),
    )
    .on_enter(message(SuperBoxMessage::RowHovered(index)))
    .into()
}

/// 主目录下的路径写成 `~/…`，太长时从中间省略。
fn short_path(path: &Path, home: &Path) -> String {
    let shown = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    format_middle_ellipsized_text(&shown, PATH_MAX_CHARS)
}

fn muted(text: String) -> Element<'static, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

fn error_text(text: String) -> Element<'static, Message> {
    container(
        readable_text(text)
            .size(CAPTION_SIZE)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(ui_colors(theme).error),
            }),
    )
    .padding([4, 8])
    .into()
}
