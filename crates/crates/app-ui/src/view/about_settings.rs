//! 设置「关于」分类页：应用标识（图标 + 名称 + 版本）、许可证说明与开源仓库链接。

use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::model::{Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility};
use crate::typography::readable_text;
use iced::widget::{button, column, container, image, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::settings_group::{
    card_row_button_style, info_setting_row, muted_setting_text, settings_card, settings_group,
    SETTINGS_GROUP_SPACING,
};
use super::settings_window::chrome_top_spacer;
use super::{themed_icon, IconSymbol, IconTone};

/// 图标固定尺寸。早期按视口宽度比例缩放的方案被放弃：滚动条隐藏/可见
/// 会改变滚动区实测宽度，图标跟着抖。品牌展示图用固定像素最稳。
const ABOUT_ICON_SIZE: f32 = 140.0;

/// 品牌展示名，不做翻译。
const APP_DISPLAY_NAME: &str = "Bennu";

const LICENSE_DESCRIPTION: &str =
    "Bennu is free software released under the GNU General Public License, version 3 or later.";
const REPOSITORY_ROW_TITLE: &str = "GitHub repository";
const REPOSITORY_ROW_DESCRIPTION: &str = "Report issues or browse the source code in your browser.";

/// 关于分类页详情：三张卡片从上到下，滚动行为与其他分类页一致。
pub(super) fn about_settings_detail(
    browser: &FileBrowser,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let content = column![
        chrome_top_spacer(browser),
        app_identity_row(),
        settings_group(
            "License",
            vec![info_setting_row(muted_setting_text(
                LICENSE_DESCRIPTION,
                11
            ))],
        ),
        settings_card(vec![repository_link_row()]),
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

fn app_identity_row() -> Element<'static, Message> {
    let icon_size = ABOUT_ICON_SIZE;
    let version = format!(
        "{} {}",
        crate::localization::translate_current("Version"),
        env!("CARGO_PKG_VERSION")
    );
    // 草图复刻：图标、名称、版本竖排居中。图标位图四角透明，用同色白底
    // 容器垫平（底色取自 512 源的贴片底色），圆角比例与源图一致。
    column![
        container(
            image::Image::new(crate::app_icon::display_icon_handle())
                .width(Length::Fixed(icon_size))
                .height(Length::Fixed(icon_size))
        )
        .width(Length::Fixed(icon_size))
        .height(Length::Fixed(icon_size))
        .style(move |_theme| iced::widget::container::Style {
            background: Some(Background::Color(Color::WHITE)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: iced::border::radius(icon_size * 0.22),
            },
            ..iced::widget::container::Style::default()
        }),
        readable_text(APP_DISPLAY_NAME).size(15),
        text(version)
            .size(11)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }),
    ]
    .spacing(6)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn repository_link_row() -> Element<'static, Message> {
    let labels = column![
        readable_text(REPOSITORY_ROW_TITLE)
            .size(12)
            .width(Length::Fill),
        muted_setting_text(REPOSITORY_ROW_DESCRIPTION, 11),
    ]
    .spacing(2)
    .width(Length::Fill);
    let content = row![
        labels,
        themed_icon(IconSymbol::Link, IconTone::Normal, 13.0)
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    button(content)
        .on_press(Message::AboutRepositoryLinkPressed)
        .padding([8, 12])
        .width(Length::Fill)
        .style(card_row_button_style())
        .into()
}
