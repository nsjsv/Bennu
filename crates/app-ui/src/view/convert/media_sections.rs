//! 图片、音频卡片，和三类共用的行：格式、质量、「更多」、缩放、声道。
//! 视频卡片在 `video_section.rs`。

use iced::widget::{container, row};
use iced::{Alignment, Element, Length};

use file_core::{
    AudioChannelSpec, ConversionEncoderAvailability, ConversionQualityPreset, PhotoMetadata,
};

use crate::app::convert::ConvertMessage;
use crate::model::{
    AudioConvertOptions, AudioEdit, ConvertEdit, ImageConvertOptions, ImageEdit, MediaOptions,
    MediaSection, Message, QualityChoice, QualityControl, QualitySelection, ResizeChoice,
    ResizeSelection, SectionEdit, SizeUnit, AUDIO_TARGET_FORMATS, IMAGE_TARGET_FORMATS,
    QUALITY_PRESETS, RESIZE_PERCENT_CHOICES, SIZE_UNITS,
};
use bennu_theme::controls::{dropdown, segmented_choice_row, text_field, SegmentedChoice};

use super::super::dialog;
use super::super::settings_group::{
    control_row, expander_rows, muted_setting_text, settings_card, RowNote, SettingsRow,
};

const FORMAT_PICK_WIDTH: f32 = 240.0;
pub(super) const ROW_CONTROL_WIDTH: f32 = 170.0;
const TARGET_SIZE_FIELD_WIDTH: f32 = 120.0;
const SIZE_UNIT_PICK_WIDTH: f32 = 90.0;

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
    let mut summary: Vec<String> = resize_summary(&options.resize).into_iter().collect();
    let mut more = resize_rows(
        &options.resize,
        |choice| image_edit(SectionEdit::Options(ImageEdit::Resize(choice))),
        |text| image_edit(SectionEdit::Options(ImageEdit::LongEdge(text))),
    );
    // 别的格式写不进这些信息，不给一个不起作用的开关。
    if options.target_format.carries_photo_metadata() {
        if options.metadata == PhotoMetadata::Strip {
            summary.push(tr!("No photo info"));
        }
        more.push(photo_metadata_row(options.metadata));
    }
    rows.extend(more_rows(section, summary, image_edit, more));
    section_card(tr!("Images"), section, rows)
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
    let summary = channels_summary(options.channels).into_iter().collect();
    let more = vec![channels_row(options.channels, |channels| {
        audio_edit(SectionEdit::Options(AudioEdit::Channels(channels)))
    })];
    rows.extend(more_rows(section, summary, audio_edit, more));
    section_card(tr!("Audio"), section, rows)
}

fn image_edit(edit: SectionEdit<ImageEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Image(edit)))
}

fn audio_edit(edit: SectionEdit<AudioEdit>) -> Message {
    Message::Convert(ConvertMessage::Edit(ConvertEdit::Audio(edit)))
}

/// 小标题：`图片 · 3 个 · PNG、JPEG · 12 MiB`。
pub(super) fn section_card<'a, O>(
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
pub(super) fn available_formats<F: Copy>(
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
pub(super) fn format_row<F>(
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
pub(super) fn quality_rows<O: MediaOptions>(
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
                rows.push(target_size_row(quality, wrap));
            }
            rows
        }
    }
}

/// 只填数字，单位在后面的下拉里选。
fn target_size_row<'a, E: 'a>(
    quality: &'a QualitySelection,
    wrap: fn(SectionEdit<E>) -> Message,
) -> SettingsRow<'a> {
    control_row(
        tr!("Target size per file"),
        None,
        row![
            text_field("", &quality.target_size_text)
                .on_input(move |text| wrap(SectionEdit::TargetSize(text)))
                .width(Length::Fixed(TARGET_SIZE_FIELD_WIDTH)),
            dropdown(SIZE_UNITS, Some(quality.target_size_unit), move |unit| {
                wrap(SectionEdit::TargetSizeUnit(unit))
            })
            .width(Length::Fixed(SIZE_UNIT_PICK_WIDTH)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
}

/// 与界面上其余大小写法一致（KiB/MiB/GiB），中英文都不翻。
impl std::fmt::Display for SizeUnit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::KiB => "KiB",
            Self::MiB => "MiB",
            Self::GiB => "GiB",
        })
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
pub(super) fn more_rows<'a, O: MediaOptions>(
    section: &MediaSection<O>,
    summary: Vec<String>,
    wrap: fn(SectionEdit<O::Edit>) -> Message,
    rows: Vec<SettingsRow<'a>>,
) -> Vec<SettingsRow<'a>> {
    // 灰字默认占满剩余宽度，会和左边的标题平分一行；收成按内容宽，贴在右边。
    expander_rows(
        tr!("More"),
        container(muted_setting_text(summary.join(" · "))).width(Length::Shrink),
        wrap(SectionEdit::MoreToggled),
        section.more_expanded.then_some(rows),
    )
}

pub(super) fn resize_summary(resize: &ResizeSelection) -> Option<String> {
    match resize.choice {
        ResizeChoice::Percent(percent) if percent >= 100 => None,
        ResizeChoice::Percent(percent) => Some(format!("{percent}%")),
        ResizeChoice::LongEdge => Some(tr!("Long edge %1", resize.long_edge_text.trim())),
    }
}

/// 缩放：百分比或「长边」；选长边再多一行填像素。
pub(super) fn resize_rows<'a>(
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

/// 拍摄时间、相机、GPS、颜色配置；去掉时连位置一起去掉，发给别人更放心。
fn photo_metadata_row(selected: PhotoMetadata) -> SettingsRow<'static> {
    let choices = [
        (PhotoMetadata::Keep, tr!("Keep")),
        (PhotoMetadata::Strip, tr!("Remove")),
    ]
    .into_iter()
    .map(|(metadata, label)| SegmentedChoice {
        label,
        selected: selected == metadata,
        message: Some(image_edit(SectionEdit::Options(ImageEdit::Metadata(
            metadata,
        )))),
        tooltip: None,
    })
    .collect();
    control_row(
        tr!("Photo info"),
        Some(RowNote::Hint(tr!(
            "Date taken, camera, location and color profile"
        ))),
        segmented_choice_row(choices),
    )
}

pub(super) fn channels_summary(channels: AudioChannelSpec) -> Option<String> {
    (channels == AudioChannelSpec::Mono).then(|| tr!("Mono"))
}

pub(super) fn channels_row(
    selected: AudioChannelSpec,
    message: impl Fn(AudioChannelSpec) -> Message,
) -> SettingsRow<'static> {
    let choices = [
        (AudioChannelSpec::Keep, tr!("Keep")),
        (AudioChannelSpec::Mono, tr!("Mono")),
    ]
    .into_iter()
    .map(|(channels, label)| SegmentedChoice {
        label,
        selected: selected == channels,
        message: Some(message(channels)),
        tooltip: None,
    })
    .collect();
    control_row(tr!("Channels"), None, segmented_choice_row(choices))
}
