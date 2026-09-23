//! 音视频/动图预览共用的底部控制件词汇：共享尺寸常量、迷你进度条
//! （交叉淡化契约见 image-preview-guidelines 第三节）、淡出滑杆/图标
//! 样式与播放位置文案。自 app-ui view/preview_panel.rs 下沉，逐字节保真。

use std::time::Duration;

use iced::widget::{container, progress_bar};
use iced::{alignment::Vertical, Background, Border, Element, Length, Theme};

use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::{base_text_color, navigation_icon_button_style};
use bennu_theme::ui_colors;

use crate::formatting::format_duration;

pub(crate) const AUDIO_CONTROL_BUTTON_SIZE: f32 = 30.0;
pub(crate) const AUDIO_CONTROL_ICON_SIZE: f32 = 14.0;
pub(crate) const AUDIO_TIMELINE_CONTROL_GAP: f32 = 10.0;
pub(crate) const AUDIO_PROGRESS_SLIDER_STEP_SECONDS: f32 = 0.05;
pub(crate) const AUDIO_VOLUME_SLIDER_STEP: f32 = 0.01;
pub(crate) const VIDEO_PREVIEW_CONTROL_HEIGHT: f32 = 88.0;
pub(crate) const VIDEO_CONTROL_HORIZONTAL_PADDING: u16 = 16;
const MINI_PROGRESS_BAR_HEIGHT: f32 = 3.0;

/// 迷你进度条透明度 = 1 - 控件栏有效透明度（交叉淡化，纯函数可单测）。
pub(crate) fn mini_progress_opacity_for_controls_opacity(controls_opacity: f32) -> f32 {
    (1.0 - controls_opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// 迷你进度条层：全宽、高 3px、贴窗口最底边的纯指示条（不可交互）。
pub(crate) fn mini_progress_bar_layer<Message: 'static>(
    fraction: f32,
    opacity: f32,
) -> Element<'static, Message> {
    container(
        progress_bar(0.0..=1.0, fraction)
            .girth(Length::Fixed(MINI_PROGRESS_BAR_HEIGHT))
            .style(move |theme| mini_progress_bar_style(theme, opacity)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Vertical::Bottom)
    .into()
}

fn mini_progress_bar_style(theme: &Theme, opacity: f32) -> progress_bar::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let colors = ui_colors(theme);
    progress_bar::Style {
        background: Background::Color(colors.outline_variant.scale_alpha(opacity)),
        bar: Background::Color(colors.primary.scale_alpha(opacity)),
        border: Border::default(),
    }
}

/// 播放位置文案 `position / duration`；duration 未知时只显示 position。
pub(crate) fn media_position_text(position: Duration, duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => format!(
            "{} / {}",
            format_duration(position),
            format_duration(duration)
        ),
        None => format_duration(position),
    }
}

/// 随控件透明度淡出的图标（音视频底部控件共用）。
pub(crate) fn faded_media_icon<Message: 'static>(
    symbol: IconSymbol,
    size: f32,
    opacity: f32,
) -> Element<'static, Message> {
    themed_icon(symbol, IconTone::Normal, size)
        .opacity(opacity.clamp(0.0, 1.0))
        .into()
}

/// 随控件透明度淡出的按钮样式（基于导航图标按钮样式缩放透明度）。
pub(crate) fn faded_media_button_style(
    theme: &Theme,
    status: iced::widget::button::Status,
    opacity: f32,
) -> iced::widget::button::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let mut style = navigation_icon_button_style()(theme, status);
    style.background = style
        .background
        .map(|background| background.scale_alpha(opacity));
    style.text_color = style.text_color.scale_alpha(opacity);
    style.border.color = style.border.color.scale_alpha(opacity);
    style
}

/// 随控件透明度淡出的滑杆样式（缩放轨道/滑块全部颜色）。
pub(crate) fn faded_media_slider_style(
    theme: &Theme,
    status: iced::widget::slider::Status,
    opacity: f32,
) -> iced::widget::slider::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let mut style = iced::widget::slider::default(theme, status);
    style.rail.backgrounds = (
        style.rail.backgrounds.0.scale_alpha(opacity),
        style.rail.backgrounds.1.scale_alpha(opacity),
    );
    style.rail.border.color = style.rail.border.color.scale_alpha(opacity);
    style.handle.background = style.handle.background.scale_alpha(opacity);
    style.handle.border_color = style.handle.border_color.scale_alpha(opacity);
    style
}

/// 底部控件的时间标签样式（随控件透明度淡出）。
pub(crate) fn faded_media_text_style(theme: &Theme, opacity: f32) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(base_text_color(theme).scale_alpha(opacity)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_progress_bar_opacity_inverts_controls_opacity() {
        assert_eq!(mini_progress_opacity_for_controls_opacity(0.0), 1.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(0.25), 0.75);
        assert_eq!(mini_progress_opacity_for_controls_opacity(1.0), 0.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(1.5), 0.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(-0.5), 1.0);
    }
}
