//! 设置「关于」分类页：应用标识（图标 + 名称 + 版本）、许可证说明与开源仓库链接。

use crate::appearance::muted_text_color;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use iced::widget::{column, container, image, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use super::settings_group::{link_row, notice_row, settings_card, settings_group, RowNote};
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};

/// 图标固定尺寸。早期按视口宽度比例缩放的方案被放弃：滚动条隐藏/可见
/// 会改变滚动区实测宽度，图标跟着抖。品牌展示图用固定像素最稳。
const ABOUT_ICON_SIZE: f32 = 140.0;

/// 品牌展示名，不做翻译。
const APP_DISPLAY_NAME: &str = "Bennu";

/// 关于分类页：图标名称版本在卡片外，下面许可证与仓库链接。
pub(super) fn about_settings_sections() -> Vec<Element<'static, Message>> {
    vec![
        app_identity_row(),
        settings_group(
            tr!("License"),
            None,
            vec![notice_row(RowNote::Hint(tr!(
                "Bennu is free software released under the GNU General Public License, version 3 or later."
            )))],
        ),
        settings_card(vec![link_row(
            tr!("GitHub repository"),
            Some(RowNote::Hint(tr!(
                "Report issues or browse the source code in your browser."
            ))),
            Message::AboutRepositoryLinkPressed,
        )]),
    ]
}

fn app_identity_row() -> Element<'static, Message> {
    let icon_size = ABOUT_ICON_SIZE;
    let version = tr!("Version %1", env!("CARGO_PKG_VERSION"));
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
        readable_text(APP_DISPLAY_NAME.to_owned())
            .size(TITLE_SIZE)
            .font(BOLD),
        text(version)
            .size(CAPTION_SIZE)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }),
    ]
    .spacing(6)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}
