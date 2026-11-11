//! 窗口按钮（最小化 / 最大化 / 关闭 / 预览固定）的样式：24px 小圆按钮，平时一层淡底，
//! 悬停加深。关闭按钮不单独变红，与 GNOME 一致。

use iced::widget::button;
use iced::{Background, Border, Theme};

use crate::controls::foreground_tint;
use crate::ui_colors;

/// 窗口按钮直径。
pub const WINDOW_CONTROL_SIZE: f32 = 24.0;

pub fn window_control_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Active | button::Status::Disabled => 0.08,
        button::Status::Hovered => 0.14,
        button::Status::Pressed => 0.2,
    };
    button::Style {
        background: Some(Background::Color(foreground_tint(theme, alpha))),
        text_color: ui_colors(theme).on_surface,
        border: Border {
            radius: (WINDOW_CONTROL_SIZE / 2.0).into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_controls_are_round_and_darken_on_hover() {
        for theme in [Theme::Light, Theme::Dark] {
            let active = window_control_button_style(&theme, button::Status::Active);
            let hovered = window_control_button_style(&theme, button::Status::Hovered);
            assert_eq!(active.border.radius, (WINDOW_CONTROL_SIZE / 2.0).into());
            let (Some(Background::Color(idle)), Some(Background::Color(hover))) =
                (active.background, hovered.background)
            else {
                panic!("window controls always show a tint");
            };
            assert!(hover.a > idle.a);
        }
    }
}
