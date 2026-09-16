use iced::widget::{column, container, pick_list, row, slider, text_input, Column, Space};
use iced::{Alignment, Element, Length};

use file_core::{AudioTargetFormat, ConversionQualityPreset, ImageTargetFormat, VideoTargetFormat};

use crate::app::convert::{
    AudioConvertOptions, ConvertMessage, ConvertMode, ConvertState, ImageConvertOptions,
    VideoConvertOptions, AUDIO_TARGET_FORMATS, FPS_CHOICES, IMAGE_TARGET_FORMATS, QUALITY_PRESETS,
    RESIZE_PERCENT_CHOICES, VIDEO_TARGET_FORMATS,
};
use crate::appearance::{context_menu_style, muted_text_color};
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::readable_text;

use super::option_controls::{
    inactive_primary_action_button, primary_action_button, secondary_action_button,
    segmented_choice_row, SegmentedChoice,
};
use super::settings_group::{info_setting_row, labeled_setting_row, settings_card};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const CONVERT_PANEL_WIDTH: f32 = 640.0;
const SECTION_SPACING: f32 = 14.0;
const ROW_CONTROL_WIDTH: f32 = 170.0;
const FORMAT_PICK_WIDTH: f32 = 240.0;
const QUALITY_VALUE_WIDTH: f32 = 30.0;
const NOTICE_ICON_SIZE: f32 = 14.0;

pub(super) fn convert_panel(state: &ConvertState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileImage, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("Convert Format").size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut content = column![title, source_summary(state)]
        .spacing(SECTION_SPACING)
        .width(Length::Fill);

    if let Some(options) = state.image() {
        content = content.push(image_section(state, options));
    }
    if let Some(options) = state.video() {
        content = content.push(video_section(state, options));
    }
    if let Some(options) = state.audio() {
        content = content.push(audio_section(state, options));
    }

    if let Some(notice) = ffmpeg_notice(state) {
        content = content.push(notice);
    }
    if let Some(error) = state.validation_error() {
        content = content.push(container(
            readable_text(crate::localization::translate_current(error)).size(12),
        ));
    }
    content = content.push(actions(state));

    container(content)
        .padding(16)
        .width(Length::Fixed(CONVERT_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn image_section<'a>(
    state: &'a ConvertState,
    options: &'a ImageConvertOptions,
) -> Element<'a, Message> {
    let formats: Vec<_> = IMAGE_TARGET_FORMATS
        .iter()
        .copied()
        .filter(|format| state.encoders().supports_lossy_image(*format))
        .collect();
    let mut rows = vec![format_row(&formats, options.target_format(), |format| {
        Message::Convert(ConvertMessage::ImageFormatSelected(format))
    })];

    if formats.is_empty() {
        rows.push(info_setting_row(lossless_note_text(
            "ffmpeg is missing; WebP and AVIF are unavailable.",
        )));
    } else if image_format_is_lossless(options.target_format()) {
        rows.push(info_setting_row(lossless_note_text(
            "This format is lossless; quality and target size do not apply.",
        )));
    } else if options.target_format() == ImageTargetFormat::Gif {
        // GIF 调色板编码只有档位语义,没有质量/体积模式之分。
        rows.push(quality_preset_row(options.preset(), |preset| {
            Message::Convert(ConvertMessage::ImagePresetSelected(preset))
        }));
    } else {
        rows.push(mode_row(options.mode(), |mode| {
            Message::Convert(ConvertMessage::ImageModeSelected(mode))
        }));
        rows.push(image_quality_row(options));
    }
    rows.push(resize_row(
        options.resize(),
        |percent| Message::Convert(ConvertMessage::ImageResizePercentSelected(percent)),
        |text| Message::Convert(ConvertMessage::ImageCustomWidthChanged(text)),
        Message::Convert(ConvertMessage::ImageCustomWidthToggled),
    ));

    section(
        "Images",
        state.image_source_count(),
        IconSymbol::FileImage,
        rows,
    )
}

fn video_section<'a>(
    state: &'a ConvertState,
    options: &'a VideoConvertOptions,
) -> Element<'a, Message> {
    let formats: Vec<_> = VIDEO_TARGET_FORMATS
        .iter()
        .copied()
        .filter(|format| state.encoders().supports_video(*format))
        .collect();
    let mut rows = vec![format_row(&formats, options.target_format(), |format| {
        Message::Convert(ConvertMessage::VideoFormatSelected(format))
    })];

    if formats.is_empty() {
        rows.push(info_setting_row(lossless_note_text(
            "Install ffmpeg to convert videos.",
        )));
    } else if options.target_format() == VideoTargetFormat::Gif {
        // GIF 调色板编码只有档位语义,没有质量/体积模式之分。
        rows.push(quality_preset_row(options.preset(), |preset| {
            Message::Convert(ConvertMessage::VideoPresetSelected(preset))
        }));
    } else {
        rows.push(mode_row(options.mode(), |mode| {
            Message::Convert(ConvertMessage::VideoModeSelected(mode))
        }));
        rows.push(preset_or_size_row(
            options.mode(),
            options.preset(),
            options.target_size_text(),
            |preset| Message::Convert(ConvertMessage::VideoPresetSelected(preset)),
            |text| Message::Convert(ConvertMessage::VideoTargetSizeChanged(text)),
        ));
        rows.push(resize_row(
            options.resize(),
            |percent| Message::Convert(ConvertMessage::VideoResizePercentSelected(percent)),
            |text| Message::Convert(ConvertMessage::VideoCustomWidthChanged(text)),
            Message::Convert(ConvertMessage::VideoCustomWidthToggled),
        ));
        rows.push(labeled_setting_row(
            "Frame rate",
            segmented_choice_row(fps_choices(options.fps_override())),
        ));
    }

    section(
        "Videos",
        state.video_source_count(),
        IconSymbol::Video,
        rows,
    )
}

fn audio_section<'a>(
    state: &'a ConvertState,
    options: &'a AudioConvertOptions,
) -> Element<'a, Message> {
    let formats: Vec<_> = AUDIO_TARGET_FORMATS
        .iter()
        .copied()
        .filter(|format| state.encoders().supports_audio(*format))
        .collect();
    let mut rows = vec![format_row(&formats, options.target_format(), |format| {
        Message::Convert(ConvertMessage::AudioFormatSelected(format))
    })];

    if formats.is_empty() {
        rows.push(info_setting_row(lossless_note_text(
            "Install ffmpeg to convert audio files.",
        )));
    } else if audio_format_is_lossless(options.target_format()) {
        rows.push(info_setting_row(lossless_note_text(
            "This format is lossless; quality and target size do not apply.",
        )));
    } else {
        rows.push(mode_row(options.mode(), |mode| {
            Message::Convert(ConvertMessage::AudioModeSelected(mode))
        }));
        rows.push(preset_or_size_row(
            options.mode(),
            options.preset(),
            options.target_size_text(),
            |preset| Message::Convert(ConvertMessage::AudioPresetSelected(preset)),
            |text| Message::Convert(ConvertMessage::AudioTargetSizeChanged(text)),
        ));
    }
    rows.push(labeled_setting_row(
        "Channels",
        segmented_choice_row(vec![
            SegmentedChoice {
                label: "Keep",
                selected: options.channels() == file_core::AudioChannelSpec::Keep,
                message: Message::Convert(ConvertMessage::AudioChannelsSelected(
                    file_core::AudioChannelSpec::Keep,
                )),
            },
            SegmentedChoice {
                label: "Mono",
                selected: options.channels() == file_core::AudioChannelSpec::Mono,
                message: Message::Convert(ConvertMessage::AudioChannelsSelected(
                    file_core::AudioChannelSpec::Mono,
                )),
            },
        ]),
    ));

    section("Audio", state.audio_source_count(), IconSymbol::Music, rows)
}

/// 分区:muted 标题 + 卡片行组;与设置窗口的分组观感一致。
fn section<'a>(
    title: &'static str,
    count: usize,
    icon: IconSymbol,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let localized_title = crate::localization::translate_current(title);
    column![
        container(
            row![
                themed_icon(icon, IconTone::Normal, NOTICE_ICON_SIZE),
                readable_text(format!("{localized_title} · {count}")).size(12),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .style(muted_container_style),
        settings_card(rows),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// 目标格式用下拉选择:格式多到分段按钮放不下,下拉能承载完整清单。
fn format_row<F>(
    formats: &[F],
    selected: F,
    message: impl Fn(F) -> Message + 'static,
) -> Element<'static, Message>
where
    F: Copy + PartialEq + std::fmt::Display + 'static,
{
    labeled_setting_row(
        "Format",
        pick_list(formats.to_vec(), Some(selected), message)
            .width(Length::Fixed(FORMAT_PICK_WIDTH))
            .text_size(12)
            .padding([5, 8])
            .into(),
    )
}

fn image_format_is_lossless(format: ImageTargetFormat) -> bool {
    matches!(
        format,
        ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico
    )
}

fn audio_format_is_lossless(format: AudioTargetFormat) -> bool {
    matches!(format, AudioTargetFormat::Flac | AudioTargetFormat::Wav)
}

fn mode_row(
    mode: ConvertMode,
    message: impl Fn(ConvertMode) -> Message + 'static,
) -> Element<'static, Message> {
    labeled_setting_row(
        "Mode",
        segmented_choice_row(vec![
            SegmentedChoice {
                label: "Quality",
                selected: mode == ConvertMode::Quality,
                message: message(ConvertMode::Quality),
            },
            SegmentedChoice {
                label: "Target size",
                selected: mode == ConvertMode::TargetSize,
                message: message(ConvertMode::TargetSize),
            },
        ]),
    )
}

fn image_quality_row(options: &ImageConvertOptions) -> Element<'static, Message> {
    match options.mode() {
        ConvertMode::Quality => labeled_setting_row(
            "Level",
            row![
                slider(1..=100, options.quality(), |value| Message::Convert(
                    ConvertMessage::ImageQualityChanged(value)
                ),)
                .width(Length::Fill),
                readable_text(format!("{}", options.quality()))
                    .size(12)
                    .width(Length::Fixed(QUALITY_VALUE_WIDTH)),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        ),
        ConvertMode::TargetSize => target_size_row(options.target_size_text(), |text| {
            Message::Convert(ConvertMessage::ImageTargetSizeChanged(text))
        }),
    }
}

fn quality_preset_row(
    preset: ConversionQualityPreset,
    preset_message: impl Fn(ConversionQualityPreset) -> Message + 'static,
) -> Element<'static, Message> {
    labeled_setting_row(
        "Quality",
        segmented_choice_row(
            QUALITY_PRESETS
                .iter()
                .copied()
                .map(|candidate| SegmentedChoice {
                    label: preset_label(candidate),
                    selected: candidate == preset,
                    message: preset_message(candidate),
                })
                .collect(),
        ),
    )
}

fn preset_or_size_row(
    mode: ConvertMode,
    preset: ConversionQualityPreset,
    target_size_text: &str,
    preset_message: impl Fn(ConversionQualityPreset) -> Message + 'static,
    size_message: impl Fn(String) -> Message + 'static,
) -> Element<'static, Message> {
    match mode {
        ConvertMode::Quality => quality_preset_row(preset, preset_message),
        ConvertMode::TargetSize => target_size_row(target_size_text, size_message),
    }
}

fn preset_label(preset: ConversionQualityPreset) -> &'static str {
    match preset {
        ConversionQualityPreset::Low => "Low",
        ConversionQualityPreset::Medium => "Medium",
        ConversionQualityPreset::High => "High",
    }
}

fn target_size_row(
    target_size_text: &str,
    size_message: impl Fn(String) -> Message + 'static,
) -> Element<'static, Message> {
    labeled_setting_row(
        "Target size per file",
        text_input("500KB / 2MB", target_size_text)
            .on_input(size_message)
            .padding([6, 8])
            .size(14)
            .width(Length::Fixed(ROW_CONTROL_WIDTH))
            .into(),
    )
}

/// 尺寸行:百分比预设 + 自定义;勾选自定义时在同一卡片内追加宽度输入行。
fn resize_row(
    selection: &crate::app::convert::ResizeSelection,
    percent_message: impl Fn(u8) -> Message + Copy + 'static,
    custom_changed: impl Fn(String) -> Message + 'static,
    custom_toggle: Message,
) -> Element<'static, Message> {
    let mut choices: Vec<SegmentedChoice> = RESIZE_PERCENT_CHOICES
        .iter()
        .copied()
        .map(|percent| SegmentedChoice {
            label: percent_label(percent),
            selected: !selection.uses_custom_width() && selection.percent() == percent,
            message: percent_message(percent),
        })
        .collect();
    choices.push(SegmentedChoice {
        label: "Custom",
        selected: selection.uses_custom_width(),
        message: custom_toggle,
    });

    // 行标签用 "Resize" 而非 "Size":该行是缩放百分比选择,裸 "Size"
    // 要留给文件大小语义(中文"大小"),避免词条被尺寸语义遮蔽。
    let mut rows = vec![labeled_setting_row("Resize", segmented_choice_row(choices))];
    if selection.uses_custom_width() {
        rows.push(labeled_setting_row(
            "Width",
            text_input("1920", selection.custom_width_text())
                .on_input(custom_changed)
                .padding([6, 8])
                .size(14)
                .width(Length::Fixed(ROW_CONTROL_WIDTH))
                .into(),
        ));
    }
    if rows.len() == 1 {
        rows.remove(0)
    } else {
        Column::with_children(rows).width(Length::Fill).into()
    }
}

fn percent_label(percent: u8) -> &'static str {
    match percent {
        50 => "50%",
        75 => "75%",
        _ => "100%",
    }
}

fn fps_choices(selected: Option<u32>) -> Vec<SegmentedChoice> {
    let mut choices = vec![SegmentedChoice {
        label: "Keep",
        selected: selected.is_none(),
        message: Message::Convert(ConvertMessage::VideoFpsSelected(None)),
    }];
    choices.extend(FPS_CHOICES.iter().copied().map(|fps| SegmentedChoice {
        label: fps_label(fps),
        selected: selected == Some(fps),
        message: Message::Convert(ConvertMessage::VideoFpsSelected(Some(fps))),
    }));
    choices
}

fn fps_label(fps: u32) -> &'static str {
    match fps {
        24 => "24fps",
        30 => "30fps",
        _ => "60fps",
    }
}

fn ffmpeg_notice(state: &ConvertState) -> Option<Element<'static, Message>> {
    let message = if !state.ffmpeg_probed() {
        "Checking ffmpeg availability..."
    } else if state.ffmpeg_available() {
        return None;
    } else {
        "ffmpeg is missing; video, audio, WebP and AVIF conversion is disabled. Install ffmpeg to enable them."
    };
    Some(
        row![
            themed_icon(
                IconSymbol::TriangleAlert,
                IconTone::Normal,
                NOTICE_ICON_SIZE
            ),
            readable_text(crate::localization::translate_current(message))
                .size(12)
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    )
}

fn lossless_note_text(message: &'static str) -> Element<'static, Message> {
    readable_text(crate::localization::translate_current(message))
        .size(12)
        .into()
}

fn actions(state: &ConvertState) -> Element<'static, Message> {
    let convert_button = if state.can_submit() {
        primary_action_button("Convert", Message::Convert(ConvertMessage::Submitted))
    } else {
        inactive_primary_action_button("Convert")
    };

    row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::DismissFloating),
        convert_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn source_summary(state: &ConvertState) -> Element<'static, Message> {
    let summary = if crate::localization::current_language_is_chinese() {
        format!(
            "{} 个项目 · 输出到原目录,保留原文件,重名自动追加序号",
            state.source_count()
        )
    } else {
        format!(
            "{} items · Converted files are written next to the originals; existing files are never replaced.",
            state.source_count()
        )
    };
    container(readable_text(summary).size(11).width(Length::Fill))
        .width(Length::Fill)
        .style(muted_container_style)
        .into()
}

fn muted_container_style(theme: &iced::Theme) -> container::Style {
    container::Style {
        text_color: Some(muted_text_color(theme)),
        ..container::Style::default()
    }
}
