//! 滑块、进度条、提示气泡。

#![allow(clippy::disallowed_methods)]

use std::ops::RangeInclusive;
use std::time::Duration;

use iced::advanced::text::Renderer as TextRenderer;
use iced::widget::progress_bar as iced_progress_bar;
use iced::widget::{
    container,
    slider::{self, Slider},
    text, tooltip as iced_tooltip, Container, ProgressBar, Tooltip,
};
use iced::{Background, Border, Element, Length, Theme};

use super::{foreground_tint, knob_color};
use crate::styles::subtle_border_color;
use crate::typography::CAPTION_SIZE;
use crate::ui_colors;

const SLIDER_RAIL_WIDTH: f32 = 4.0;
const SLIDER_HANDLE_RADIUS: f32 = 8.0;
const PROGRESS_GIRTH: f32 = 6.0;
const TOOLTIP_RADIUS: f32 = 6.0;
/// 完整名字的提示框最宽多少，再长就换行。
const FULL_NAME_BUBBLE_MAX_WIDTH: f32 = 360.0;

/// 鼠标停这么久提示才出来：只是划过去不冒东西挡视线。0.5 秒是桌面上通行的等待时长。
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

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

/// 全应用的悬浮提示只从这里来，都要停 `TOOLTIP_DELAY` 才出现。
pub fn tooltip<'a, Message, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    bubble: impl Into<Element<'a, Message, Theme, Renderer>>,
    position: iced_tooltip::Position,
) -> Tooltip<'a, Message, Theme, Renderer>
where
    Renderer: TextRenderer,
{
    Tooltip::new(content, bubble, position).delay(TOOLTIP_DELAY)
}

pub fn tooltip_bubble<'a, Message: 'a>(hint: String) -> Container<'a, Message> {
    container(text(hint).size(CAPTION_SIZE))
        .padding([5, 8])
        .style(tooltip_bubble_style)
}

/// 被省略的名字的完整写法：短的一行，长的到最宽处换行（文件名常常没有空格，按字换）。
pub fn full_name_bubble<'a, Message: 'a>(name: String) -> Container<'a, Message> {
    container(text(name).wrapping(text::Wrapping::WordOrGlyph))
        .padding([5, 8])
        .max_width(FULL_NAME_BUBBLE_MAX_WIDTH)
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

#[cfg(test)]
mod tests {
    use iced::advanced::layout::Limits;
    use iced::advanced::widget::Tree;
    use iced::advanced::{clipboard, mouse, Layout, Shell};
    use iced::widget::{text, tooltip as iced_tooltip, Space};
    use iced::{window, Element, Event, Point, Rectangle, Size, Theme, Vector};

    use super::{tooltip, TOOLTIP_DELAY};

    /// 鼠标放上去先不出提示，停够 `TOOLTIP_DELAY` 才出来。
    #[test]
    fn tooltips_wait_before_showing() {
        let mut hint: Element<'_, (), Theme, ()> = tooltip(
            Space::new().width(100.0).height(20.0),
            text("hint"),
            iced_tooltip::Position::Bottom,
        )
        .into();
        let mut tree = Tree::new(&hint);
        let node = hint.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, 20.0)),
        );
        let viewport = Rectangle::with_size(Size::new(100.0, 20.0));
        let position = Point::new(10.0, 10.0);
        let deliver = |hint: &mut Element<'_, (), Theme, ()>, tree: &mut Tree, event: Event| {
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            hint.as_widget_mut().update(
                tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Available(position),
                &(),
                &mut clipboard::Null,
                &mut shell,
                &viewport,
            );
            hint.as_widget_mut()
                .overlay(tree, Layout::new(&node), &(), &viewport, Vector::ZERO)
                .is_some()
        };

        let shown_on_enter = deliver(
            &mut hint,
            &mut tree,
            Event::Mouse(mouse::Event::CursorMoved { position }),
        );
        assert!(!shown_on_enter);

        std::thread::sleep(TOOLTIP_DELAY);
        let shown_after_waiting = deliver(
            &mut hint,
            &mut tree,
            Event::Window(window::Event::RedrawRequested(std::time::Instant::now())),
        );
        assert!(shown_after_waiting);
    }
}
