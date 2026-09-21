use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme};

use super::{
    base_text_color, button_hover_surface_color, button_pressed_surface_color, subtle_border_color,
};
use crate::matugen_theme::ui_colors;

pub(crate) fn window_top_bar_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(window_chrome_background(theme))),
        text_color: Some(base_text_color(theme)),
        ..container::Style::default()
    }
}

pub(crate) fn preview_window_top_gradient_style(_theme: &Theme, opacity: f32) -> container::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let gradient = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
        .add_stop(0.0, Color::BLACK.scale_alpha(0.9 * opacity))
        .add_stop(1.0, Color::TRANSPARENT);

    container::Style {
        background: Some(Background::Gradient(gradient.into())),
        ..container::Style::default()
    }
}

pub(crate) fn preview_window_bottom_gradient_style(
    _theme: &Theme,
    opacity: f32,
) -> container::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let gradient = iced::gradient::Linear::new(iced::Radians(0.0))
        .add_stop(0.0, Color::BLACK.scale_alpha(0.9 * opacity))
        .add_stop(1.0, Color::TRANSPARENT);

    container::Style {
        background: Some(Background::Gradient(gradient.into())),
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

pub(crate) fn window_control_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(button_hover_surface_color(theme)),
        button::Status::Pressed => Some(button_pressed_surface_color(theme)),
        button::Status::Active | button::Status::Disabled => None,
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: base_text_color(theme),
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub(crate) fn window_close_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_colors(theme);
    let (background, text_color) = match status {
        button::Status::Hovered => (Some(colors.error), colors.on_error),
        button::Status::Pressed => (Some(colors.error_container), colors.on_error_container),
        button::Status::Active | button::Status::Disabled => (None, colors.on_surface),
    };
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
pub(crate) fn floating_window_control_button_style(
    theme: &Theme,
    status: button::Status,
) -> button::Style {
    let colors = ui_colors(theme);
    let background = match status {
        button::Status::Active => colors.surface_container_high,
        button::Status::Hovered => colors.surface_container_highest,
        button::Status::Pressed => colors.surface_container,
        button::Status::Disabled => colors.surface_container_low,
    };
    button::Style {
        background: Some(Background::Color(with_opacity(background, 0.9))),
        text_color: colors.on_surface,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub(crate) fn floating_window_close_button_style(
    theme: &Theme,
    status: button::Status,
) -> button::Style {
    let colors = ui_colors(theme);
    let (background, text_color) = match status {
        button::Status::Active => (colors.surface_container_high, colors.on_surface),
        button::Status::Hovered => (colors.error, colors.on_error),
        button::Status::Pressed => (colors.error_container, colors.on_error_container),
        button::Status::Disabled => (colors.surface_container_low, colors.on_surface),
    };
    button::Style {
        background: Some(Background::Color(with_opacity(background, 0.9))),
        text_color,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn with_opacity(color: Color, opacity: f32) -> Color {
    Color {
        a: opacity,
        ..color
    }
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

    #[test]
    fn preview_window_top_gradient_fades_from_black_to_transparent() {
        for theme in [Theme::Light, Theme::Dark] {
            let style = preview_window_top_gradient_style(&theme, 1.0);
            let Some(Background::Gradient(iced::Gradient::Linear(gradient))) = style.background
            else {
                panic!("expected a linear preview window top gradient");
            };
            let stops: Vec<_> = gradient.stops.into_iter().flatten().collect();

            assert_eq!(stops.len(), 2);
            assert_eq!(stops[0].offset, 0.0);
            assert_eq!(stops[1].offset, 1.0);
            assert_eq!(
                stops[0].color,
                Color {
                    a: 0.9,
                    ..Color::BLACK
                }
            );
            assert_eq!(stops[1].color, Color::TRANSPARENT);
            assert_eq!(gradient.angle, iced::Radians(std::f32::consts::PI));
        }
    }

    #[test]
    fn preview_window_bottom_gradient_fades_from_black_at_bottom_to_transparent_at_top() {
        for theme in [Theme::Light, Theme::Dark] {
            let style = preview_window_bottom_gradient_style(&theme, 1.0);
            let Some(Background::Gradient(iced::Gradient::Linear(gradient))) = style.background
            else {
                panic!("expected a linear preview window bottom gradient");
            };
            let stops: Vec<_> = gradient.stops.into_iter().flatten().collect();

            assert_eq!(stops[0].offset, 0.0);
            assert_eq!(
                stops[0].color,
                Color {
                    a: 0.9,
                    ..Color::BLACK
                }
            );
            assert_eq!(stops[1].offset, 1.0);
            assert_eq!(stops[1].color, Color::TRANSPARENT);
            assert_eq!(gradient.angle, iced::Radians(0.0));

            let transparent = preview_window_bottom_gradient_style(&theme, 0.0);
            let Some(Background::Gradient(iced::Gradient::Linear(gradient))) =
                transparent.background
            else {
                panic!("expected a transparent bottom gradient");
            };
            assert!(gradient
                .stops
                .into_iter()
                .flatten()
                .all(|stop| stop.color.a == 0.0));
        }
    }

    #[test]
    fn close_button_uses_danger_feedback_only_while_interacting() {
        for theme in [Theme::Light, Theme::Dark] {
            let active = window_close_button_style(&theme, button::Status::Active);
            let hovered = window_close_button_style(&theme, button::Status::Hovered);
            let pressed = window_close_button_style(&theme, button::Status::Pressed);

            assert!(active.background.is_none());
            assert!(hovered.background.is_some());
            assert!(pressed.background.is_some());
            assert_eq!(hovered.text_color, ui_colors(&theme).on_error);
        }
    }

    #[test]
    fn ordinary_control_has_no_idle_background() {
        for theme in [Theme::Light, Theme::Dark] {
            let active = window_control_button_style(&theme, button::Status::Active);
            let hovered = window_control_button_style(&theme, button::Status::Hovered);

            assert!(active.background.is_none());
            assert!(hovered.background.is_some());
        }
    }
    #[test]
    fn floating_controls_keep_surface_feedback_and_close_danger_state() {
        for theme in [Theme::Light, Theme::Dark] {
            let active = floating_window_control_button_style(&theme, button::Status::Active);
            let hovered = floating_window_control_button_style(&theme, button::Status::Hovered);
            let pressed = floating_window_control_button_style(&theme, button::Status::Pressed);
            assert!(active.background.is_some());
            assert!(hovered.background.is_some());
            assert!(pressed.background.is_some());
            assert_eq!(active.border.width, 0.0);

            let close = floating_window_close_button_style(&theme, button::Status::Hovered);
            assert_eq!(
                close.background,
                Some(Background::Color(with_opacity(
                    ui_colors(&theme).error,
                    0.9,
                )))
            );
            assert_eq!(close.text_color, ui_colors(&theme).on_error);
        }
    }
}
