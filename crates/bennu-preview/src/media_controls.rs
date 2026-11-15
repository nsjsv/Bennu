//! 音视频/动图预览共用的底部控制件词汇：共享尺寸常量、迷你进度条
//! （交叉淡化契约见 image-preview-guidelines 第三节）、淡出滑杆/图标
//! 样式与播放位置文案。自 app-ui view/preview_panel.rs 下沉，逐字节保真。

use std::time::Duration;

use iced::widget::container;
use iced::{alignment::Vertical, Background, Border, Element, Length, Theme};

use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::base_text_color;
use bennu_theme::ui_colors;

use crate::formatting::format_duration;
use crate::preview_config::MediaEndAction;
use bennu_theme::controls::progress_bar;

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

fn mini_progress_bar_style(theme: &Theme, opacity: f32) -> iced::widget::progress_bar::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let colors = ui_colors(theme);
    iced::widget::progress_bar::Style {
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
    let mut style = bennu_theme::controls::icon_button_style(theme, status);
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

const FLOATING_BUTTON_SIZE: f32 = 28.0;
const FLOATING_ICON_SIZE: f32 = 16.0;
const FLOATING_BAR_BOTTOM_MARGIN: f32 = 14.0;
const FLOATING_BAR_RADIUS: f32 = 18.0;
const FLOATING_BAR_BACKGROUND_ALPHA: f32 = 0.9;

/// 浮在内容底部正中的一条控件（图片缩放、文档缩放翻页共用）：跟主题配色，深浅主题、
/// 内容明暗下都看得清；整层铺满，只有控件那一条接事件。
pub(crate) fn floating_bar<'a, Message: 'a>(
    controls: Element<'a, Message>,
    opacity: f32,
) -> Element<'a, Message> {
    let opacity = opacity.clamp(0.0, 1.0);
    let bar = container(controls)
        .padding([4, 8])
        .style(move |theme| floating_bar_style(theme, opacity));
    container(bar)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(Vertical::Bottom)
        .padding(iced::Padding {
            bottom: FLOATING_BAR_BOTTOM_MARGIN,
            ..iced::Padding::ZERO
        })
        .into()
}

fn floating_bar_style(theme: &Theme, opacity: f32) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(
            colors
                .surface_container_high
                .scale_alpha(FLOATING_BAR_BACKGROUND_ALPHA * opacity)
                .into(),
        ),
        border: Border {
            color: colors.outline_variant.scale_alpha(opacity),
            width: 1.0,
            radius: FLOATING_BAR_RADIUS.into(),
        },
        shadow: iced::Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25 * opacity),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

pub(crate) fn floating_icon_button<Message: Clone + 'static>(
    symbol: IconSymbol,
    message: Message,
    opacity: f32,
) -> iced::widget::Button<'static, Message> {
    iced::widget::button(faded_media_icon(symbol, FLOATING_ICON_SIZE, opacity))
        .on_press(message)
        .padding(6)
        .width(Length::Fixed(FLOATING_BUTTON_SIZE))
        .height(Length::Fixed(FLOATING_BUTTON_SIZE))
        .style(move |theme, status| faded_media_button_style(theme, status, opacity))
}

pub(crate) fn floating_text_button<Message: Clone + 'static>(
    label: String,
    message: Message,
    opacity: f32,
) -> Element<'static, Message> {
    iced::widget::button(
        crate::panel_text::readable_text(label)
            .style(move |theme| faded_media_text_style(theme, opacity)),
    )
    .on_press(message)
    .padding([4, 8])
    .height(Length::Fixed(FLOATING_BUTTON_SIZE))
    .style(move |theme, status| faded_media_button_style(theme, status, opacity))
    .into()
}

/// 浮条上居中的一段字（百分比、页数），宽度固定，数字变了不跳。
pub(crate) fn floating_bar_text<Message: 'static>(
    content: String,
    width: f32,
    opacity: f32,
) -> Element<'static, Message> {
    iced::widget::text(content)
        .width(Length::Fixed(width))
        .align_x(iced::alignment::Horizontal::Center)
        .style(move |theme| faded_media_text_style(theme, opacity))
        .into()
}

/// 播完模式按钮的图标（音频、视频共用一个设置）：放完停（→|）、单个循环、接着放下一个。
pub(crate) fn end_action_icon(action: MediaEndAction) -> IconSymbol {
    match action {
        MediaEndAction::Stop => IconSymbol::StopAfterTrack,
        MediaEndAction::Repeat => IconSymbol::Repeat1,
        MediaEndAction::Next => IconSymbol::SkipForward,
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
