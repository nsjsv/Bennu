//! 视频预览面板（帧视图/底部控件/音量/迷你进度条）。自 app-ui
//! view/preview_panel.rs 下沉，逐字节保真；控件透明度契约见
//! image-preview-guidelines 第三节。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, mouse_area, row, Column, Space, Stack};
use iced::{Alignment, Element, Length};

use bennu_theme::anchored_popup::anchored_dropup;
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::preview_styles::{preview_media_style, preview_window_bottom_gradient_style};
use bennu_theme::styles::{context_menu_item_button_style, context_menu_style};

use crate::media_controls::{
    faded_media_button_style, faded_media_icon, faded_media_slider_style, faded_media_text_style,
    media_position_text, mini_progress_bar_layer, mini_progress_opacity_for_controls_opacity,
    AUDIO_CONTROL_BUTTON_SIZE, AUDIO_CONTROL_ICON_SIZE, AUDIO_PROGRESS_SLIDER_STEP_SECONDS,
    AUDIO_TIMELINE_CONTROL_GAP, AUDIO_VOLUME_SLIDER_STEP, VIDEO_CONTROL_HORIZONTAL_PADDING,
    VIDEO_PREVIEW_CONTROL_HEIGHT,
};
use crate::panel_text::readable_text;
use crate::preview::{scaled_media_size, PreviewSize};
use crate::preview::{
    VideoPreviewDetails, VideoPreviewPlayback, VideoPreviewPlaybackStatus, VideoTrackMenu,
};
use crate::preview_message::PreviewMessage;
use crate::video_surface::{video_surface, VideoPicture};
use crate::video_track_text::{audio_track_label, subtitle_label};
use bennu_localization::tr;
use bennu_theme::controls::slider;

const VIDEO_PROGRESS_SLIDER_PORTION: u16 = 4;
const VIDEO_VOLUME_SLIDER_PORTION: u16 = 1;
const VIDEO_CONTROL_SLIDER_GAP: f32 = 14.0;
const VIDEO_VOLUME_ICON_GAP: f32 = 6.0;
const TRACK_MENU_ITEM_PADDING: [u16; 2] = [5, 12];
const TRACK_MENU_CHECK_SIZE: f32 = 14.0;

pub(crate) fn video_preview_panel<Message>(
    path: &Path,
    picture: Option<&Arc<VideoPicture>>,
    details: &VideoPreviewDetails,
    playback: Option<&VideoPreviewPlayback>,
    media_volume: f32,
    size: PreviewSize,
    preview_bottom_controls_opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let duration = details.probe.duration;
    // 控件和画面一样宽；画面还没出来时按整块宽度。
    let frame_width = picture.map_or(size.width, |picture| {
        scaled_media_size(
            size.width.max(1.0),
            size.height.max(1.0),
            picture.width(),
            picture.height(),
        )
        .0
    });
    let effective_opacity =
        video_controls_opacity_for_playback(playback, preview_bottom_controls_opacity);
    let frame_content: Element<'static, Message> = match picture {
        Some(picture) => video_surface(Arc::clone(picture)),
        None => Space::new().width(Length::Fill).height(Length::Fill).into(),
    };
    // 单击播放/暂停、双击全屏：判定交给引擎（要等一会儿看有没有第二下）。全屏时鼠标停一会儿就藏。
    let mut frame_area = mouse_area(
        container(frame_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(preview_media_style),
    )
    .on_press(Message::from(PreviewMessage::VideoPreviewSurfacePressed));
    if playback.is_some_and(|playback| playback.cursor_hidden) {
        frame_area = frame_area.interaction(iced::mouse::Interaction::Hidden);
    }
    let frame_view: Element<'static, Message> = frame_area.into();

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
            details,
            media_volume,
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

/// 拖进度条、菜单开着时控件强制可见，其余跟随底部悬停透明度。
pub(crate) fn video_controls_opacity_for_playback(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> f32 {
    if playback
        .is_some_and(|playback| playback.seek_drag.is_some() || playback.track_menu.is_some())
    {
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
    details: &VideoPreviewDetails,
    media_volume: f32,
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
    let duration = playback
        .and_then(|playback| playback.duration)
        .or(details.probe.duration);
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
        container(video_volume_control(media_volume, opacity))
            .width(Length::FillPortion(VIDEO_VOLUME_SLIDER_PORTION)),
    ]
    .spacing(VIDEO_CONTROL_SLIDER_GAP)
    .width(Length::Fill)
    .align_y(Alignment::Center);

    let mut top_row = row![
        video_primary_button(playback, opacity),
        readable_text(media_position_text(position, duration))
            .style(move |theme| faded_media_text_style(theme, opacity)),
        Space::new().width(Length::Fill),
    ]
    .spacing(AUDIO_TIMELINE_CONTROL_GAP)
    .align_y(Alignment::Center);
    let open_menu = playback.and_then(|playback| playback.track_menu);
    if !details.subtitles.is_empty() {
        let current = playback.and_then(|playback| playback.subtitle.choice());
        let items = std::iter::once((tr!("No Subtitles"), current.is_none(), None))
            .chain(details.subtitles.iter().enumerate().map(|(index, choice)| {
                (subtitle_label(choice), current == Some(index), Some(index))
            }))
            .map(|(label, selected, choice)| {
                (
                    label,
                    selected,
                    Message::from(PreviewMessage::VideoPreviewSubtitleSelected(choice)),
                )
            });
        top_row = top_row.push(track_menu_button(
            IconSymbol::Captions,
            VideoTrackMenu::Subtitles,
            open_menu,
            items.collect(),
            opacity,
        ));
    }
    if details.probe.audio_tracks.len() > 1 {
        let current = playback.map_or(0, |playback| playback.audio_track);
        let items = details
            .probe
            .audio_tracks
            .iter()
            .map(|track| {
                (
                    audio_track_label(track),
                    track.ordinal == current,
                    Message::from(PreviewMessage::VideoPreviewAudioTrackSelected(
                        track.ordinal,
                    )),
                )
            })
            .collect();
        top_row = top_row.push(track_menu_button(
            IconSymbol::AudioLines,
            VideoTrackMenu::Audio,
            open_menu,
            items,
            opacity,
        ));
    }

    container(
        column![top_row, slider_row]
            .spacing(8)
            .width(Length::Fixed(width)),
    )
    .padding([0, VIDEO_CONTROL_HORIZONTAL_PADDING])
    .width(Length::Fixed(width))
    .into()
}

/// 控制条上的字幕/音轨按钮，点了在上方弹出菜单（贴着窗口底边，只能往上开）。
fn track_menu_button<Message>(
    icon: IconSymbol,
    menu: VideoTrackMenu,
    open_menu: Option<VideoTrackMenu>,
    items: Vec<(String, bool, Message)>,
    opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let anchor = button(faded_media_icon(icon, AUDIO_CONTROL_ICON_SIZE, opacity))
        .on_press(Message::from(PreviewMessage::VideoPreviewTrackMenuToggled(
            menu,
        )))
        .padding(8)
        .width(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .height(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .style(move |theme, status| faded_media_button_style(theme, status, opacity));
    let popup = (open_menu == Some(menu)).then(|| {
        let mut list = Column::new().spacing(2);
        for (label, selected, message) in items {
            let check: Element<'static, Message> = if selected {
                themed_icon(IconSymbol::Check, IconTone::Normal, TRACK_MENU_CHECK_SIZE).into()
            } else {
                Space::new()
                    .width(Length::Fixed(TRACK_MENU_CHECK_SIZE))
                    .into()
            };
            list = list.push(
                button(
                    row![check, readable_text(label)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                )
                .padding(TRACK_MENU_ITEM_PADDING)
                .width(Length::Fill)
                .style(context_menu_item_button_style())
                .on_press(message),
            );
        }
        container(list).padding(4).style(context_menu_style).into()
    });
    anchored_dropup(anchor, popup)
}

fn video_volume_control<Message>(media_volume: f32, opacity: f32) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    row![
        faded_media_icon(IconSymbol::Volume2, AUDIO_CONTROL_ICON_SIZE, opacity),
        video_volume_slider(media_volume, opacity).width(Length::Fill),
    ]
    .spacing(VIDEO_VOLUME_ICON_GAP)
    .align_y(Alignment::Center)
    .into()
}

fn video_volume_slider<Message>(
    media_volume: f32,
    opacity: f32,
) -> iced::widget::Slider<'static, f32, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    slider(0.0..=1.0, media_volume, |value: f32| {
        Message::from(PreviewMessage::MediaVolumeChanged(value))
    })
    .step(AUDIO_VOLUME_SLIDER_STEP)
    .style(move |theme, status| faded_media_slider_style(theme, status, opacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::VideoSeekDrag;
    use std::path::PathBuf;

    fn playback(path: &str, duration: Option<Duration>) -> VideoPreviewPlayback {
        VideoPreviewPlayback::ready(PathBuf::from(path), duration, true)
    }

    #[test]
    fn seek_keeps_video_controls_visible_until_commit() {
        let mut playback = playback("clip.mp4", Some(Duration::from_secs(10)));

        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            0.0
        );

        playback.seek_drag = Some(VideoSeekDrag {
            resume_after: false,
        });
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            1.0
        );

        playback.seek_drag = None;
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.25),
            0.25
        );
    }

    #[test]
    fn video_progress_fraction_follows_playback() {
        let mut playback_10s = playback("clip.mp4", Some(Duration::from_secs(10)));
        playback_10s.position = Duration::from_secs(4);
        assert_eq!(video_progress_fraction(Some(&playback_10s), None), 0.4);

        // duration 未知时回退语义与 video_controls 一致:按 position + 1s 计算。
        let mut unknown = playback("clip.webm", None);
        unknown.position = Duration::from_secs(5);
        assert_eq!(video_progress_fraction(Some(&unknown), None), 5.0 / 6.0);

        // 播放流未就绪时仅显示轨道。
        assert_eq!(
            video_progress_fraction(None, Some(Duration::from_secs(10))),
            0.0
        );
    }
}
