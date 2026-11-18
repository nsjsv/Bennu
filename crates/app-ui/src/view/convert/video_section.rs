//! 视频卡片：目标可以是另一种视频，也可以只要声音；「更多」随目标变。
//! 截取只在选了一个视频时出现：几段不同的视频共用一组时间没有意义。

use std::fmt;

use iced::widget::row;
use iced::{Alignment, Element, Length};

use file_core::{ConversionEncoderAvailability, VideoRotation, VideoSound, VideoTargetFormat};

use crate::app::convert::ConvertMessage;
use crate::model::{
    ConvertEdit, MediaSection, Message, SectionEdit, VideoClip, VideoConvertOptions, VideoEdit,
    VideoTarget, FPS_CHOICES,
};
use bennu_theme::controls::{segmented_choice_row, text_field, SegmentedChoice};

use super::super::settings_group::{control_row, muted_setting_text, SettingsRow};
use super::media_sections::{
    available_formats, channels_row, channels_summary, format_row, more_rows, quality_rows,
    resize_rows, resize_summary, section_card,
};

const TRIM_FIELD_WIDTH: f32 = 110.0;

impl fmt::Display for VideoTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Video(format) => format.fmt(formatter),
            Self::Audio(format) => formatter.write_str(&tr!("%1 (sound only)", format)),
        }
    }
}

pub(in crate::view) fn video_section<'a>(
    section: &'a MediaSection<VideoConvertOptions>,
    encoders: Option<&ConversionEncoderAvailability>,
) -> Element<'a, Message> {
    let options = &section.options;
    let targets: Vec<VideoTarget> = VideoTarget::all().collect();
    let targets = available_formats(&targets, encoders, |encoders, target| {
        target.is_available(encoders)
    });
    let mut rows = vec![format_row(targets, options.target, |target| {
        edit(VideoEdit::Target(target))
    })];
    rows.extend(quality_rows(options, video_edit));

    let mut summary = Vec::new();
    let mut more = Vec::new();
    match options.target {
        VideoTarget::Video(format) => {
            summary.extend(resize_summary(&options.resize));
            summary.extend(options.fps_override.map(|fps| format!("{fps}fps")));
            summary.extend(rotation_label(options.clip.rotation));
            more.extend(resize_rows(
                &options.resize,
                |choice| edit(VideoEdit::Resize(choice)),
                |text| edit(VideoEdit::LongEdge(text)),
            ));
            more.push(control_row(
                tr!("Frame rate"),
                None,
                segmented_choice_row(fps_choices(options.fps_override)),
            ));
            more.push(rotation_row(options.clip.rotation));
            // GIF 本来没声音。
            if format != VideoTargetFormat::Gif {
                if options.clip.sound == VideoSound::Remove {
                    summary.push(tr!("No sound"));
                }
                more.push(sound_row(options.clip.sound));
            }
        }
        VideoTarget::Audio(_) => {
            summary.extend(channels_summary(options.channels));
            more.push(channels_row(options.channels, |channels| {
                edit(VideoEdit::Channels(channels))
            }));
        }
    }
    if section.sources.paths.len() == 1 {
        summary.extend(trim_summary(&options.clip));
        more.push(trim_row(&options.clip));
    }
    rows.extend(more_rows(section, summary, video_edit, more));
    section_card(tr!("Videos"), section, rows)
}

fn video_edit(edit: SectionEdit<VideoEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Video(edit)))
}

fn edit(edit: VideoEdit) -> Message {
    video_edit(SectionEdit::Options(edit))
}

fn fps_choices(selected: Option<u32>) -> Vec<SegmentedChoice<Message>> {
    std::iter::once((None, tr!("Keep")))
        .chain(
            FPS_CHOICES
                .into_iter()
                .map(|fps| (Some(fps), format!("{fps}fps"))),
        )
        .map(|(fps, label)| SegmentedChoice {
            label,
            selected: selected == fps,
            message: Some(edit(VideoEdit::Fps(fps))),
            tooltip: None,
        })
        .collect()
}

/// 箭头比“顺时针 90°”短，四个按钮放得下；意思放进悬停提示。
fn rotation_label(rotation: VideoRotation) -> Option<String> {
    match rotation {
        VideoRotation::None => None,
        VideoRotation::Clockwise => Some("↻ 90°".to_owned()),
        VideoRotation::HalfTurn => Some("180°".to_owned()),
        VideoRotation::CounterClockwise => Some("↺ 90°".to_owned()),
    }
}

fn rotation_row(selected: VideoRotation) -> SettingsRow<'static> {
    let choices = [
        (VideoRotation::None, None),
        (VideoRotation::Clockwise, Some(tr!("Rotate clockwise"))),
        (VideoRotation::HalfTurn, Some(tr!("Turn upside down"))),
        (
            VideoRotation::CounterClockwise,
            Some(tr!("Rotate counterclockwise")),
        ),
    ]
    .into_iter()
    .map(|(rotation, tooltip)| SegmentedChoice {
        label: rotation_label(rotation).unwrap_or_else(|| tr!("Keep")),
        selected: selected == rotation,
        message: Some(edit(VideoEdit::Rotation(rotation))),
        tooltip,
    })
    .collect();
    control_row(tr!("Rotate"), None, segmented_choice_row(choices))
}

fn sound_row(selected: VideoSound) -> SettingsRow<'static> {
    let choices = [
        (VideoSound::Keep, tr!("Keep")),
        (VideoSound::Remove, tr!("Remove")),
    ]
    .into_iter()
    .map(|(sound, label)| SegmentedChoice {
        label,
        selected: selected == sound,
        message: Some(edit(VideoEdit::Sound(sound))),
        tooltip: None,
    })
    .collect();
    control_row(tr!("Sound"), None, segmented_choice_row(choices))
}

/// 空着的一端写“开头 / 结尾”。
fn trim_summary(clip: &VideoClip) -> Option<String> {
    let start = clip.trim_start_text.trim();
    let end = clip.trim_end_text.trim();
    if start.is_empty() && end.is_empty() {
        return None;
    }
    let or_label = |text: &str, label: String| {
        if text.is_empty() {
            label
        } else {
            text.to_owned()
        }
    };
    Some(format!(
        "{}–{}",
        or_label(start, tr!("Beginning")),
        or_label(end, tr!("End"))
    ))
}

fn trim_row(clip: &VideoClip) -> SettingsRow<'_> {
    control_row(
        tr!("Trim"),
        None,
        row![
            text_field(&tr!("Beginning"), &clip.trim_start_text)
                .on_input(|text| edit(VideoEdit::TrimStart(text)))
                .width(Length::Fixed(TRIM_FIELD_WIDTH)),
            muted_setting_text("–".to_owned()),
            text_field(&tr!("End"), &clip.trim_end_text)
                .on_input(|text| edit(VideoEdit::TrimEnd(text)))
                .width(Length::Fixed(TRIM_FIELD_WIDTH)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
}
