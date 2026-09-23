//! 动图（GIF）预览面板：双帧叠加（前一帧+当前帧消除闪烁）、底部 seek
//! 控件与迷你进度条。自 app-ui view/preview_panel.rs 下沉，逐字节保真。

use std::time::Duration;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{column, container, slider, Space, Stack};
use iced::{Alignment, Element, Length};

use bennu_theme::preview_styles::{preview_media_style, preview_window_bottom_gradient_style};

use crate::animated_image_preview::AnimatedImagePreview;
use crate::formatting::format_duration;
use crate::image_preview_panel::preview_image_frame;
use crate::media_controls::{
    faded_media_slider_style, faded_media_text_style, mini_progress_bar_layer,
    mini_progress_opacity_for_controls_opacity, AUDIO_PROGRESS_SLIDER_STEP_SECONDS,
    VIDEO_CONTROL_HORIZONTAL_PADDING, VIDEO_PREVIEW_CONTROL_HEIGHT,
};
use crate::panel_text::readable_text;
use crate::preview::image_preview_size;
use crate::preview::PreviewSize;
use crate::preview_message::PreviewMessage;

const ANIMATED_IMAGE_CONTROL_SIDE_PADDING: f32 = 28.0;
const ANIMATED_IMAGE_MIN_CONTROL_WIDTH: f32 = 220.0;

pub(crate) fn animated_image_preview_panel<Message>(
    preview: &AnimatedImagePreview,
    size: PreviewSize,
    preview_bottom_controls_opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (image_width, image_height) = image_preview_size(size, preview.width(), preview.height());
    let mut frames = Stack::new()
        .width(Length::Fixed(image_width))
        .height(Length::Fixed(image_height));

    if let Some(handle) = preview.previous_frame_handle() {
        frames = frames.push(preview_image_frame(handle, image_width, image_height));
    }

    frames = frames.push(preview_image_frame(
        preview.current_frame_handle(),
        image_width,
        image_height,
    ));

    let frame_view: Element<'static, Message> = container(frames)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(preview_media_style)
        .into();
    let effective_opacity =
        animated_image_controls_opacity_for_preview(preview, preview_bottom_controls_opacity);

    let mini_progress_opacity = mini_progress_opacity_for_controls_opacity(effective_opacity);
    let mut overlay = Stack::with_children([frame_view])
        .width(Length::Fill)
        .height(Length::Fill);
    if mini_progress_opacity > f32::EPSILON {
        if let Some(fraction) = animated_image_progress_fraction(preview) {
            overlay = overlay.push(mini_progress_bar_layer(fraction, mini_progress_opacity));
        }
    }

    if effective_opacity > f32::EPSILON {
        let Some(controls) = animated_image_controls(preview, size, image_width, effective_opacity)
        else {
            return overlay.into();
        };
        let gradient: Element<'static, Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| preview_window_bottom_gradient_style(theme, effective_opacity))
            .into();
        let controls: Element<'static, Message> = container(controls)
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

fn animated_image_controls<Message>(
    preview: &AnimatedImagePreview,
    size: PreviewSize,
    image_width: f32,
    opacity: f32,
) -> Option<Element<'static, Message>>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let duration = preview.playback_duration()?;
    let opacity = opacity.clamp(0.0, 1.0);
    let width = animated_image_control_width(size, image_width);
    let position = preview.playback_position().min(duration);
    let duration_seconds = duration
        .as_secs_f32()
        .max(AUDIO_PROGRESS_SLIDER_STEP_SECONDS);
    let position_seconds = position.as_secs_f32().min(duration_seconds);
    let progress_slider = slider(0.0..=duration_seconds, position_seconds, |value: f32| {
        Message::from(PreviewMessage::AnimatedImageSeekRequested(value))
    })
    .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
    .on_release(Message::from(PreviewMessage::AnimatedImageSeekCommitted))
    .width(Length::Fixed(width))
    .style(move |theme, status| faded_media_slider_style(theme, status, opacity));
    let position_text = readable_text(animated_image_position_text(position, duration))
        .size(12)
        .style(move |theme| faded_media_text_style(theme, opacity));
    let controls = column![position_text, progress_slider]
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fixed(width));

    Some(
        container(controls)
            .padding([0, VIDEO_CONTROL_HORIZONTAL_PADDING])
            .width(Length::Fixed(width))
            .into(),
    )
}

/// seek 未提交时控件强制可见，其余跟随底部悬停透明度。
pub(crate) fn animated_image_controls_opacity_for_preview(
    preview: &AnimatedImagePreview,
    opacity: f32,
) -> f32 {
    if preview.is_seeking() {
        1.0
    } else {
        opacity.clamp(0.0, 1.0)
    }
}

fn animated_image_position_text(position: Duration, duration: Duration) -> String {
    format!(
        "{} / {}",
        format_duration(position),
        format_duration(duration)
    )
}

fn animated_image_control_width(size: PreviewSize, image_width: f32) -> f32 {
    let available_width = (size.width - ANIMATED_IMAGE_CONTROL_SIDE_PADDING * 2.0).max(1.0);
    available_width.min(image_width.max(ANIMATED_IMAGE_MIN_CONTROL_WIDTH))
}

/// 迷你进度条分数；duration 未知时返回 None（整条不渲染）。
pub(crate) fn animated_image_progress_fraction(preview: &AnimatedImagePreview) -> Option<f32> {
    let duration = preview.playback_duration()?;
    let duration_seconds = duration
        .as_secs_f32()
        .max(AUDIO_PROGRESS_SLIDER_STEP_SECONDS);
    let position_seconds = preview
        .playback_position()
        .min(duration)
        .as_secs_f32()
        .min(duration_seconds);
    Some((position_seconds / duration_seconds).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePlayback};
    use std::path::PathBuf;

    fn first_frame() -> AnimatedImageFrame {
        AnimatedImageFrame {
            path: PathBuf::from("animation.gif"),
            generation: 1,
            position: Duration::ZERO,
            delay: Duration::from_millis(20),
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn seeking_keeps_animated_image_controls_visible_until_commit() {
        let mut preview = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            first_frame(),
            1,
            Some(Duration::from_secs(10)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");

        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            0.0
        );
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.25),
            0.25
        );

        preview.seek_to_position(Duration::from_secs(3));
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            1.0
        );

        preview.commit_seek(2);
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            0.0
        );
    }

    #[test]
    fn animated_image_progress_fraction_requires_duration() {
        let timed = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            AnimatedImageFrame {
                position: Duration::from_secs(2),
                ..first_frame()
            },
            1,
            Some(Duration::from_secs(8)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");
        assert_eq!(animated_image_progress_fraction(&timed), Some(0.25));

        let untimed = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            AnimatedImageFrame {
                position: Duration::from_secs(2),
                ..first_frame()
            },
            1,
            None,
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");
        assert_eq!(animated_image_progress_fraction(&untimed), None);
    }
}
