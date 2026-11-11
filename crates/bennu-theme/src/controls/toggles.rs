//! 复选框、开关、分段选择。

#![allow(clippy::disallowed_methods)]

use iced::widget::{button, checkbox, container, row, text, tooltip, Checkbox, Row, Space};
use iced::{Alignment, Background, Border, Element, Length, Shadow, Theme, Vector};

use super::feedback::tooltip_bubble;
use super::{foreground_tint, knob_color, raised_surface};
use crate::metrics::{CHECKBOX_RADIUS, CONTROL_HEIGHT, CONTROL_RADIUS};
use crate::styles::{elevation_shadow_color, muted_text_color};
use crate::ui_colors;

const CHECKBOX_SIZE: f32 = 16.0;
const CHECKBOX_SPACING: f32 = 8.0;

const SWITCH_THUMB: f32 = 16.0;
const SWITCH_PADDING: f32 = 3.0;
/// 轨道高度由圆点和内边距推出：单独写死会让圆点偏离中线（旧实现 22 ≠ 3+14+3）。
pub const SWITCH_HEIGHT: f32 = SWITCH_THUMB + 2.0 * SWITCH_PADDING;
pub const SWITCH_WIDTH: f32 = 2.0 * SWITCH_HEIGHT - 2.0 * SWITCH_PADDING;

/// 分段选择底槽与选中块之间的缝；工具栏分段按钮组共用。
pub(crate) const SEGMENT_INSET: f32 = 3.0;

pub fn check_box<'a, Message: 'a>(is_checked: bool) -> Checkbox<'a, Message> {
    checkbox(is_checked)
        .size(CHECKBOX_SIZE)
        .spacing(CHECKBOX_SPACING)
        .style(check_box_style)
}

/// 只画开关本身；点击由所在的整行负责（设置行整行就是按钮）。
pub fn switch<'a, Message: 'a>(is_on: bool) -> Element<'a, Message> {
    let thumb = container(Space::new())
        .width(Length::Fixed(SWITCH_THUMB))
        .height(Length::Fixed(SWITCH_THUMB))
        .style(move |theme: &Theme| switch_thumb_style(theme, is_on));
    let spacer = Space::new().width(Length::Fill);
    let content: Row<'a, Message> = if is_on {
        row![spacer, thumb]
    } else {
        row![thumb, spacer]
    };

    container(content.align_y(Alignment::Center))
        .padding(SWITCH_PADDING)
        .width(Length::Fixed(SWITCH_WIDTH))
        .height(Length::Fixed(SWITCH_HEIGHT))
        .style(move |theme: &Theme| switch_track_style(theme, is_on))
        .into()
}

pub struct SegmentedChoice<Message> {
    pub label: String,
    pub selected: bool,
    /// None = 只显示、不可点。
    pub message: Option<Message>,
    /// 悬停补充说明；None = 无。
    pub tooltip: Option<String>,
}

/// 2~4 个短标签的互斥选择：浅灰底槽，选中的那格是一块抬起的浅色块。
pub fn segmented_choice_row<'a, Message: Clone + 'a>(
    choices: Vec<SegmentedChoice<Message>>,
) -> Element<'a, Message> {
    let mut segments = Row::new().spacing(0).align_y(Alignment::Center);
    for choice in choices {
        segments = segments.push(segment(choice));
    }
    container(segments)
        .padding(SEGMENT_INSET)
        .width(Length::Fill)
        .height(Length::Fixed(CONTROL_HEIGHT))
        .style(segment_track_style)
        .into()
}

fn segment<'a, Message: Clone + 'a>(choice: SegmentedChoice<Message>) -> Element<'a, Message> {
    let selected = choice.selected;
    let label = container(text(choice.label)).center(Length::Fill);
    let mut segment = button(label)
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .padding([0, 8])
        .style(move |theme: &Theme, status| segment_style(theme, status, selected));
    if let Some(message) = choice.message {
        segment = segment.on_press(message);
    }
    match choice.tooltip {
        Some(hint) => tooltip(segment, tooltip_bubble(hint), tooltip::Position::Bottom).into(),
        None => segment.into(),
    }
}

fn check_box_style(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let colors = ui_colors(theme);
    let (is_checked, hovered, disabled) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true, false),
        checkbox::Status::Disabled { is_checked } => (is_checked, false, true),
    };
    let (background, border_color) = if is_checked {
        let fill = if disabled {
            foreground_tint(theme, 0.2)
        } else {
            colors.primary
        };
        (fill, fill)
    } else {
        (
            foreground_tint(theme, if hovered { 0.10 } else { 0.06 }),
            foreground_tint(theme, if hovered { 0.55 } else { 0.4 }),
        )
    };
    checkbox::Style {
        background: Background::Color(background),
        icon_color: colors.on_primary,
        border: Border {
            color: border_color,
            width: 1.5,
            radius: CHECKBOX_RADIUS.into(),
        },
        text_color: disabled.then(|| muted_text_color(theme)),
    }
}

fn switch_track_style(theme: &Theme, is_on: bool) -> container::Style {
    container::Style {
        background: Some(Background::Color(if is_on {
            ui_colors(theme).primary
        } else {
            foreground_tint(theme, 0.22)
        })),
        border: Border {
            radius: (SWITCH_HEIGHT / 2.0).into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

fn switch_thumb_style(theme: &Theme, is_on: bool) -> container::Style {
    container::Style {
        background: Some(Background::Color(if is_on {
            ui_colors(theme).on_primary
        } else {
            knob_color(theme)
        })),
        border: Border {
            radius: (SWITCH_THUMB / 2.0).into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: elevation_shadow_color(theme, 0.2),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..container::Style::default()
    }
}

pub(crate) fn segment_track_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(foreground_tint(theme, 0.07))),
        border: Border {
            radius: CONTROL_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

pub(crate) fn segment_style(
    theme: &Theme,
    status: button::Status,
    selected: bool,
) -> button::Style {
    let colors = ui_colors(theme);
    let (background, shadow) = if selected {
        (
            raised_surface(theme),
            Shadow {
                color: elevation_shadow_color(theme, 0.14),
                offset: Vector::new(0.0, 1.0),
                blur_radius: 3.0,
            },
        )
    } else {
        let tint = match status {
            button::Status::Hovered => 0.05,
            button::Status::Pressed => 0.09,
            button::Status::Active | button::Status::Disabled => 0.0,
        };
        (foreground_tint(theme, tint), Shadow::default())
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: match status {
            button::Status::Disabled => muted_text_color(theme),
            _ => colors.on_surface,
        },
        border: Border {
            radius: (CONTROL_RADIUS - SEGMENT_INSET).into(),
            ..Border::default()
        },
        shadow,
        ..button::Style::default()
    }
}
