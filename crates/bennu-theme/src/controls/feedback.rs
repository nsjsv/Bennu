//! 滑块、进度条、提示气泡。

#![allow(clippy::disallowed_methods)]

use std::ops::RangeInclusive;

use iced::widget::progress_bar as iced_progress_bar;
use iced::widget::{
    container,
    slider::{self, Slider},
    text, Container, ProgressBar,
};
use iced::{Background, Border, Length, Theme};

use super::{foreground_tint, knob_color};
use crate::styles::subtle_border_color;
use crate::typography::CAPTION_SIZE;
use crate::ui_colors;

const SLIDER_RAIL_WIDTH: f32 = 4.0;
const SLIDER_HANDLE_RADIUS: f32 = 8.0;
const PROGRESS_GIRTH: f32 = 6.0;
const TOOLTIP_RADIUS: f32 = 6.0;

pub fn slider<'a, T, Message>(
    range: RangeInclusive<T>,
    value: T,
    on_change: impl Fn(T) -> Message + 'a,
) -> Slider<'a, T, Message>
where
    T: Copy + From<u8> + PartialOrd,
    Message: Clone,
{
    iced::widget::slider(range, value, on_change)
        .height(2.0 * SLIDER_HANDLE_RADIUS)
        .style(slider_style)
}

pub fn progress_bar<'a>(range: RangeInclusive<f32>, value: f32) -> ProgressBar<'a> {
    iced_progress_bar(range, value)
        .girth(Length::Fixed(PROGRESS_GIRTH))
        .style(progress_bar_style)
}

pub fn tooltip_bubble<'a, Message: 'a>(hint: String) -> Container<'a, Message> {
    container(text(hint).size(CAPTION_SIZE))
        .padding([5, 8])
        .style(tooltip_bubble_style)
}

pub fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let colors = ui_colors(theme);
    let handle_border = match status {
        slider::Status::Active => foreground_tint(theme, 0.18),
        slider::Status::Hovered | slider::Status::Dragged => foreground_tint(theme, 0.3),
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (
                Background::Color(colors.primary),
                Background::Color(foreground_tint(theme, 0.15)),
            ),
            width: SLIDER_RAIL_WIDTH,
            border: Border {
                radius: (SLIDER_RAIL_WIDTH / 2.0).into(),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle {
                radius: SLIDER_HANDLE_RADIUS,
            },
            background: Background::Color(knob_color(theme)),
            border_width: 1.0,
            border_color: handle_border,
        },
    }
}

pub fn progress_bar_style(theme: &Theme) -> iced_progress_bar::Style {
    iced_progress_bar::Style {
        background: Background::Color(foreground_tint(theme, 0.12)),
        bar: Background::Color(ui_colors(theme).primary),
        border: Border {
            radius: (PROGRESS_GIRTH / 2.0).into(),
            ..Border::default()
        },
    }
}

pub fn tooltip_bubble_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_highest)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: TOOLTIP_RADIUS.into(),
        },
        ..container::Style::default()
    }
}
