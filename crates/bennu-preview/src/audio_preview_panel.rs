//! 音频预览面板（标题摘要/时间线控制/音量控制）。自 app-ui
//! view/preview_panel.rs 下沉，逐字节保真。

use std::path::Path;
use std::time::Duration;

use iced::widget::{button, column, container, row, slider, Button, Column, Space};
use iced::{Alignment, Element, Length};

use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::navigation_icon_button_style;

use crate::formatting::{format_duration, format_file_size};
use crate::media_controls::{
    media_position_text, AUDIO_CONTROL_BUTTON_SIZE, AUDIO_CONTROL_ICON_SIZE,
    AUDIO_PROGRESS_SLIDER_STEP_SECONDS, AUDIO_TIMELINE_CONTROL_GAP, AUDIO_VOLUME_SLIDER_STEP,
};
use crate::panel_text::{localized_text, readable_text};
use crate::preview::{AudioPreviewPlayback, AudioPreviewPlaybackStatus};
use crate::preview_message::PreviewMessage;

const AUDIO_PREVIEW_CONTROL_HEIGHT: f32 = 92.0;
const AUDIO_PROGRESS_SLIDER_WIDTH: f32 = 280.0;
const AUDIO_VOLUME_SLIDER_WIDTH: f32 = 100.0;

pub(crate) fn audio_preview_panel<Message>(
    path: &Path,
    duration: Option<Duration>,
    len: u64,
    playback: Option<&AudioPreviewPlayback>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let title = column![
        readable_text("Audio preview").size(17),
        localized_text(audio_preview_summary(duration, len)).size(12),
        localized_text(audio_preview_status(playback, duration)).size(12),
    ]
    .spacing(4)
    .width(Length::Fixed(150.0));

    let controls = row![
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
        .style(navigation_icon_button_style());
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
            readable_text(media_position_text(position, duration)).size(12),
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
        localized_text(format!("Volume {:.0}%", volume * 100.0)).size(12),
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

fn audio_preview_summary(duration: Option<Duration>, len: u64) -> String {
    match duration {
        Some(duration) => format!("{} · {}", format_duration(duration), format_file_size(len)),
        None => format!("Duration unknown · {}", format_file_size(len)),
    }
}

fn audio_preview_status(
    playback: Option<&AudioPreviewPlayback>,
    duration: Option<Duration>,
) -> String {
    use AudioPreviewPlaybackStatus as Status;
    let Some(playback) = playback else {
        return "Ready to play".to_owned();
    };

    match playback.status {
        Status::Loading => "Opening audio output...".to_owned(),
        Status::Playing => format!(
            "Playing · {}",
            media_position_text(playback.position, duration)
        ),
        Status::Paused => format!(
            "Paused · {}",
            media_position_text(playback.position, duration)
        ),
        Status::Stopped => "Stopped".to_owned(),
        Status::Finished => "Finished".to_owned(),
        Status::Error => playback
            .error
            .as_ref()
            .map(|error| format!("Audio unavailable: {error}"))
            .unwrap_or_else(|| "Could not start audio preview".to_owned()),
    }
}
