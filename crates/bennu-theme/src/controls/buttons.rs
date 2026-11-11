//! 四种按钮：普通（淡灰实底）、主按钮（强调色）、危险（红）、纯图标（透明）。
//! 种类是不同概念，所以拆成不同函数而不是一个带种类参数的函数。

use iced::alignment::Vertical;
use iced::widget::{button, container, Button};
use iced::{Background, Border, Color, Element, Length, Theme};

use super::{foreground_tint, mix};
use crate::metrics::{BUTTON_PADDING_X, CONTROL_HEIGHT, CONTROL_RADIUS};
use crate::styles::muted_text_color;
use crate::ui_colors;

pub fn normal_button<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Button<'a, Message> {
    labeled_button(content).style(normal_button_style)
}

pub fn suggested_button<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Button<'a, Message> {
    labeled_button(content).style(suggested_button_style)
}

pub fn destructive_button<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Button<'a, Message> {
    labeled_button(content).style(destructive_button_style)
}

/// 方形透明按钮，图标在正中。
pub fn icon_button<'a, Message: 'a>(icon: impl Into<Element<'a, Message>>) -> Button<'a, Message> {
    button(container(icon).center(Length::Fill))
        .width(Length::Fixed(CONTROL_HEIGHT))
        .height(Length::Fixed(CONTROL_HEIGHT))
        .padding(0)
        .style(icon_button_style)
}

/// iced 的按钮不会把内容垂直居中；高度固定后用 Fill 高的容器把内容放到中线上。
/// 宽度保持 Shrink：内容宽 Fill 会把按钮撑满整行。
fn labeled_button<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Button<'a, Message> {
    button(
        container(content)
            .height(Length::Fill)
            .align_y(Vertical::Center),
    )
    .height(Length::Fixed(CONTROL_HEIGHT))
    .padding([0.0, BUTTON_PADDING_X])
}

pub fn normal_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Active => 0.08,
        button::Status::Hovered => 0.12,
        button::Status::Pressed => 0.18,
        button::Status::Disabled => 0.04,
    };
    filled_button(
        foreground_tint(theme, alpha),
        enabled_text(theme, status, ui_colors(theme).on_surface),
    )
}

pub fn suggested_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_colors(theme);
    accent_button(theme, status, colors.primary, colors.on_primary)
}

pub fn destructive_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_colors(theme);
    accent_button(theme, status, colors.error, colors.on_error)
}

/// 带开/关状态的按钮（筛选开关、配色卡片）：关时同普通按钮，开时用强调色容器底，
/// 里面的 `IconTone::Selected` 图标与它同一对颜色。
pub fn toggle_button_style(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        if !selected {
            return normal_button_style(theme, status);
        }
        let colors = ui_colors(theme);
        accent_button(
            theme,
            status,
            colors.primary_container,
            colors.on_primary_container,
        )
    }
}

pub fn icon_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active | button::Status::Disabled => Color::TRANSPARENT,
        button::Status::Hovered => foreground_tint(theme, 0.08),
        button::Status::Pressed => foreground_tint(theme, 0.14),
    };
    filled_button(
        background,
        enabled_text(theme, status, ui_colors(theme).on_surface),
    )
}

fn accent_button(
    theme: &Theme,
    status: button::Status,
    fill: Color,
    on_fill: Color,
) -> button::Style {
    match status {
        button::Status::Active => filled_button(fill, on_fill),
        button::Status::Hovered => filled_button(mix(fill, on_fill, 0.1), on_fill),
        button::Status::Pressed => filled_button(mix(fill, on_fill, 0.2), on_fill),
        button::Status::Disabled => {
            filled_button(foreground_tint(theme, 0.04), muted_text_color(theme))
        }
    }
}

fn enabled_text(theme: &Theme, status: button::Status, color: Color) -> Color {
    match status {
        button::Status::Disabled => muted_text_color(theme),
        _ => color,
    }
}

fn filled_button(background: Color, text_color: Color) -> button::Style {
    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border {
            radius: CONTROL_RADIUS.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
