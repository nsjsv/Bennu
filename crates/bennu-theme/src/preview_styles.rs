//! 预览面板/窗口视觉词汇：媒体底板、浮动预览窗上下渐变遮罩、文档
//! 预览页底板。自主程序 app-ui 原样搬移（值一个都不许漂移），预览
//! 视图层下沉 bennu-preview 后与主程序共用同一份视觉。

use iced::widget::container;
use iced::{Background, Border, Color, Theme};

use crate::styles::subtle_border_color;
use crate::ui_colors;

/// 预览媒体底板：不透明纯黑，让图片/视频内容在暗场中不受主题影响。
pub fn preview_media_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        ..container::Style::default()
    }
}

/// 预览窗顶部渐变遮罩：黑色自上而下淡出，opacity 随 chrome 淡入控制。
pub fn preview_window_top_gradient_style(_theme: &Theme, opacity: f32) -> container::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let gradient = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
        .add_stop(0.0, Color::BLACK.scale_alpha(0.9 * opacity))
        .add_stop(1.0, Color::TRANSPARENT);

    container::Style {
        background: Some(Background::Gradient(gradient.into())),
        ..container::Style::default()
    }
}

/// 预览窗底部渐变遮罩：黑色自下而上淡出，opacity 随控件淡入控制。
pub fn preview_window_bottom_gradient_style(_theme: &Theme, opacity: f32) -> container::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let gradient = iced::gradient::Linear::new(iced::Radians(0.0))
        .add_stop(0.0, Color::BLACK.scale_alpha(0.9 * opacity))
        .add_stop(1.0, Color::TRANSPARENT);

    container::Style {
        background: Some(Background::Gradient(gradient.into())),
        ..container::Style::default()
    }
}

/// 文档预览页底板：最浅 surface 底 + 细边 + 小圆角，模拟纸张。
pub fn document_page_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container_lowest)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_media_surface_is_opaque_black_without_border() {
        for theme in [Theme::Light, Theme::Dark] {
            let style = preview_media_style(&theme);
            assert_eq!(
                style.background,
                Some(Background::Color(Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }))
            );
            assert_eq!(style.border, Border::default());
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
}
