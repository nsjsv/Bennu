//! 音频预览面板（标题摘要/时间线控制/音量控制）。自 app-ui
//! view/preview_panel.rs 下沉，逐字节保真。

use std::path::Path;
use std::time::Duration;

use bennu_localization::tr;
use iced::widget::{button, column, container, image, row, Button, Column, Space};
use iced::{Alignment, Element, Length};

use bennu_theme::controls::icon_button_style;
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};

use crate::audio_preview::AudioStreamFacts;
use crate::audio_track_text::{stream_summary, track_subtitle, track_title};
use crate::formatting::format_file_size;
use crate::media_controls::{
    media_position_text, AUDIO_CONTROL_BUTTON_SIZE, AUDIO_CONTROL_ICON_SIZE,
    AUDIO_PROGRESS_SLIDER_STEP_SECONDS, AUDIO_TIMELINE_CONTROL_GAP, AUDIO_VOLUME_SLIDER_STEP,
};
use crate::panel_text::readable_text;
use crate::preview::{AudioPreviewDetails, AudioPreviewPlayback, AudioPreviewPlaybackStatus};
use crate::preview_message::PreviewMessage;
use bennu_theme::controls::slider;
use bennu_theme::typography::{BOLD, TITLE_SIZE};

const AUDIO_PREVIEW_CONTROL_HEIGHT: f32 = 92.0;
const AUDIO_PROGRESS_SLIDER_WIDTH: f32 = 280.0;
const AUDIO_VOLUME_SLIDER_WIDTH: f32 = 100.0;

pub(crate) fn audio_preview_panel<Message>(
    path: &Path,
    details: &AudioPreviewDetails,
    cover: Option<&image::Handle>,
    playback: Option<&AudioPreviewPlayback>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let duration = details.probe.duration;
    let len = details.len;
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let mut title = column![readable_text(track_title(path, &details.probe.tags))
        .size(TITLE_SIZE)
        .font(BOLD)]
    .spacing(4)
    .width(Length::Fill);
    if let Some(subtitle) = track_subtitle(&details.probe.tags) {
        title = title.push(readable_text(subtitle));
    }
    title = title
        .push(readable_text(audio_file_summary(
            &details.probe.stream,
            len,
        )))
        .push(readable_text(audio_preview_status(playback, duration)));

    let controls = row![
        audio_cover(cover),
        title,
        audio_timeline_control(playback, duration),
        audio_volume_control(playback),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    column![container(controls)
        .width(Length::Fill)
        .height(Length::Fixed(AUDIO_PREVIEW_CONTROL_HEIGHT))
        .center_y(Length::Fixed(AUDIO_PREVIEW_CONTROL_HEIGHT)),]
}

fn audio_primary_button<Message>(
    playback: Option<&AudioPreviewPlayback>,
) -> Button<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let icon = match playback.map(|playback| playback.status) {
        Some(AudioPreviewPlaybackStatus::Playing) => IconSymbol::Pause,
        _ => IconSymbol::Play,
    };
    let button = button(themed_icon(icon, IconTone::Normal, AUDIO_CONTROL_ICON_SIZE))
        .padding(8)
        .width(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .height(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .style(icon_button_style);
    if matches!(
        playback.map(|playback| playback.status),
        Some(AudioPreviewPlaybackStatus::Loading)
    ) {
        button
    } else {
        button.on_press(Message::from(PreviewMessage::AudioPreviewPlaybackToggled))
    }
}

fn audio_timeline_control<Message>(
    playback: Option<&AudioPreviewPlayback>,
    duration: Option<Duration>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration_seconds = duration
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| (position.as_secs_f32() + 1.0).max(1.0))
        .max(1.0);
    let position_seconds = position.as_secs_f32().min(duration_seconds);

    let slider_row = row![
        audio_primary_button(playback),
        slider(0.0..=duration_seconds, position_seconds, |value: f32| {
            Message::from(PreviewMessage::AudioPreviewSeekRequested(value))
        },)
        .on_release(Message::from(PreviewMessage::AudioPreviewSeekCommitted))
        .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
        .width(Length::Fixed(AUDIO_PROGRESS_SLIDER_WIDTH)),
    ]
    .spacing(AUDIO_TIMELINE_CONTROL_GAP)
    .align_y(Alignment::Center);
    let label_offset = AUDIO_CONTROL_BUTTON_SIZE + AUDIO_TIMELINE_CONTROL_GAP;

    column![
        slider_row,
        row![
            Space::new().width(Length::Fixed(label_offset)),
            readable_text(media_position_text(position, duration)),
        ],
    ]
    .spacing(4)
    .width(Length::Fixed(label_offset + AUDIO_PROGRESS_SLIDER_WIDTH))
    .into()
}

fn audio_volume_control<Message>(
    playback: Option<&AudioPreviewPlayback>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let volume = playback.map(|playback| playback.volume).unwrap_or(1.0);
    column![
        readable_text(tr!("Volume %1%", format!("{:.0}", volume * 100.0))),
        slider(0.0..=1.0, volume, |value: f32| Message::from(
            PreviewMessage::AudioPreviewVolumeChanged(value)
        ),)
        .step(AUDIO_VOLUME_SLIDER_STEP)
        .width(Length::Fixed(AUDIO_VOLUME_SLIDER_WIDTH)),
    ]
    .spacing(4)
    .width(Length::Fixed(AUDIO_VOLUME_SLIDER_WIDTH))
    .into()
}

const AUDIO_COVER_SIZE: f32 = 72.0;

fn audio_cover<Message: 'static>(cover: Option<&image::Handle>) -> Element<'static, Message> {
    let artwork: Element<'static, Message> = match cover {
        Some(handle) => image(handle.clone())
            .content_fit(iced::ContentFit::Cover)
            .width(Length::Fixed(AUDIO_COVER_SIZE))
            .height(Length::Fixed(AUDIO_COVER_SIZE))
            .into(),
        None => themed_icon(IconSymbol::Music, IconTone::Normal, AUDIO_COVER_SIZE * 0.5).into(),
    };
    container(artwork)
        .width(Length::Fixed(AUDIO_COVER_SIZE))
        .height(Length::Fixed(AUDIO_COVER_SIZE))
        .center_x(Length::Fixed(AUDIO_COVER_SIZE))
        .center_y(Length::Fixed(AUDIO_COVER_SIZE))
        .into()
}

fn audio_file_summary(stream: &AudioStreamFacts, len: u64) -> String {
    let facts = stream_summary(stream);
    if facts.is_empty() {
        format_file_size(len)
    } else {
        format!("{facts} · {}", format_file_size(len))
    }
}

fn audio_preview_status(
    playback: Option<&AudioPreviewPlayback>,
    duration: Option<Duration>,
) -> String {
    use AudioPreviewPlaybackStatus as Status;
    let Some(playback) = playback else {
        return tr!("Ready to play");
    };

    match playback.status {
        Status::Loading => tr!("Opening audio output..."),
        Status::Playing => tr!(
            "Playing · %1",
            media_position_text(playback.position, duration)
        ),
        Status::Paused => tr!(
            "Paused · %1",
            media_position_text(playback.position, duration)
        ),
        Status::Stopped => tr!("Stopped"),
        Status::Finished => tr!("Finished"),
        Status::Error => playback
            .error
            .as_ref()
            .map(|error| tr!("Audio unavailable: %1", error))
            .unwrap_or_else(|| tr!("Could not start audio preview")),
    }
}
