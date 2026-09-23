//! 窗口控制按钮的纯视觉样式（标准/浮动两套呈现），自 app-ui
//! appearance/window_chrome.rs 下沉；app-ui 侧 re-export 保持调用路径。
//! 顶栏/标题栏容器样式（window_top_bar_style/window_title_bar_style）
//! 只被宿主辅助窗口 chrome 消费，留 app-ui。

use iced::widget::button;
use iced::{Background, Border, Color, Theme};

use crate::styles::{base_text_color, button_hover_surface_color, button_pressed_surface_color};
use crate::ui_colors;

pub fn window_control_button_style(theme: &Theme, status: button::Status) -> button::Style {
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

pub fn window_close_button_style(theme: &Theme, status: button::Status) -> button::Style {
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

pub fn floating_window_control_button_style(
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

pub fn floating_window_close_button_style(theme: &Theme, status: button::Status) -> button::Style {
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
