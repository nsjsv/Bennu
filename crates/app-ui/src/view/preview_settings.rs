use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::{muted_text_color, subtle_border_color};
use crate::config;
use crate::model::{Message, SettingsSubpage};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{destructive_button, normal_button, text_field};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

use super::network_settings::preview_size_limit_rows;
use super::settings_group::{expander_row, navigation_row, settings_card, SettingsRow};

/// 每行标签数量：标签按内容收缩，设置面板宽度下 8 个一行不溢出。
const CHIPS_PER_ROW: usize = 8;

/// 折叠状态下每种类别默认展示的标签数量，余量靠切换标签展开。
const COLLAPSED_CHIPS_PER_KIND: usize = 3;

/// Files 分类里的预览设置入口：一行导航条，点击进入预览二级页面。
pub(super) fn preview_settings_entry_card() -> Element<'static, Message> {
    settings_card(vec![navigation_row(
        tr!("Preview settings"),
        None,
        Message::SettingsSubpageOpened(SettingsSubpage::Preview),
    )])
}

/// 预览设置二级页面：各类型扩展名 + 最大预览大小（后退在顶栏）。
pub(super) fn preview_settings_subpage_sections(
    browser: &FileBrowser,
) -> Vec<Element<'_, Message>> {
    let extension_rows = config::PreviewFileSizeKind::ALL
        .iter()
        .enumerate()
        .map(|(kind_index, kind)| preview_extension_row(browser, kind_index, *kind))
        .collect();

    vec![
        settings_card(extension_rows),
        settings_card(preview_size_limit_rows(browser)),
    ]
}

/// 折叠态只露前 COLLAPSED_CHIPS_PER_KIND 个只读标签和灰色余量；展开态是添加输入、
/// 标签流和重置（行内二次确认）。
fn preview_extension_row(
    browser: &FileBrowser,
    kind_index: usize,
    kind: config::PreviewFileSizeKind,
) -> SettingsRow<'_> {
    let title = match kind {
        config::PreviewFileSizeKind::Text => tr!("Text Preview Extensions"),
        config::PreviewFileSizeKind::Image => tr!("Image Preview Extensions"),
        config::PreviewFileSizeKind::Video => tr!("Video Preview Extensions"),
        config::PreviewFileSizeKind::Audio => tr!("Audio Preview Extensions"),
        config::PreviewFileSizeKind::Archive => tr!("Archive Preview Extensions"),
        config::PreviewFileSizeKind::Document => tr!("Document Preview Extensions"),
        config::PreviewFileSizeKind::Sqlite => tr!("SQLite Preview Extensions"),
    };
    let extensions = browser.user_config().preview_extension_rules.list(kind);
    let toggle = Message::PreviewExtensionExpandToggled(kind_index);

    let visible_count = extensions.len().min(COLLAPSED_CHIPS_PER_KIND);
    let mut summary = row![].spacing(6).align_y(Alignment::Center);
    for extension in &extensions[..visible_count] {
        summary = summary.push(extension_chip(extension));
    }
    if extensions.len() > COLLAPSED_CHIPS_PER_KIND {
        summary = summary.push(muted_count_label(
            extensions.len() - COLLAPSED_CHIPS_PER_KIND,
        ));
    }

    let body = browser.preview_extension_expanded[kind_index]
        .then(|| preview_extension_editor(browser, kind_index, extensions));
    expander_row(title, summary, toggle, body)
}

fn preview_extension_editor<'a>(
    browser: &'a FileBrowser,
    kind_index: usize,
    extensions: &[String],
) -> Element<'a, Message> {
    let input = text_field("txt", &browser.preview_extension_inputs[kind_index])
        .on_input(move |value| Message::PreviewExtensionInputChanged(kind_index, value))
        .on_submit(Message::PreviewExtensionInputCommitted(kind_index))
        .width(Length::Fixed(110.0));
    let add = normal_button(readable_text(tr!("Add")))
        .on_press(Message::PreviewExtensionInputCommitted(kind_index));
    // 重置原地二次确认：第一次进入待确认态，第二次真正恢复默认；
    // 折叠该行或改点其他类型即撤销。
    let reset = if browser.preview_extension_reset_confirmation == Some(kind_index) {
        destructive_button(readable_text(tr!("Click Again to Reset")))
            .on_press(Message::PreviewExtensionResetConfirmed(kind_index))
    } else {
        normal_button(readable_text(tr!("Reset")))
            .on_press(Message::PreviewExtensionResetRequested(kind_index))
    };

    let mut editor = column![row![input, add, Space::new().width(Length::Fill), reset]
        .spacing(8)
        .align_y(Alignment::Center)]
    .spacing(8);
    if !extensions.is_empty() {
        editor = editor.push(chip_flow(kind_index, extensions));
    }
    if let Some(error) = &browser.preview_extension_input_errors[kind_index] {
        editor = editor.push(
            readable_text(error.clone())
                .size(CAPTION_SIZE)
                .width(Length::Fill)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).error),
                }),
        );
    }
    editor.into()
}

/// 展开区的完整标签流：每行固定数量手动换行，标签可单个删除。
fn chip_flow(kind_index: usize, extensions: &[String]) -> Element<'static, Message> {
    // 标签按文字内容收缩，每行固定数量手动换行；Grid 会把单元
    // 拉伸成等宽大方格，不适合紧凑标签。
    extensions
        .chunks(CHIPS_PER_ROW)
        .fold(column![].spacing(4), |rows, chunk| {
            let chips = chunk.iter().fold(row![].spacing(4), |row, extension| {
                row.push(removable_extension_chip(kind_index, extension))
            });
            rows.push(chips)
        })
        .into()
}

fn chip_remove_button_style() -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let colors = ui_colors(theme);
        button::Style {
            background: Some(Background::Color(
                if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    colors.surface_container_high
                } else {
                    Color::TRANSPARENT
                },
            )),
            text_color: muted_text_color(theme),
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// 折叠行的只读标签：仅预览，不带删除。
fn extension_chip(extension: &str) -> Element<'static, Message> {
    container(readable_text(extension.to_owned()).size(CAPTION_SIZE))
        .padding([2, 6])
        .style(chip_container_style)
        .into()
}

/// 展开区的标签：带删除按钮，可单个移除。
fn removable_extension_chip(kind_index: usize, extension: &str) -> Element<'static, Message> {
    let remove = button(readable_text("×".to_owned()).size(CAPTION_SIZE))
        .padding([2, 6])
        .style(chip_remove_button_style())
        .on_press(Message::PreviewExtensionRemoved(
            kind_index,
            extension.to_owned(),
        ));
    container(
        row![
            readable_text(extension.to_owned()).size(CAPTION_SIZE),
            remove
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding([2, 4])
    .style(chip_container_style)
    .into()
}

/// 折叠行的余量提示，纯展示不可点。
fn muted_count_label(hidden_count: usize) -> Element<'static, Message> {
    text(format!("+{hidden_count}"))
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

fn chip_container_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}
