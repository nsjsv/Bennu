//! 输入框、多行输入区、下拉框。
//!
//! 输入框底色和按钮一样是半透明前景色，叠在什么底上都是“淡淡一层”。
//! 地址栏编辑层要盖住下面的面包屑，它在 `styles::faded_text_input_style`
//! 里自己换算成等效的实色，这个限制不压给所有输入框。

#![allow(clippy::disallowed_methods)]

use std::borrow::Borrow;

use iced::overlay::menu;
use iced::widget::text::LineHeight;
use iced::widget::text_editor::{self, TextEditor};
use iced::widget::text_input::{self, TextInput};
use iced::widget::{pick_list, PickList};
use iced::{Background, Border, Color, Pixels, Shadow, Theme, Vector};

use super::foreground_tint;
use crate::metrics::{CONTROL_LINE_HEIGHT, CONTROL_PADDING_Y, CONTROL_RADIUS, FIELD_PADDING_X};

use crate::styles::{elevation_shadow_color, muted_text_color, subtle_border_color};
use crate::ui_colors;

pub fn text_field<'a, Message: Clone>(placeholder: &str, value: &str) -> TextInput<'a, Message> {
    iced::widget::text_input(placeholder, value)
        .line_height(LineHeight::Absolute(Pixels(CONTROL_LINE_HEIGHT)))
        .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
        .style(text_field_style)
}

/// 嵌在文件条目里的输入框（原地重命名）：字号和内边距跟着条目几何走，
/// 不套 [`text_field`] 的固定 32 高。
pub fn inline_text_field<'a, Message: Clone>(
    placeholder: &str,
    value: &str,
) -> TextInput<'a, Message> {
    iced::widget::text_input(placeholder, value).style(text_field_style)
}

pub fn text_area<'a, Message: Clone>(
    content: &'a text_editor::Content,
) -> TextEditor<'a, iced::advanced::text::highlighter::PlainText, Message> {
    iced::widget::text_editor(content)
        .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
        .style(text_area_style)
}

pub fn dropdown<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_select: impl Fn(T) -> Message + 'a,
) -> PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone,
{
    pick_list(options, selected, on_select)
        .text_line_height(LineHeight::Absolute(Pixels(CONTROL_LINE_HEIGHT)))
        .padding([CONTROL_PADDING_Y, FIELD_PADDING_X + 2.0])
        .style(dropdown_style)
        .menu_style(dropdown_menu_style)
}

/// 输入框和按钮必须一眼分得开：按钮是实心淡灰块，输入框是更淡的底 + 一圈细边框，
/// 聚焦时边框换成强调色。只靠底色时，用户认不出放在卡片里的输入框
/// （2026-11-10 用户验收反馈）。
pub fn text_field_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let colors = ui_colors(theme);
    let (background, border) = match status {
        text_input::Status::Active => (0.03, hairline_border(theme, 0.20)),
        text_input::Status::Hovered => (0.05, hairline_border(theme, 0.32)),
        text_input::Status::Focused { .. } => (0.03, focus_border(theme)),
        text_input::Status::Disabled => (0.02, hairline_border(theme, 0.10)),
    };
    text_input::Style {
        background: Background::Color(foreground_tint(theme, background)),
        border,
        icon: colors.on_surface_variant,
        placeholder: colors.on_surface_variant,
        value: match status {
            text_input::Status::Disabled => muted_text_color(theme),
            _ => colors.on_surface,
        },
        selection: Color {
            a: 0.35,
            ..colors.primary
        },
    }
}

fn text_area_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let field = text_field_style(
        theme,
        match status {
            text_editor::Status::Active => text_input::Status::Active,
            text_editor::Status::Hovered => text_input::Status::Hovered,
            text_editor::Status::Focused { is_hovered } => {
                text_input::Status::Focused { is_hovered }
            }
            text_editor::Status::Disabled => text_input::Status::Disabled,
        },
    );
    text_editor::Style {
        background: field.background,
        border: field.border,
        placeholder: field.placeholder,
        value: field.value,
        selection: field.selection,
    }
}

pub fn dropdown_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let colors = ui_colors(theme);
    let alpha = match status {
        pick_list::Status::Active => 0.08,
        pick_list::Status::Hovered | pick_list::Status::Opened { is_hovered: true } => 0.12,
        pick_list::Status::Opened { is_hovered: false } => 0.10,
    };
    pick_list::Style {
        text_color: colors.on_surface,
        placeholder_color: colors.on_surface_variant,
        handle_color: colors.on_surface_variant,
        background: Background::Color(foreground_tint(theme, alpha)),
        border: Border {
            radius: CONTROL_RADIUS.into(),
            ..Border::default()
        },
    }
}

pub fn dropdown_menu_style(theme: &Theme) -> menu::Style {
    let colors = ui_colors(theme);
    menu::Style {
        background: Background::Color(colors.surface_container_high),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: CONTROL_RADIUS.into(),
        },
        text_color: colors.on_surface,
        selected_text_color: colors.on_surface,
        selected_background: Background::Color(foreground_tint(theme, 0.10)),
        shadow: Shadow {
            color: elevation_shadow_color(theme, 0.18),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
    }
}

fn hairline_border(theme: &Theme, alpha: f32) -> Border {
    Border {
        color: foreground_tint(theme, alpha),
        width: 1.0,
        radius: CONTROL_RADIUS.into(),
    }
}

fn focus_border(theme: &Theme) -> Border {
    Border {
        color: Color {
            a: 0.7,
            ..ui_colors(theme).primary
        },
        width: 2.0,
        radius: CONTROL_RADIUS.into(),
    }
}
