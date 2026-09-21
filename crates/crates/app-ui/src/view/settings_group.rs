use iced::widget::{button, column, container, row, Button, Column, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::appearance::{base_text_color, muted_text_color, subtle_border_color};
use crate::matugen_theme::ui_colors;
use crate::model::Message;
use crate::typography::readable_text;

use super::option_controls::hover_background_color;
use super::toggle_switch::switch_control;
use super::{themed_icon, IconSymbol, IconTone};

pub(super) const SETTINGS_GROUP_SPACING: f32 = 16.0;

const GROUP_HEADER_SPACING: f32 = 6.0;
const CARD_PADDING: u16 = 4;
const CARD_BORDER_RADIUS: f32 = 10.0;
const ROW_BORDER_RADIUS: f32 = 6.0;
const ROW_PADDING: [u16; 2] = [8, 12];
const ROW_CONTENT_INSET: u16 = CARD_PADDING + ROW_PADDING[1];

pub(super) fn settings_group<'a>(
    header: &'static str,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    column![settings_group_header(header), settings_card(rows)]
        .spacing(GROUP_HEADER_SPACING)
        .width(Length::Fill)
        .into()
}

pub(super) fn settings_card<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let row_count = rows.len();
    let mut content = Column::new().width(Length::Fill);
    for (index, card_row) in rows.into_iter().enumerate() {
        content = content.push(card_row);
        if index + 1 < row_count {
            content = content.push(row_separator());
        }
    }

    container(content)
        .padding(CARD_PADDING)
        .width(Length::Fill)
        .style(settings_card_style)
        .into()
}

pub(super) fn toggle_setting_row(
    title: &'static str,
    description: Option<&'static str>,
    is_on: bool,
    message: Message,
) -> Element<'static, Message> {
    let mut labels = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(readable_text(title).size(12).width(Length::Fill));
    if let Some(description) = description {
        labels = labels.push(muted_setting_text(description, 11));
    }

    let content = row![labels, switch_control(is_on)]
        .spacing(8)
        .align_y(Alignment::Center);

    button(content)
        .on_press(message)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .style(card_row_button_style())
        .into()
}

pub(super) fn labeled_setting_row<'a>(
    label: &'static str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    container(
        row![readable_text(label).size(12).width(Length::Fill), control]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding(ROW_PADDING)
    .width(Length::Fill)
    .into()
}

pub(super) fn action_setting_row(
    title: &'static str,
    description: &'static str,
) -> Button<'static, Message> {
    let labels = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(readable_text(title).size(12).width(Length::Fill))
        .push(readable_text(description).size(11).width(Length::Fill));

    button(labels)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .style(card_row_button_style())
}

/// 进入二级页面的导航行：左标题右箭头，整行可点。
pub(super) fn navigation_setting_row(title: &'static str) -> Button<'static, Message> {
    let content = row![
        readable_text(title).size(12).width(Length::Fill),
        themed_icon(IconSymbol::ChevronRight, IconTone::Normal, 13.0),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    button(content)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .style(card_row_button_style())
}

pub(super) fn info_setting_row(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .padding(ROW_PADDING)
        .width(Length::Fill)
        .into()
}

pub(super) fn muted_setting_text(label: &'static str, size: u32) -> Element<'static, Message> {
    container(readable_text(label).size(size).width(Length::Fill))
        .width(Length::Fill)
        .style(muted_text_style)
        .into()
}

fn settings_group_header(header: &'static str) -> Element<'static, Message> {
    container(readable_text(header).size(12).width(Length::Fill))
        .padding([0, ROW_CONTENT_INSET])
        .width(Length::Fill)
        .style(muted_text_style)
        .into()
}

fn row_separator() -> Element<'static, Message> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(separator_line_style),
    )
    .padding([0, ROW_PADDING[1]])
    .width(Length::Fill)
    .into()
}

fn settings_card_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(settings_card_background(theme))),
        text_color: Some(base_text_color(theme)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: CARD_BORDER_RADIUS.into(),
        },
        ..container::Style::default()
    }
}

fn settings_card_background(theme: &Theme) -> Color {
    ui_colors(theme).surface_container
}

fn muted_text_style(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(muted_text_color(theme)),
        ..container::Style::default()
    }
}

fn separator_line_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: 0.55,
            ..subtle_border_color(theme)
        })),
        ..container::Style::default()
    }
}

pub(super) fn card_row_button_style() -> fn(&Theme, button::Status) -> button::Style {
    card_row_button_appearance
}

fn card_row_button_appearance(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(hover_background_color(theme)),
        button::Status::Active | button::Status::Disabled => None,
    };

    button::Style {
        background: background.map(Background::Color),
        text_color: base_text_color(theme),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: ROW_BORDER_RADIUS.into(),
        },
        ..button::Style::default()
    }
}
