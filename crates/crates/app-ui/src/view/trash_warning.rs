use iced::widget::{button, column, container, row};
use iced::{Alignment, Background, Border, Element, Length, Theme};

use crate::app::FileBrowser;
use crate::appearance::transparent_button_style;
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::localization;
use crate::matugen_theme::ui_colors;
use crate::model::Message;
use crate::typography::{localized_text, readable_text};

use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const MAX_VISIBLE_WARNING_DETAILS: usize = 50;
const WARNING_DETAIL_MAX_CHARS: usize = 140;

fn trash_warning_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.tertiary_container)),
        text_color: Some(colors.on_tertiary_container),
        border: Border {
            color: colors.tertiary,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub(super) fn trash_warning_panel(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let warnings = browser
        .trash_refresh
        .snapshot()
        .map(|snapshot| snapshot.skipped.as_slice())
        .unwrap_or_default();
    let refresh_error = browser.trash_refresh.last_error();
    if warnings.is_empty() && refresh_error.is_none() {
        return None;
    }

    let expanded = browser.trash_refresh.warning_details_expanded();
    let summary_text = refresh_error.map_or_else(
        || localization::trash_warning_summary(warnings.len()),
        localization::trash_refresh_failed,
    );
    let mut summary = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text(summary_text).size(12).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    if !warnings.is_empty() {
        let action_label = if expanded {
            "Hide warning details"
        } else {
            "Show warning details"
        };
        summary = summary.push(
            button(localized_text(action_label).size(11))
                .padding([4, 6])
                .style(transparent_button_style())
                .on_press(Message::TrashWarningsToggled),
        );
    }

    let mut content = column![summary].spacing(5).width(Length::Fill);
    if expanded {
        for warning in warnings.iter().take(MAX_VISIBLE_WARNING_DETAILS) {
            let detail = format!("{}: {}", warning.path.display(), warning.message);
            content = content.push(
                readable_text(format_middle_ellipsized_text(
                    &detail,
                    WARNING_DETAIL_MAX_CHARS,
                ))
                .size(11)
                .width(Length::Fill),
            );
        }
        if warnings.len() > MAX_VISIBLE_WARNING_DETAILS {
            content = content.push(
                readable_text(localization::trash_additional_warning_count(
                    warnings.len() - MAX_VISIBLE_WARNING_DETAILS,
                ))
                .size(11),
            );
        }
    }

    Some(
        container(container(content).padding(10).style(trash_warning_style))
            .padding([8, 18])
            .width(Length::Fill)
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matugen_theme::{parse_matugen_theme, ui_colors};

    #[test]
    fn trash_warning_uses_tertiary_roles_in_both_modes() {
        for document in [
            include_str!("../../../bennu-theme/test-data/matugen-dark.toml"),
            include_str!("../../../bennu-theme/test-data/matugen-light.toml"),
        ] {
            let theme = parse_matugen_theme(document).expect("fixture must be valid");
            let colors = ui_colors(&theme);
            let style = trash_warning_style(&theme);

            assert_eq!(
                style.background,
                Some(Background::Color(colors.tertiary_container))
            );
            assert_eq!(style.text_color, Some(colors.on_tertiary_container));
            assert_eq!(style.border.color, colors.tertiary);
        }
    }
}
