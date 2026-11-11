//! 外观 → 配色：每套配色画一个迷你窗口（侧栏 + 内容 + 强调色），一行 4 个。
//! 缩略图的颜色就是换上这套配色后真正的颜色（`FileBrowser::color_scheme_roles`），
//! 不另存一份预览色板。

use iced::widget::{button, column, container, row, svg, Column, Row, Svg};
use iced::{Alignment, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::{ColorSchemePreset, UiColorRoles};

use super::settings_group::{card_row_button_style, custom_row, SettingsRow};
use super::{themed_icon, IconTone};

const SCHEMES_PER_ROW: usize = 4;
const THUMBNAIL_WIDTH: f32 = 96.0;
const THUMBNAIL_HEIGHT: f32 = 64.0;

pub(super) fn color_scheme_row(browser: &FileBrowser) -> SettingsRow<'_> {
    let selected = browser.user_config().color_scheme;
    let mut grid = Column::new().spacing(4).width(Length::Fill);
    for schemes in ColorSchemePreset::ALL.chunks(SCHEMES_PER_ROW) {
        let cells = schemes
            .iter()
            .map(|scheme| scheme_cell(browser, *scheme, *scheme == selected));
        grid = grid.push(
            Row::with_children(cells)
                .spacing(4)
                .align_y(Alignment::Start)
                .width(Length::Fill),
        );
    }
    custom_row(
        column![readable_text(tr!("Color scheme")), grid]
            .spacing(8)
            .width(Length::Fill),
    )
}

fn scheme_cell(
    browser: &FileBrowser,
    scheme: ColorSchemePreset,
    selected: bool,
) -> Element<'static, Message> {
    let mut name = row![readable_text(scheme.display_label())]
        .spacing(4)
        .align_y(Alignment::Center);
    if selected {
        name = name.push(themed_icon(IconSymbol::Check, IconTone::Selected, 13.0));
    }
    let mut content = column![
        thumbnail(&browser.color_scheme_roles(scheme), selected),
        name
    ]
    .spacing(6)
    .align_x(Alignment::Center)
    .width(Length::Fill);
    if scheme == ColorSchemePreset::Matugen && !browser.matugen_colors_available() {
        content = content.push(
            readable_text(tr!("Not generated yet"))
                .size(CAPTION_SIZE)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text_color(theme)),
                }),
        );
    }
    button(container(content).padding([6, 4]))
        .on_press(Message::ColorSchemePresetSelected(scheme))
        .width(Length::FillPortion(1))
        .padding(0)
        .style(card_row_button_style())
        .into()
}

fn thumbnail(roles: &UiColorRoles, selected: bool) -> Svg<'static, Theme> {
    Svg::new(svg::Handle::from_memory(thumbnail_svg(roles, selected)))
        .width(Length::Fixed(THUMBNAIL_WIDTH))
        .height(Length::Fixed(THUMBNAIL_HEIGHT))
}

/// 96×64 的迷你窗口：左边侧栏（选中条 + 两行字），右边内容（标题、一行次要文字、
/// 一张卡片和强调色按钮）。选中时外框换成强调色粗边。
fn thumbnail_svg(roles: &UiColorRoles, selected: bool) -> Vec<u8> {
    let hex = |color| color_hex(color);
    let (frame, frame_width) = if selected {
        (hex(roles.primary), 2.5)
    } else {
        (hex(roles.outline_variant), 1.0)
    };
    format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 96 64\">",
            "<defs><clipPath id=\"window\"><rect width=\"96\" height=\"64\" rx=\"8\"/></clipPath></defs>",
            "<g clip-path=\"url(#window)\">",
            "<rect width=\"96\" height=\"64\" fill=\"{background}\"/>",
            "<rect width=\"30\" height=\"64\" fill=\"{sidebar}\"/>",
            "<rect x=\"30\" width=\"1\" height=\"64\" fill=\"{divider}\"/>",
            "<rect x=\"4\" y=\"9\" width=\"22\" height=\"8\" rx=\"4\" fill=\"{selection}\"/>",
            "<rect x=\"7\" y=\"12\" width=\"12\" height=\"2\" rx=\"1\" fill=\"{text}\"/>",
            "<rect x=\"7\" y=\"23\" width=\"14\" height=\"2\" rx=\"1\" fill=\"{muted}\"/>",
            "<rect x=\"7\" y=\"31\" width=\"10\" height=\"2\" rx=\"1\" fill=\"{muted}\"/>",
            "<rect x=\"38\" y=\"10\" width=\"34\" height=\"3\" rx=\"1.5\" fill=\"{text}\"/>",
            "<rect x=\"38\" y=\"18\" width=\"24\" height=\"2\" rx=\"1\" fill=\"{muted}\"/>",
            "<rect x=\"38\" y=\"28\" width=\"50\" height=\"26\" rx=\"5\" fill=\"{card}\"/>",
            "<rect x=\"43\" y=\"34\" width=\"22\" height=\"2\" rx=\"1\" fill=\"{text}\"/>",
            "<rect x=\"43\" y=\"40\" width=\"16\" height=\"2\" rx=\"1\" fill=\"{muted}\"/>",
            "<rect x=\"68\" y=\"42\" width=\"16\" height=\"8\" rx=\"4\" fill=\"{accent}\"/>",
            "</g>",
            "<rect x=\"{inset}\" y=\"{inset}\" width=\"{frame_size_x}\" height=\"{frame_size_y}\" ",
            "rx=\"8\" fill=\"none\" stroke=\"{frame}\" stroke-width=\"{frame_width}\"/>",
            "</svg>"
        ),
        background = hex(roles.background),
        sidebar = hex(roles.surface_dim),
        divider = hex(roles.outline_variant),
        selection = hex(roles.surface_container_highest),
        text = hex(roles.on_surface),
        muted = hex(roles.on_surface_variant),
        card = hex(roles.surface_container),
        accent = hex(roles.primary),
        frame = frame,
        frame_width = frame_width,
        inset = frame_width / 2.0,
        frame_size_x = 96.0 - frame_width,
        frame_size_y = 64.0 - frame_width,
    )
    .into_bytes()
}

fn color_hex(color: iced::Color) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn thumbnails_are_painted_with_the_colors_the_scheme_really_uses() {
        let (browser, _) = FileBrowser::new(config::default_user_config());
        let roles = browser.color_scheme_roles(ColorSchemePreset::Nord);
        let svg = String::from_utf8(thumbnail_svg(&roles, false)).expect("UTF-8 SVG");

        for role in [
            roles.background,
            roles.surface_dim,
            roles.surface_container,
            roles.primary,
            roles.on_surface,
        ] {
            assert!(svg.contains(&format!("fill=\"{}\"", color_hex(role))));
        }
        let selected = String::from_utf8(thumbnail_svg(&roles, true)).expect("UTF-8 SVG");
        assert!(selected.contains(&format!("stroke=\"{}\"", color_hex(roles.primary))));
    }
}
