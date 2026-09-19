//! 选择窗口视图：导航栏（上级 + 面包屑 + 过滤器）、目录列表、
//! SaveFile 文件名输入与确认按钮。纯展示，交互全部转成 `SessionMessage`。

use std::path::Path;

use file_core::entry::{DirectoryEntry, FileKind};
use iced::theme::Palette;
use iced::widget::{button, column, container, mouse_area, pick_list, row, scrollable, text, text_input};
use iced::widget::{Column, Row, Space};
use iced::{alignment, Border, Color, Element, Length, Padding, Pixels, Size, Theme};

use crate::picker_request::PickerKind;
use crate::picker_session::{
    breadcrumb_chain, DirectoryListing, PickerSession, SessionMessage,
};

/// 每请求窗口的初始尺寸。
pub(crate) fn window_size() -> Size {
    Size::new(820.0, 560.0)
}

pub(crate) fn window_min_size() -> Size {
    Size::new(640.0, 420.0)
}

fn translucent(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// 窗口内容。`emit` 由上层提供，负责把会话消息与窗口关联。
pub(crate) fn picker_window_view<'a>(
    session: &'a PickerSession,
    theme: &'a Theme,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Element<'a, SessionMessage> {
    let palette = theme.palette();

    let mut layout: Column<'a, SessionMessage> = column![].spacing(8).padding(8);
    layout = layout.push(navigation_bar(session, palette, emit.clone()));
    if let Some(name_input) = name_input_row(session, palette, emit.clone()) {
        layout = layout.push(name_input);
    }
    layout = layout.push(listing_body(session, palette, emit.clone()));
    layout = layout.push(confirm_footer(session, palette, emit));

    container(layout)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &Theme| page_style(theme))
        .into()
}

fn page_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(palette.background.into()),
        text_color: Some(palette.text),
        ..container::Style::default()
    }
}

fn navigation_bar<'a>(
    session: &'a PickerSession,
    palette: Palette,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Element<'a, SessionMessage> {
    let up_button = button(
        text("上级")
            .size(14)
            .color(palette.text)
            .align_x(alignment::Horizontal::Center),
    )
    .padding(Padding::new(6.0))
    .on_press(emit(SessionMessage::NavigateUp));

    let mut breadcrumb: Row<'a, SessionMessage> = row![].spacing(2);
    for (position, segment) in breadcrumb_chain(session.directory()).iter().enumerate() {
        if position > 0 {
            breadcrumb = breadcrumb.push(
                text("›").size(14).color(translucent(palette.text, 0.5)),
            );
        }
        breadcrumb = breadcrumb.push(
            button(
                text(segment_label(segment))
                    .size(14)
                    .color(palette.primary),
            )
            .padding(Padding::new(4.0))
            .on_press(emit(SessionMessage::BreadcrumbActivated {
                ancestor: position,
            })),
        );
    }

    let mut bar: Row<'a, SessionMessage> = row![].spacing(8);
    bar = bar.push(up_button);
    bar = bar.push(
        scrollable(breadcrumb).direction(scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::default(),
        )),
    );

    if session.filters().len() > 1 {
        let labels: Vec<String> = session
            .filters()
            .iter()
            .map(|rule| rule.name.clone())
            .collect();
        let known_labels = labels.clone();
        let current = Some(session.active_filter_label());
        bar = bar.push(Space::new().width(Length::Fill));
        bar = bar.push(
            pick_list(labels, current, move |picked: String| {
                match known_labels.iter().position(|label| *label == picked) {
                    Some(rule) => emit(SessionMessage::FilterSelected { rule }),
                    None => emit(SessionMessage::FilterSelectionIgnored),
                }
            })
            .padding(Padding::new(4.0)),
        );
    } else {
        bar = bar.push(Space::new().width(Length::Fill));
        bar = bar.push(
            text(session.active_filter_label())
                .size(13)
                .color(translucent(palette.text, 0.6)),
        );
    }

    container(bar).width(Length::Fill).into()
}

fn segment_label(segment: &Path) -> String {
    segment
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

fn listing_body<'a>(
    session: &'a PickerSession,
    palette: Palette,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Element<'a, SessionMessage> {
    let body: Element<'a, SessionMessage> = match session.listing() {
        DirectoryListing::Pending => container(
            text("正在读取目录…")
                .size(14)
                .color(translucent(palette.text, 0.6)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .into(),
        DirectoryListing::Failed(details) => container(
            column![
                text("目录读取失败").size(15).color(palette.danger),
                text(details.clone())
                    .size(13)
                    .color(translucent(palette.text, 0.7)),
            ]
            .spacing(6),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .padding(24)
        .into(),
        DirectoryListing::Ready(_) => {
            if session.visible().is_empty() {
                container(
                    text("空目录")
                        .size(14)
                        .color(translucent(palette.text, 0.6)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(alignment::Horizontal::Center)
                .align_y(alignment::Vertical::Center)
                .into()
            } else {
                let mut list: Column<'a, SessionMessage> = column![].spacing(2);
                for (index, entry) in session.visible().iter().enumerate() {
                    list = list.push(entry_row(
                        index,
                        entry,
                        session.selection().contains(&index),
                        palette,
                        emit.clone(),
                    ));
                }
                scrollable(list).width(Length::Fill).height(Length::Fill).into()
            }
        }
    };
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(translucent(palette.background, 0.4).into()),
                border: Border {
                    color: translucent(palette.text, 0.1),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            }
        })
        .padding(6)
        .into()
}

fn entry_row<'a>(
    index: usize,
    entry: &'a DirectoryEntry,
    selected: bool,
    palette: Palette,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Element<'a, SessionMessage> {
    let badge = entry_badge(entry, palette);

    let meta = match entry.kind {
        FileKind::Directory => "文件夹".to_string(),
        _ => readable_size(entry.metadata.len),
    };

    let mut content: Row<'a, SessionMessage> = row![].spacing(8).padding(Padding::new(4.0));
    content = content.push(badge);
    content = content.push(text(entry.name.to_string_lossy().into_owned()).size(14));
    content = content.push(Space::new().width(Length::Fill));
    content = content.push(text(meta).size(12).color(translucent(palette.text, 0.55)));

    let background = if selected {
        translucent(palette.primary, 0.25)
    } else {
        Color::TRANSPARENT
    };

    mouse_area(
        container(content)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(background.into()),
                border: Border {
                    radius: 6.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
    )
    .on_press(emit(SessionMessage::EntryClicked {
        index,
        ctrl: false,
        shift: false,
    }))
    .on_double_click(emit(SessionMessage::EntryDoubleClicked { index }))
    .into()
}

/// 条目徽标：目录主色，文件用扩展名缩写与次色区分。
fn entry_badge(
    entry: &DirectoryEntry,
    palette: Palette,
) -> Element<'static, SessionMessage> {
    let (initial, tint) = match entry.kind {
        FileKind::Directory => ("D".to_string(), palette.primary),
        _ => {
            let extension = entry
                .path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("F");
            let initial: String = extension
                .chars()
                .take(3)
                .flat_map(char::to_uppercase)
                .collect();
            (initial, palette.success)
        }
    };
    container(text(initial).size(11).color(palette.background))
        .width(Pixels(30.0))
        .height(Pixels(20.0))
        .align_y(alignment::Vertical::Center)
        .align_x(alignment::Horizontal::Center)
        .style(move |_| container::Style {
            background: Some(translucent(tint, 0.75).into()),
            border: Border {
                radius: 5.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

fn readable_size(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    let mut value = bytes as f64;
    let mut unit = "B";
    for candidate in ["KB", "MB", "GB", "TB"] {
        if value < UNIT {
            break;
        }
        value /= UNIT;
        unit = candidate;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn name_input_row<'a>(
    session: &'a PickerSession,
    palette: Palette,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Option<Element<'a, SessionMessage>> {
    if !matches!(session.kind(), PickerKind::SaveFile { .. }) {
        return None;
    }
    let input = text_input("文件名", session.name_input())
        .on_input({
            let emit = emit.clone();
            move |value| emit(SessionMessage::NameInputChanged(value))
        })
        .on_submit(emit(SessionMessage::ConfirmPressed))
        .size(14)
        .padding(Padding::new(6.0));
    Some(
        row![text("文件名").size(13).color(palette.text), input]
            .spacing(8)
            .into(),
    )
}

fn confirm_footer<'a>(
    session: &'a PickerSession,
    palette: Palette,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'a,
) -> Element<'a, SessionMessage> {
    let mut footer: Row<'a, SessionMessage> = row![].spacing(8);

    if let Some(target) = session.overwrite_target() {
        footer = footer.push(
            text(format!(
                "“{}” 已存在，确认覆盖？",
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ))
            .size(13)
            .color(palette.danger),
        );
    }

    footer = footer.push(Space::new().width(Length::Fill));

    if session.overwrite_target().is_some() {
        footer = footer.push(
            button(
                text("换个名字")
                    .size(13)
                    .color(palette.text)
                    .align_x(alignment::Horizontal::Center),
            )
            .padding(Padding::new(5.0))
            .on_press(emit(SessionMessage::OverwriteDeclined)),
        );
    }

    let cancel = button(
        text("取消")
            .size(14)
            .color(palette.text)
            .align_x(alignment::Horizontal::Center),
    )
    .padding(Padding::new(6.0))
    .on_press(emit(SessionMessage::DismissPressed));

    let mut confirm = button(
        text(session.accept_button_label())
            .size(14)
            .color(palette.background)
            .align_x(alignment::Horizontal::Center),
    )
    .padding(Padding::new(6.0));
    if session.can_confirm() {
        confirm = confirm
            .on_press(emit(SessionMessage::ConfirmPressed))
            .style(button::primary);
    }

    footer = footer.push(cancel).push(confirm);

    container(footer).width(Length::Fill).padding(Padding::new(2.0)).into()
}
