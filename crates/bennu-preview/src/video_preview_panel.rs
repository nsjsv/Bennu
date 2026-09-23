//! 视频预览面板（帧视图/底部控件/音量/迷你进度条）。自 app-ui
//! view/preview_panel.rs 下沉，逐字节保真；控件透明度契约见
//! image-preview-guidelines 第三节。

use std::path::Path;
use std::time::Duration;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, image, row, slider, Space, Stack};
use iced::{Alignment, Element, Length};

use bennu_theme::icons::IconSymbol;
use bennu_theme::preview_styles::{preview_media_style, preview_window_bottom_gradient_style};

use crate::media_controls::{
    faded_media_button_style, faded_media_icon, faded_media_slider_style, faded_media_text_style,
    media_position_text, mini_progress_bar_layer, mini_progress_opacity_for_controls_opacity,
    AUDIO_CONTROL_BUTTON_SIZE, AUDIO_CONTROL_ICON_SIZE, AUDIO_PROGRESS_SLIDER_STEP_SECONDS,
    AUDIO_TIMELINE_CONTROL_GAP, AUDIO_VOLUME_SLIDER_STEP, VIDEO_CONTROL_HORIZONTAL_PADDING,
    VIDEO_PREVIEW_CONTROL_HEIGHT,
};
use crate::panel_text::readable_text;
use crate::preview::VideoPreviewPlayback;
use crate::preview::VideoPreviewPlaybackStatus;
use crate::preview::{scaled_media_size, PreviewSize};
use crate::preview_message::PreviewMessage;

const VIDEO_PROGRESS_SLIDER_PORTION: u16 = 4;
const VIDEO_VOLUME_SLIDER_PORTION: u16 = 1;
const VIDEO_CONTROL_SLIDER_GAP: f32 = 14.0;
const VIDEO_VOLUME_ICON_GAP: f32 = 6.0;

// 参数即视频面板的完整输入（路径 + 帧 + 播放态 + 接线），拆结构体只是搬家
#[allow(clippy::too_many_arguments)]
pub(crate) fn video_preview_panel<Message>(
    path: &Path,
    frame: Option<&image::Handle>,
    width: u32,
    height: u32,
    duration: Option<Duration>,
    playback: Option<&VideoPreviewPlayback>,
    size: PreviewSize,
    preview_bottom_controls_opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let (frame_width, frame_height) = video_frame_size(size, width, height);
    let effective_opacity =
        video_controls_opacity_for_playback(playback, preview_bottom_controls_opacity);
    let frame_content: Element<'static, Message> = if let Some(frame) = frame {
        image::Image::new(frame.clone())
            .width(Length::Fixed(frame_width))
            .height(Length::Fixed(frame_height))
            .into()
    } else {
        container(Space::new().width(Length::Fixed(frame_width)))
            .width(Length::Fixed(frame_width))
            .height(Length::Fixed(frame_height))
            .center_x(Length::Fixed(frame_width))
            .center_y(Length::Fixed(frame_height))
            .into()
    };
    let frame_view: Element<'static, Message> = container(frame_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(preview_media_style)
        .into();

    let mini_progress_opacity = mini_progress_opacity_for_controls_opacity(effective_opacity);
    let mut overlay = Stack::with_children([frame_view])
        .width(Length::Fill)
        .height(Length::Fill);
    if mini_progress_opacity > f32::EPSILON {
        overlay = overlay.push(mini_progress_bar_layer(
            video_progress_fraction(playback, duration),
            mini_progress_opacity,
        ));
    }

    if effective_opacity > f32::EPSILON {
        let gradient: Element<'static, Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| preview_window_bottom_gradient_style(theme, effective_opacity))
            .into();
        let controls: Element<'static, Message> = container(video_controls(
            playback,
            duration,
            frame_width,
            effective_opacity,
        ))
        .width(Length::Fill)
        .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
        .center_x(Length::Fill)
        .center_y(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
        .into();
        let bottom_controls: Element<'static, Message> = container(
            Stack::with_children([gradient, controls])
                .width(Length::Fill)
                .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Bottom)
        .into();
        overlay = overlay.push(bottom_controls);
    }

    overlay.into()
}

/// seek 未提交时控件强制可见，其余跟随底部悬停透明度。
pub(crate) fn video_controls_opacity_for_playback(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> f32 {
    if playback.is_some_and(|playback| playback.seek_completion.is_some()) {
        1.0
    } else {
        opacity.clamp(0.0, 1.0)
    }
}

/// 迷你进度条进度分数：duration 缺失回退 position + 1s（与控件语义一致）。
pub(crate) fn video_progress_fraction(
    playback: Option<&VideoPreviewPlayback>,
    duration: Option<Duration>,
) -> f32 {
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration_seconds = playback
        .and_then(|playback| playback.duration)
        .or(duration)
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| position.as_secs_f32() + 1.0)
        .max(1.0);
    (position.as_secs_f32().min(duration_seconds) / duration_seconds).clamp(0.0, 1.0)
}

fn video_frame_size(size: PreviewSize, width: u32, height: u32) -> (f32, f32) {
    let max_width = size.width.max(1.0);
    let max_height = size.height.max(1.0);
    scaled_media_size(max_width, max_height, width, height)
}

fn video_primary_button<Message>(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> iced::widget::Button<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let icon = match playback.map(|playback| playback.status) {
        Some(VideoPreviewPlaybackStatus::Playing) => IconSymbol::Pause,
        _ => IconSymbol::Play,
    };
    button(faded_media_icon(icon, AUDIO_CONTROL_ICON_SIZE, opacity))
        .on_press(Message::from(PreviewMessage::VideoPreviewPlaybackToggled))
        .padding(8)
        .width(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .height(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .style(move |theme, status| faded_media_button_style(theme, status, opacity))
}

fn video_controls<Message>(
    playback: Option<&VideoPreviewPlayback>,
    duration: Option<Duration>,
    width: f32,
    opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let opacity = opacity.clamp(0.0, 1.0);
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration = playback.and_then(|playback| playback.duration).or(duration);
    let duration_seconds = duration
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| (position.as_secs_f32() + 1.0).max(1.0))
        .max(1.0);
    let position_seconds = position.as_secs_f32().min(duration_seconds);

    let progress_slider = slider(0.0..=duration_seconds, position_seconds, |value: f32| {
        Message::from(PreviewMessage::VideoPreviewSeekRequested(value))
    })
    .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
    .on_release(Message::from(PreviewMessage::VideoPreviewSeekCommitted))
    .width(Length::FillPortion(VIDEO_PROGRESS_SLIDER_PORTION))
    .style(move |theme, status| faded_media_slider_style(theme, status, opacity));
    let slider_row = row![
        progress_slider,
        container(video_volume_control(playback, opacity))
            .width(Length::FillPortion(VIDEO_VOLUME_SLIDER_PORTION)),
    ]
    .spacing(VIDEO_CONTROL_SLIDER_GAP)
    .width(Length::Fill)
    .align_y(Alignment::Center);

    container(
        column![
            row![
                video_primary_button(playback, opacity),
                readable_text(media_position_text(position, duration))
                    .size(12)
                    .style(move |theme| faded_media_text_style(theme, opacity)),
            ]
            .spacing(AUDIO_TIMELINE_CONTROL_GAP)
            .align_y(Alignment::Center),
            slider_row,
        ]
        .spacing(8)
        .width(Length::Fixed(width)),
    )
    .padding([0, VIDEO_CONTROL_HORIZONTAL_PADDING])
    .width(Length::Fixed(width))
    .into()
}

fn video_volume_control<Message>(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    row![
        faded_media_icon(IconSymbol::Volume2, AUDIO_CONTROL_ICON_SIZE, opacity),
        video_volume_slider(playback, opacity).width(Length::Fill),
    ]
    .spacing(VIDEO_VOLUME_ICON_GAP)
    .align_y(Alignment::Center)
    .into()
}

fn video_volume_slider<Message>(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> iced::widget::Slider<'static, f32, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let volume = playback.map(|playback| playback.volume).unwrap_or(1.0);
    slider(0.0..=1.0, volume, |value: f32| {
        Message::from(PreviewMessage::VideoPreviewVolumeChanged(value))
    })
    .step(AUDIO_VOLUME_SLIDER_STEP)
    .style(move |theme, status| faded_media_slider_style(theme, status, opacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::VideoPreviewSeekCompletion;
    use std::path::PathBuf;

    #[test]
    fn seek_keeps_video_controls_visible_until_commit() {
        let mut playback =
            VideoPreviewPlayback::playing(PathBuf::from("clip.mp4"), Some(Duration::from_secs(10)));

        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            0.0
        );

        playback.seek_completion = Some(VideoPreviewSeekCompletion::StayPaused);
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            1.0
        );

        playback.seek_completion = None;
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.25),
            0.25
        );
    }

    #[test]
    fn video_progress_fraction_follows_playback() {
        let mut playback =
            VideoPreviewPlayback::playing(PathBuf::from("clip.mp4"), Some(Duration::from_secs(10)));
        playback.position = Duration::from_secs(4);
        assert_eq!(video_progress_fraction(Some(&playback), None), 0.4);

        // duration 未知时回退语义与 video_controls 一致:按 position + 1s 计算。
        let mut unknown = VideoPreviewPlayback::playing(PathBuf::from("clip.webm"), None);
        unknown.position = Duration::from_secs(5);
        assert_eq!(video_progress_fraction(Some(&unknown), None), 5.0 / 6.0);

        // 播放流未就绪时仅显示轨道。
        assert_eq!(
            video_progress_fraction(None, Some(Duration::from_secs(10))),
            0.0
        );
    }
}
