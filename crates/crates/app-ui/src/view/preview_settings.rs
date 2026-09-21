use iced::widget::{button, column, container, row, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::{context_menu_button_style, muted_text_color, subtle_border_color};
use crate::config;
use crate::icons::rotated_chevron_right_view;
use crate::matugen_theme::ui_colors;
use crate::model::{
    Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, SettingsSubpage,
};
use crate::typography::{localized_text, readable_text};

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::network_settings::preview_size_limit_rows;
use super::option_controls::destructive_confirmation_button_style;
use super::settings_group::{
    card_row_button_style, info_setting_row, navigation_setting_row, settings_card,
    SETTINGS_GROUP_SPACING,
};
use super::settings_window::chrome_top_spacer;
use super::{icon_tone_style, themed_icon, IconSymbol, IconTone};

/// 每行标签数量：标签按内容收缩，设置面板宽度下 8 个一行不溢出。
const CHIPS_PER_ROW: usize = 8;

/// 折叠状态下每种类别默认展示的标签数量，余量靠切换标签展开。
const COLLAPSED_CHIPS_PER_KIND: usize = 3;

/// 空格预览分类型后缀规则编辑：每个预览类型一行，标签式增删 + 恢复默认。
pub(super) fn preview_extension_rows(browser: &FileBrowser) -> Element<'_, Message> {
    let mut rows = column![].spacing(3);
    for (kind_index, kind) in config::PreviewFileSizeKind::ALL.iter().enumerate() {
        rows = rows.push(preview_extension_row(browser, kind_index, *kind));
    }
    rows.into()
}

/// Files 分类里的预览设置入口：一行导航条，点击进入预览二级页面。
pub(super) fn preview_settings_entry_card() -> Element<'static, Message> {
    let open = navigation_setting_row("Preview settings")
        .on_press(Message::SettingsSubpageOpened(SettingsSubpage::Preview));
    settings_card(vec![open.into()])
}

/// 预览设置二级页面：返回按钮 + 各类型扩展名与最大预览大小编辑。
pub(super) fn preview_settings_subpage_detail(
    browser: &FileBrowser,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let back = button(
        row![
            themed_icon(IconSymbol::ArrowLeft, IconTone::Normal, 13.0),
            readable_text("Back").size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Message::SettingsSubpageClosed)
    .padding([6, 10])
    .style(context_menu_button_style());

    let content = column![
        chrome_top_spacer(browser),
        back,
        settings_card(vec![preview_extension_rows(browser)]),
        settings_card(vec![preview_size_limit_rows(browser)]),
    ]
    .spacing(SETTINGS_GROUP_SPACING)
    .width(Length::Fill);

    auxiliary_detail_scroller(
        content,
        ScrollbarRegion::Settings,
        scrollbar_visibility,
        scrollbar_viewport,
        Message::SettingsScrolled,
    )
}

fn preview_extension_row(
    browser: &FileBrowser,
    kind_index: usize,
    kind: config::PreviewFileSizeKind,
) -> Element<'_, Message> {
    let label = match kind {
        config::PreviewFileSizeKind::Text => "Text Preview Extensions",
        config::PreviewFileSizeKind::Image => "Image Preview Extensions",
        config::PreviewFileSizeKind::Video => "Video Preview Extensions",
        config::PreviewFileSizeKind::Audio => "Audio Preview Extensions",
        config::PreviewFileSizeKind::Archive => "Archive Preview Extensions",
        config::PreviewFileSizeKind::Document => "Document Preview Extensions",
        config::PreviewFileSizeKind::Sqlite => "SQLite Preview Extensions",
    };
    let expanded = browser.preview_extension_expanded[kind_index];
    let extensions = browser.user_config().preview_extension_rules.list(kind);

    // 折叠态是一整行切换按钮：箭头朝右，只露出前 COLLAPSED_CHIPS_PER_KIND
    // 个只读标签和余量提示；展开态恢复编辑行（输入框、添加、文字重置），
    // 箭头朝下，重置走行内确认。
    let mut content = column![].spacing(4);
    if expanded {
        let toggle = button(
            row![
                rotated_chevron_right_view(90.0, 13.0).style(icon_tone_style(IconTone::Normal)),
                readable_text(label).size(12),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .on_press(Message::PreviewExtensionExpandToggled(kind_index))
        .width(Length::Fill)
        .style(card_row_button_style());
        let mut header = row![toggle].spacing(8).align_y(Alignment::Center);
        // 重置原地二次确认：第一次进入待确认态，第二次真正恢复默认；
        // 折叠该行或改点其他类型即撤销。
        let reset = if browser.preview_extension_reset_confirmation == Some(kind_index) {
            button(container(localized_text("Click Again to Reset").size(12)).padding([6, 10]))
                .on_press(Message::PreviewExtensionResetConfirmed(kind_index))
                .style(destructive_confirmation_button_style())
        } else {
            button(container(readable_text("Reset").size(12)).padding([6, 10]))
                .on_press(Message::PreviewExtensionResetRequested(kind_index))
                .style(context_menu_button_style())
        };
        header = header.push(reset);
        let input = text_input("txt", &browser.preview_extension_inputs[kind_index])
            .on_input(move |value| Message::PreviewExtensionInputChanged(kind_index, value))
            .on_submit(Message::PreviewExtensionInputCommitted(kind_index))
            .padding([6, 8])
            .size(12)
            .width(Length::Fixed(110.0));
        let add = button(container(readable_text("Add").size(12)).padding([6, 10]))
            .on_press(Message::PreviewExtensionInputCommitted(kind_index))
            .style(context_menu_button_style());
        header = header.push(input).push(add);
        content = content.push(header);

        if !extensions.is_empty() {
            content = content.push(chip_flow(kind_index, &extensions));
        }
    } else {
        let visible_count = extensions.len().min(COLLAPSED_CHIPS_PER_KIND);
        let mut collapsed = row![
            rotated_chevron_right_view(0.0, 13.0).style(icon_tone_style(IconTone::Normal)),
            readable_text(label).size(12).width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        for extension in &extensions[..visible_count] {
            collapsed = collapsed.push(extension_chip(extension));
        }
        // 折叠行只作预览，删除只出现在展开区。
        if extensions.len() > COLLAPSED_CHIPS_PER_KIND {
            collapsed = collapsed.push(muted_count_label(
                extensions.len() - COLLAPSED_CHIPS_PER_KIND,
            ));
        }
        content = content.push(
            button(collapsed)
                .on_press(Message::PreviewExtensionExpandToggled(kind_index))
                .width(Length::Fill)
                .style(card_row_button_style()),
        );
    }
    if let Some(error) = &browser.preview_extension_input_errors[kind_index] {
        content = content.push(localized_text(error).size(11).width(Length::Fill));
    }

    info_setting_row(content.into())
}

/// 展开区的完整标签流：每行固定数量手动换行，标签可单个删除。
fn chip_flow(kind_index: usize, extensions: &[String]) -> Element<'_, Message> {
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
    container(readable_text(extension).size(11))
        .padding([2, 6])
        .style(chip_container_style)
        .into()
}

/// 展开区的标签：带删除按钮，可单个移除。
fn removable_extension_chip(kind_index: usize, extension: &str) -> Element<'_, Message> {
    let remove = button(readable_text("×").size(11))
        .padding([2, 6])
        .style(chip_remove_button_style())
        .on_press(Message::PreviewExtensionRemoved(
            kind_index,
            extension.to_owned(),
        ));
    container(
        row![readable_text(extension).size(11), remove]
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
        .size(11)
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
