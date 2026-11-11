//! 主窗口上的对话框统一外壳：标题（可带一行灰字说明）、内容、右下角的按钮。
//!
//! 卡片底色比主窗口高一层，圆角、细边框加阴影，看得出是浮在上面的。各对话框只给
//! 标题、内容和按钮，不自己画外框、不自己定内边距和字号，所以所有对话框长一个样。
//! 菜单、补全这类弹出层不是对话框，仍用菜单的样式。

use iced::widget::{column, container, Column, Row, Space};
use iced::{Alignment, Background, Border, Element, Length, Shadow, Theme, Vector};

use crate::appearance::muted_text_color;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::styles::elevation_shadow_color;
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};
use bennu_theme::ui_colors;

const DIALOG_PADDING: f32 = 20.0;
const DIALOG_RADIUS: f32 = 14.0;
const SECTION_SPACING: f32 = 16.0;
const FIELD_SPACING: f32 = 14.0;
pub(super) const DIALOG_WIDTH: f32 = 460.0;
const SUBTITLE_NAME_MAX_CHARS: usize = 32;

pub(super) struct Dialog<'a> {
    title: String,
    subtitle: Option<String>,
    body: Element<'a, Message>,
    actions: Vec<Element<'a, Message>>,
    width: f32,
    /// 内容多的对话框（批量重命名）：撑到这么高为止，内容自己滚动，按钮钉在底部。
    max_height: Option<f32>,
}

impl<'a> Dialog<'a> {
    pub(super) fn new(title: String, body: impl Into<Element<'a, Message>>) -> Self {
        Self {
            title,
            subtitle: None,
            body: body.into(),
            actions: Vec::new(),
            width: DIALOG_WIDTH,
            max_height: None,
        }
    }

    /// 标题下面一行灰字：这次操作的对象（哪个文件、存到哪）。
    pub(super) fn subtitle(mut self, subtitle: String) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    /// 右下角的按钮，从左到右；主按钮放最后。
    pub(super) fn actions(mut self, actions: Vec<Element<'a, Message>>) -> Self {
        self.actions = actions;
        self
    }

    pub(super) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// 撑满可用高度、最多到 `max_height`；内容要自己放进 `Length::Fill` 的滚动区。
    pub(super) fn fill_height(mut self, max_height: f32) -> Self {
        self.max_height = Some(max_height);
        self
    }
}

impl<'a> From<Dialog<'a>> for Element<'a, Message> {
    fn from(dialog: Dialog<'a>) -> Self {
        let mut heading = column![readable_text(dialog.title).size(TITLE_SIZE).font(BOLD)]
            .spacing(4)
            .width(Length::Fill);
        if let Some(subtitle) = dialog.subtitle {
            heading = heading.push(muted_caption(subtitle));
        }
        let mut content = column![heading, dialog.body]
            .spacing(SECTION_SPACING)
            .width(Length::Fill);
        if !dialog.actions.is_empty() {
            content = content.push(
                Row::with_children(
                    std::iter::once(Space::new().width(Length::Fill).into()).chain(dialog.actions),
                )
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let frame = container(content)
            .padding(DIALOG_PADDING)
            .width(Length::Fixed(dialog.width))
            .style(dialog_style);
        match dialog.max_height {
            Some(max_height) => frame.height(Length::Fill).max_height(max_height).into(),
            None => frame.into(),
        }
    }
}

/// 对话框内容：一列带标签的项，间距统一。
pub(super) fn fields<'a>(items: Vec<Element<'a, Message>>) -> Column<'a, Message> {
    Column::with_children(items)
        .spacing(FIELD_SPACING)
        .width(Length::Fill)
}

/// 一项：小号灰字标签在上，控件在下。
pub(super) fn field<'a>(
    label: String,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    column![muted_caption(label), control.into()]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

/// 控件下面的一行说明：灰字。
pub(super) fn hint<'a>(text: String) -> Element<'a, Message> {
    muted_caption(text)
}

/// 控件下面的一行说明：出错时红字。
pub(super) fn error<'a>(text: String) -> Element<'a, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 标题下面那行“X，放到 Y”：完整路径太长，名字就够认出来。
pub(super) fn into_subtitle(subject: &str, directory: &std::path::Path) -> String {
    tr!(
        "%1, into %2",
        format_middle_ellipsized_text(subject, SUBTITLE_NAME_MAX_CHARS),
        format_middle_ellipsized_text(&path_name(directory), SUBTITLE_NAME_MAX_CHARS)
    )
}

/// 路径最后一段；根目录这类没有名字的用整个路径。
pub(super) fn path_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn muted_caption<'a>(text: String) -> Element<'a, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

fn dialog_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_bright)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: colors.outline_variant,
            width: 1.0,
            radius: DIALOG_RADIUS.into(),
        },
        shadow: Shadow {
            color: elevation_shadow_color(theme, 0.16),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 28.0,
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn subtitles_name_the_item_and_the_folder_it_goes_into() {
        assert_eq!(path_name(Path::new("/home/user/docs")), "docs");
        // 根目录没有名字，用整个路径。
        assert_eq!(path_name(Path::new("/")), "/");
        assert_eq!(
            into_subtitle("notes.txt", Path::new("/home/user/docs")),
            tr!("%1, into %2", "notes.txt", "docs")
        );
    }
}
