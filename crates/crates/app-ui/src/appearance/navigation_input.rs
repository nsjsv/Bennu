use iced::widget::{container, text_input};
use iced::Theme;

pub(crate) use bennu_theme::styles::navigation_text_input_style;

pub(crate) fn address_bar_style(theme: &Theme) -> container::Style {
    let input_style = navigation_text_input_style(theme, text_input::Status::Active);
    container::Style {
        background: Some(input_style.background),
        border: input_style.border,
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_inputs_share_the_same_rounded_frame() {
        let statuses = [
            text_input::Status::Active,
            text_input::Status::Hovered,
            text_input::Status::Focused { is_hovered: false },
            text_input::Status::Focused { is_hovered: true },
            text_input::Status::Disabled,
        ];

        for theme in [Theme::Light, Theme::Dark] {
            for status in statuses {
                let input_style = navigation_text_input_style(&theme, status);
                assert_eq!(input_style.border.radius, 8.0.into());
            }

            let input_style = navigation_text_input_style(&theme, text_input::Status::Active);
            let frame_style = address_bar_style(&theme);
            assert_eq!(frame_style.background, Some(input_style.background));
            assert_eq!(frame_style.border, input_style.border);
        }
    }
}
