//! 三类媒体的卡片：格式、质量、「更多」。质量行和「更多」三类共用，
//! 各类只给自己的格式表和「更多」里的项。

use iced::widget::row;
use iced::{Alignment, Element, Length};

use file_core::{AudioChannelSpec, ConversionEncoderAvailability, ConversionQualityPreset};

use crate::app::convert::ConvertMessage;
use crate::model::{
    AudioConvertOptions, AudioEdit, ConvertEdit, ImageConvertOptions, ImageEdit, MediaOptions,
    MediaSection, Message, QualityChoice, QualityControl, ResizeChoice, ResizeSelection,
    SectionEdit, VideoConvertOptions, VideoEdit, AUDIO_TARGET_FORMATS, FPS_CHOICES,
    IMAGE_TARGET_FORMATS, QUALITY_PRESETS, RESIZE_PERCENT_CHOICES, VIDEO_TARGET_FORMATS,
};
use bennu_theme::controls::{dropdown, segmented_choice_row, text_field, SegmentedChoice};

use super::super::dialog;
use super::super::settings_group::{
    control_row, expander_rows, muted_setting_text, settings_card, SettingsRow,
};

const FORMAT_PICK_WIDTH: f32 = 240.0;
const ROW_CONTROL_WIDTH: f32 = 170.0;

pub(super) fn image_section<'a>(
    section: &'a MediaSection<ImageConvertOptions>,
    encoders: Option<&ConversionEncoderAvailability>,
) -> Element<'a, Message> {
    let options = &section.options;
    let formats = available_formats(&IMAGE_TARGET_FORMATS, encoders, |encoders, format| {
        encoders.supports_lossy_image(format)
    });
    let mut rows = vec![format_row(formats, options.target_format, |format| {
        image_edit(SectionEdit::Options(ImageEdit::Format(format)))
    })];
    rows.extend(quality_rows(options, image_edit));
    let summary = resize_summary(&options.resize).into_iter().collect();
    let more = resize_rows(
        &options.resize,
        |choice| image_edit(SectionEdit::Options(ImageEdit::Resize(choice))),
        |text| image_edit(SectionEdit::Options(ImageEdit::LongEdge(text))),
    );
    rows.extend(more_rows(section, summary, image_edit, more));
    section_card(tr!("Images"), section, rows)
}

pub(super) fn video_section<'a>(
    section: &'a MediaSection<VideoConvertOptions>,
    encoders: Option<&ConversionEncoderAvailability>,
) -> Element<'a, Message> {
    let options = &section.options;
    let formats = available_formats(&VIDEO_TARGET_FORMATS, encoders, |encoders, format| {
        encoders.supports_video(format)
    });
    let mut rows = vec![format_row(formats, options.target_format, |format| {
        video_edit(SectionEdit::Options(VideoEdit::Format(format)))
    })];
    rows.extend(quality_rows(options, video_edit));
    let summary = resize_summary(&options.resize)
        .into_iter()
        .chain(options.fps_override.map(|fps| format!("{fps}fps")))
        .collect();
    let mut more = resize_rows(
        &options.resize,
        |choice| video_edit(SectionEdit::Options(VideoEdit::Resize(choice))),
        |text| video_edit(SectionEdit::Options(VideoEdit::LongEdge(text))),
    );
    more.push(control_row(
        tr!("Frame rate"),
        None,
        segmented_choice_row(fps_choices(options.fps_override)),
    ));
    rows.extend(more_rows(section, summary, video_edit, more));
    section_card(tr!("Videos"), section, rows)
}

pub(super) fn audio_section<'a>(
    section: &'a MediaSection<AudioConvertOptions>,
    encoders: Option<&ConversionEncoderAvailability>,
) -> Element<'a, Message> {
    let options = &section.options;
    let formats = available_formats(&AUDIO_TARGET_FORMATS, encoders, |encoders, format| {
        encoders.supports_audio(format)
    });
    let mut rows = vec![format_row(formats, options.target_format, |format| {
        audio_edit(SectionEdit::Options(AudioEdit::Format(format)))
    })];
    rows.extend(quality_rows(options, audio_edit));
    let summary = (options.channels == AudioChannelSpec::Mono)
        .then(|| tr!("Mono"))
        .into_iter()
        .collect();
    let channel_choices = [
        (AudioChannelSpec::Keep, tr!("Keep")),
        (AudioChannelSpec::Mono, tr!("Mono")),
    ]
    .into_iter()
    .map(|(channels, label)| SegmentedChoice {
        label,
        selected: options.channels == channels,
        message: Some(audio_edit(SectionEdit::Options(AudioEdit::Channels(
            channels,
        )))),
        tooltip: None,
    })
    .collect();
    let more = vec![control_row(
        tr!("Channels"),
        None,
        segmented_choice_row(channel_choices),
    )];
    rows.extend(more_rows(section, summary, audio_edit, more));
    section_card(tr!("Audio"), section, rows)
}

fn image_edit(edit: SectionEdit<ImageEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Image(edit)))
}

fn video_edit(edit: SectionEdit<VideoEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Video(edit)))
}

fn audio_edit(edit: SectionEdit<AudioEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Audio(edit)))
}

/// 小标题：`图片 · 3 个 · PNG、JPEG · 12 MiB`。
fn section_card<'a, O>(
    kind: String,
    section: &MediaSection<O>,
    rows: Vec<SettingsRow<'a>>,
) -> Element<'a, Message> {
    let sources = &section.sources;
    let count = sources.paths.len();
    let size = bennu_localization::format_file_size(sources.total_bytes);
    let formats = sources.format_summary(&tr!(", "));
    let heading = if formats.is_empty() {
        tr!("%1 · %2 files · %3", kind, count, size)
    } else {
        tr!("%1 · %2 files · %3 · %4", kind, count, formats, size)
    };
    dialog::field(heading, settings_card(rows))
}

/// 探测完之前列出全部格式：探测很快，回来后编不了的格式会自动退掉。
fn available_formats<F: Copy>(
    formats: &[F],
    encoders: Option<&ConversionEncoderAvailability>,
    supports: impl Fn(&ConversionEncoderAvailability, F) -> bool,
) -> Vec<F> {
    formats
        .iter()
        .copied()
        .filter(|format| encoders.is_none_or(|encoders| supports(encoders, *format)))
        .collect()
}

/// 格式多到分段按钮放不下，用下拉。
fn format_row<F>(
    formats: Vec<F>,
    selected: F,
    message: impl Fn(F) -> Message + 'static,
) -> SettingsRow<'static>
where
    F: Copy + PartialEq + std::fmt::Display + 'static,
{
    control_row(
        tr!("Format"),
        None,
        dropdown(formats, Some(selected), message).width(Length::Fixed(FORMAT_PICK_WIDTH)),
    )
}

/// 无损格式没有这一行；GIF 只有档位；其余是档位加「按大小」，选了按大小再多一行输入框。
fn quality_rows<O: MediaOptions>(
    options: &O,
    wrap: fn(SectionEdit<O::Edit>) -> Message,
) -> Vec<SettingsRow<'_>> {
    let quality = options.quality();
    let preset_choices = |selected: Option<ConversionQualityPreset>| {
        QUALITY_PRESETS
            .into_iter()
            .map(|preset| SegmentedChoice {
                label: preset_label(preset),
                selected: selected == Some(preset),
                message: Some(wrap(SectionEdit::Quality(QualityChoice::Preset(preset)))),
                tooltip: None,
            })
            .collect::<Vec<_>>()
    };
    let quality_row = |choices| control_row(tr!("Quality"), None, segmented_choice_row(choices));
    match options.quality_control() {
        QualityControl::Lossless => Vec::new(),
        QualityControl::PresetOnly => vec![quality_row(preset_choices(Some(quality.preset)))],
        QualityControl::PresetOrSize => {
            let choice = quality.choice();
            let mut choices = preset_choices(match choice {
                QualityChoice::Preset(preset) => Some(preset),
                QualityChoice::TargetSize => None,
            });
            choices.push(SegmentedChoice {
                label: tr!("By size"),
                selected: choice == QualityChoice::TargetSize,
                message: Some(wrap(SectionEdit::Quality(QualityChoice::TargetSize))),
                tooltip: None,
            });
            let mut rows = vec![quality_row(choices)];
            if choice == QualityChoice::TargetSize {
                rows.push(control_row(
                    tr!("Target size per file"),
                    None,
                    text_field("500 KiB / 2 MiB", &quality.target_size_text)
                        .on_input(move |text| wrap(SectionEdit::TargetSize(text)))
                        .width(Length::Fixed(ROW_CONTROL_WIDTH)),
                ));
            }
            rows
        }
    }
}

fn preset_label(preset: ConversionQualityPreset) -> String {
    match preset {
        ConversionQualityPreset::Low => tr!("Low"),
        ConversionQualityPreset::Medium => tr!("Medium"),
        ConversionQualityPreset::High => tr!("High"),
    }
}

/// 「更多」：收起时右边灰字列出改过的项，全是默认就空着。
fn more_rows<'a, O: MediaOptions>(
    section: &MediaSection<O>,
    summary: Vec<String>,
    wrap: fn(SectionEdit<O::Edit>) -> Message,
    rows: Vec<SettingsRow<'a>>,
) -> Vec<SettingsRow<'a>> {
    expander_rows(
        tr!("More"),
        muted_setting_text(summary.join(" · ")),
        wrap(SectionEdit::MoreToggled),
        section.more_expanded.then_some(rows),
    )
}

fn resize_summary(resize: &ResizeSelection) -> Option<String> {
    match resize.choice {
        ResizeChoice::Percent(percent) if percent >= 100 => None,
        ResizeChoice::Percent(percent) => Some(format!("{percent}%")),
        ResizeChoice::LongEdge => Some(tr!("Long edge %1", resize.long_edge_text.trim())),
    }
}

/// 缩放：百分比或「长边」；选长边再多一行填像素。
fn resize_rows<'a>(
    resize: &'a ResizeSelection,
    choose: impl Fn(ResizeChoice) -> Message,
    long_edge_changed: impl Fn(String) -> Message + 'a,
) -> Vec<SettingsRow<'a>> {
    let choices = RESIZE_PERCENT_CHOICES
        .into_iter()
        .map(|percent| (ResizeChoice::Percent(percent), format!("{percent}%")))
        .chain([(ResizeChoice::LongEdge, tr!("Long edge"))])
        .map(|(choice, label)| SegmentedChoice {
            label,
            selected: resize.choice == choice,
            message: Some(choose(choice)),
            tooltip: None,
        })
        .collect();
    let mut rows = vec![control_row(
        tr!("Resize"),
        None,
        segmented_choice_row(choices),
    )];
    if resize.choice == ResizeChoice::LongEdge {
        rows.push(control_row(
            tr!("Long edge up to"),
            None,
            row![
                text_field("1920", &resize.long_edge_text)
                    .on_input(long_edge_changed)
                    .width(Length::Fixed(ROW_CONTROL_WIDTH)),
                muted_setting_text(tr!("px")),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ));
    }
    rows
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
            message: Some(video_edit(SectionEdit::Options(VideoEdit::Fps(fps)))),
            tooltip: None,
        })
        .collect()
}
