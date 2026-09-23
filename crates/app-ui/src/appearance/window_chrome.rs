use iced::widget::container;
use iced::{Background, Border, Color, Theme};

use super::{base_text_color, subtle_border_color};
use crate::matugen_theme::ui_colors;

pub(crate) fn window_top_bar_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(window_chrome_background(theme))),
        text_color: Some(base_text_color(theme)),
        ..container::Style::default()
    }
}

pub(crate) fn window_title_bar_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(window_chrome_background(theme))),
        text_color: Some(base_text_color(theme)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            ..Border::default()
        },
        ..container::Style::default()
    }
}

fn window_chrome_background(theme: &Theme) -> Color {
    ui_colors(theme).background
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_integrated_top_bars_share_the_window_surface() {
        for theme in [Theme::Light, Theme::Dark] {
            let title_bar = window_title_bar_style(&theme);
            let top_bar = window_top_bar_style(&theme);

            assert_eq!(title_bar.background, top_bar.background);
            assert_eq!(title_bar.text_color, top_bar.text_color);
            assert_eq!(top_bar.border.width, 0.0);
            assert_eq!(title_bar.border.width, 1.0);
        }
    }
}
