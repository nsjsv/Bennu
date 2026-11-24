//! 输入框、多行输入区、下拉框。
//!
//! 输入框底色和按钮一样是半透明前景色，叠在什么底上都是“淡淡一层”。
//! 地址栏编辑层要盖住下面的面包屑，它在 `styles::faded_text_input_style`
//! 里自己换算成等效的实色，这个限制不压给所有输入框。

#![allow(clippy::disallowed_methods)]

use std::borrow::Borrow;

use iced::advanced::text::{self, highlighter::PlainText};
use iced::advanced::widget;
use iced::alignment;
use iced::overlay::menu;
use iced::widget::text::LineHeight;
use iced::widget::text_editor::{self, TextEditor};
use iced::widget::text_input::{self, TextInput};
use iced::widget::{pick_list, PickList};
use iced::{Background, Border, Color, Element, Length, Padding, Pixels, Shadow, Theme, Vector};

use super::foreground_tint;
use super::primary_selection::{
    PrimarySelection, SelectionSource, TextEditorSelection, TextInputSelection,
};
use crate::metrics::{CONTROL_LINE_HEIGHT, CONTROL_PADDING_Y, CONTROL_RADIUS, FIELD_PADDING_X};

use crate::styles::{elevation_shadow_color, muted_text_color, subtle_border_color};
use crate::ui_colors;

pub fn text_field<'a, Message: Clone>(placeholder: &str, value: &str) -> TextField<'a, Message> {
    TextField::new(placeholder, value)
        .line_height(LineHeight::Absolute(Pixels(CONTROL_LINE_HEIGHT)))
        .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
}

/// 嵌在文件条目里的输入框（原地重命名）：字号和内边距跟着条目几何走，
/// 不套 [`text_field`] 的固定 32 高。
pub fn inline_text_field<'a, Message: Clone>(
    placeholder: &str,
    value: &str,
) -> TextField<'a, Message> {
    TextField::new(placeholder, value)
}

pub fn text_area<'a, Message: Clone>(content: &'a text_editor::Content) -> TextArea<'a, Message> {
    TextArea::new(content)
}

/// 单行输入框：iced 的 `TextInput` 加上 PRIMARY 选区要用的信息。
///
/// 选区要换算成文字、密码框不写选区、禁用的框不接受中键粘贴，这三件事 `TextInput`
/// 都不对外暴露，所以由这里记着；方法与 `TextInput` 同名同义，只转发调用方用到的。
pub struct TextField<'a, Message: Clone, Renderer: text::Renderer = iced::Renderer> {
    input: TextInput<'a, Message, Theme, Renderer>,
    value: String,
    is_secure: bool,
    is_editable: bool,
}

impl<'a, Message: Clone, Renderer: text::Renderer> TextField<'a, Message, Renderer> {
    pub(super) fn new(placeholder: &str, value: &str) -> Self {
        Self {
            input: TextInput::new(placeholder, value).style(text_field_style),
            value: value.to_owned(),
            is_secure: false,
            is_editable: false,
        }
    }

    fn map_input(
        self,
        change: impl FnOnce(
            TextInput<'a, Message, Theme, Renderer>,
        ) -> TextInput<'a, Message, Theme, Renderer>,
    ) -> Self {
        Self {
            input: change(self.input),
            ..self
        }
    }

    pub fn id(self, id: impl Into<widget::Id>) -> Self {
        self.map_input(|input| input.id(id))
    }

    pub fn secure(self, is_secure: bool) -> Self {
        Self {
            is_secure,
            ..self.map_input(|input| input.secure(is_secure))
        }
    }

    /// 和 `TextInput` 一样：不挂 `on_input` 的输入框是禁用的。
    pub fn on_input(self, on_input: impl Fn(String) -> Message + 'a) -> Self {
        Self {
            is_editable: true,
            ..self.map_input(|input| input.on_input(on_input))
        }
    }

    pub fn on_input_maybe(self, on_input: Option<impl Fn(String) -> Message + 'a>) -> Self {
        Self {
            is_editable: on_input.is_some(),
            ..self.map_input(|input| input.on_input_maybe(on_input))
        }
    }

    pub fn on_submit(self, message: Message) -> Self {
        self.map_input(|input| input.on_submit(message))
    }

    pub fn on_submit_maybe(self, message: Option<Message>) -> Self {
        self.map_input(|input| input.on_submit_maybe(message))
    }

    pub fn on_paste(self, on_paste: impl Fn(String) -> Message + 'a) -> Self {
        self.map_input(|input| input.on_paste(on_paste))
    }

    pub fn font(self, font: Renderer::Font) -> Self {
        self.map_input(|input| input.font(font))
    }

    pub fn width(self, width: impl Into<Length>) -> Self {
        self.map_input(|input| input.width(width))
    }

    pub fn padding<P: Into<Padding>>(self, padding: P) -> Self {
        self.map_input(|input| input.padding(padding))
    }

    pub fn size(self, size: impl Into<Pixels>) -> Self {
        self.map_input(|input| input.size(size))
    }

    pub fn line_height(self, line_height: impl Into<LineHeight>) -> Self {
        self.map_input(|input| input.line_height(line_height))
    }

    pub fn align_x(self, alignment: impl Into<alignment::Horizontal>) -> Self {
        self.map_input(|input| input.align_x(alignment))
    }

    pub fn style(
        self,
        style: impl Fn(&Theme, text_input::Status) -> text_input::Style + 'a,
    ) -> Self {
        self.map_input(|input| input.style(style))
    }
}

impl<'a, Message, Renderer> From<TextField<'a, Message, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Renderer: text::Renderer + 'a,
{
    fn from(field: TextField<'a, Message, Renderer>) -> Self {
        let selection = (!field.is_secure).then(|| {
            Box::new(TextInputSelection::<Renderer::Paragraph>::new(field.value))
                as Box<dyn SelectionSource + 'a>
        });
        Element::new(PrimarySelection {
            inner: field.input.into(),
            selection,
            accepts_paste: field.is_editable,
        })
    }
}

/// 多行输入区：iced 的 `TextEditor` 加上 PRIMARY 选区要用的信息。
pub struct TextArea<'a, Message: Clone, Renderer: text::Renderer = iced::Renderer> {
    editor: TextEditor<'a, PlainText, Message, Theme, Renderer>,
    content: &'a text_editor::Content<Renderer>,
    is_editable: bool,
}

impl<'a, Message: Clone, Renderer: text::Renderer> TextArea<'a, Message, Renderer> {
    pub(super) fn new(content: &'a text_editor::Content<Renderer>) -> Self {
        Self {
            editor: TextEditor::new(content)
                .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
                .style(text_area_style),
            content,
            is_editable: false,
        }
    }

    pub fn placeholder(self, placeholder: impl text::IntoFragment<'a>) -> Self {
        Self {
            editor: self.editor.placeholder(placeholder),
            ..self
        }
    }

    /// 和 `TextEditor` 一样：不挂 `on_action` 的输入区是禁用的。
    pub fn on_action(self, on_edit: impl Fn(text_editor::Action) -> Message + 'a) -> Self {
        Self {
            editor: self.editor.on_action(on_edit),
            is_editable: true,
            ..self
        }
    }

    pub fn height(self, height: impl Into<Length>) -> Self {
        Self {
            editor: self.editor.height(height),
            ..self
        }
    }
}

impl<'a, Message, Renderer> From<TextArea<'a, Message, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Renderer: text::Renderer + 'a,
{
    fn from(area: TextArea<'a, Message, Renderer>) -> Self {
        Element::new(PrimarySelection {
            inner: area.editor.into(),
            selection: Some(Box::new(TextEditorSelection {
                content: area.content,
            })),
            accepts_paste: area.is_editable,
        })
    }
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
