use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use file_core::{AudioTargetFormat, ConversionQualityPreset, ImageTargetFormat, VideoTargetFormat};

use crate::app::convert::{
    AudioConvertOptions, ConvertMessage, ConvertMode, ConvertState, ImageConvertOptions,
    VideoConvertOptions, AUDIO_TARGET_FORMATS, FPS_CHOICES, IMAGE_TARGET_FORMATS, QUALITY_PRESETS,
    RESIZE_PERCENT_CHOICES, VIDEO_TARGET_FORMATS,
};
use crate::appearance::{context_menu_style, muted_text_color};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::settings_group::{control_row, custom_row, settings_card, SettingsRow};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::dropdown;
use bennu_theme::controls::slider;
use bennu_theme::controls::text_field;
use bennu_theme::controls::{normal_button, suggested_button};
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};

const CONVERT_PANEL_WIDTH: f32 = 640.0;
const SECTION_SPACING: f32 = 14.0;
const ROW_CONTROL_WIDTH: f32 = 170.0;
const FORMAT_PICK_WIDTH: f32 = 240.0;
const QUALITY_VALUE_WIDTH: f32 = 30.0;
const NOTICE_ICON_SIZE: f32 = 14.0;

pub(super) fn convert_panel(state: &ConvertState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileImage, IconTone::Normal, MENU_ICON_SIZE),
        readable_text(tr!("Convert Format"))
            .size(TITLE_SIZE)
            .font(BOLD)
            .width(Length::Fill),
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
        content = content.push(container(readable_text(error.to_owned())));
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
        rows.push(custom_row(note_text(tr!(
            "ffmpeg is missing; WebP and AVIF are unavailable."
        ))));
    } else if image_format_is_lossless(options.target_format()) {
        rows.push(custom_row(note_text(tr!(
            "This format is lossless; quality and target size do not apply."
        ))));
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
    rows.extend(resize_row(
        options.resize(),
        |percent| Message::Convert(ConvertMessage::ImageResizePercentSelected(percent)),
        |text| Message::Convert(ConvertMessage::ImageCustomWidthChanged(text)),
        Message::Convert(ConvertMessage::ImageCustomWidthToggled),
    ));

    section(
        tr!("Images"),
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
        rows.push(custom_row(note_text(tr!(
            "Install ffmpeg to convert videos."
        ))));
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
        rows.extend(resize_row(
            options.resize(),
            |percent| Message::Convert(ConvertMessage::VideoResizePercentSelected(percent)),
            |text| Message::Convert(ConvertMessage::VideoCustomWidthChanged(text)),
            Message::Convert(ConvertMessage::VideoCustomWidthToggled),
        ));
        rows.push(control_row(
            tr!("Frame rate"),
            None,
            segmented_choice_row(fps_choices(options.fps_override())),
        ));
    }

    section(
        tr!("Videos"),
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
        rows.push(custom_row(note_text(tr!(
            "Install ffmpeg to convert audio files."
        ))));
    } else if audio_format_is_lossless(options.target_format()) {
        rows.push(custom_row(note_text(tr!(
            "This format is lossless; quality and target size do not apply."
        ))));
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
    rows.push(control_row(
        tr!("Channels"),
        None,
        segmented_choice_row(vec![
            SegmentedChoice {
                label: tr!("Keep"),
                selected: options.channels() == file_core::AudioChannelSpec::Keep,
                message: Some(Message::Convert(ConvertMessage::AudioChannelsSelected(
                    file_core::AudioChannelSpec::Keep,
                ))),
                tooltip: None,
            },
            SegmentedChoice {
                label: tr!("Mono"),
                selected: options.channels() == file_core::AudioChannelSpec::Mono,
                message: Some(Message::Convert(ConvertMessage::AudioChannelsSelected(
                    file_core::AudioChannelSpec::Mono,
                ))),
                tooltip: None,
            },
        ]),
    ));

    section(
        tr!("Audio"),
        state.audio_source_count(),
        IconSymbol::Music,
        rows,
    )
}

/// 分区:muted 标题 + 卡片行组;与设置窗口的分组观感一致。
fn section<'a>(
    title: String,
    count: usize,
    icon: IconSymbol,
    rows: Vec<SettingsRow<'a>>,
) -> Element<'a, Message> {
    column![
        container(
            row![
                themed_icon(icon, IconTone::Normal, NOTICE_ICON_SIZE),
                readable_text(format!("{title} · {count}")),
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
) -> SettingsRow<'static>
where
    F: Copy + PartialEq + std::fmt::Display + 'static,
{
    control_row(
        tr!("Format"),
        None,
        dropdown(formats.to_vec(), Some(selected), message).width(Length::Fixed(FORMAT_PICK_WIDTH)),
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
) -> SettingsRow<'static> {
    control_row(
        tr!("Mode"),
        None,
        segmented_choice_row(vec![
            SegmentedChoice {
                label: tr!("Quality"),
                selected: mode == ConvertMode::Quality,
                message: Some(message(ConvertMode::Quality)),
                tooltip: None,
            },
            SegmentedChoice {
                label: tr!("Target size"),
                selected: mode == ConvertMode::TargetSize,
                message: Some(message(ConvertMode::TargetSize)),
                tooltip: None,
            },
        ]),
    )
}

fn image_quality_row(options: &ImageConvertOptions) -> SettingsRow<'static> {
    match options.mode() {
        ConvertMode::Quality => control_row(
            tr!("Level"),
            None,
            row![
                slider(1..=100, options.quality(), |value| Message::Convert(
                    ConvertMessage::ImageQualityChanged(value)
                ),)
                .width(Length::Fill),
                readable_text(format!("{}", options.quality()))
                    .width(Length::Fixed(QUALITY_VALUE_WIDTH)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ),
        ConvertMode::TargetSize => target_size_row(options.target_size_text(), |text| {
            Message::Convert(ConvertMessage::ImageTargetSizeChanged(text))
        }),
    }
}

fn quality_preset_row(
    preset: ConversionQualityPreset,
    preset_message: impl Fn(ConversionQualityPreset) -> Message + 'static,
) -> SettingsRow<'static> {
    control_row(
        tr!("Quality"),
        None,
        segmented_choice_row(
            QUALITY_PRESETS
                .iter()
                .copied()
                .map(|candidate| SegmentedChoice {
                    label: preset_label(candidate),
                    selected: candidate == preset,
                    message: Some(preset_message(candidate)),
                    tooltip: None,
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
) -> SettingsRow<'static> {
    match mode {
        ConvertMode::Quality => quality_preset_row(preset, preset_message),
        ConvertMode::TargetSize => target_size_row(target_size_text, size_message),
    }
}

fn preset_label(preset: ConversionQualityPreset) -> String {
    match preset {
        ConversionQualityPreset::Low => tr!("Low"),
        ConversionQualityPreset::Medium => tr!("Medium"),
        ConversionQualityPreset::High => tr!("High"),
    }
}

fn target_size_row(
    target_size_text: &str,
    size_message: impl Fn(String) -> Message + 'static,
) -> SettingsRow<'static> {
    control_row(
        tr!("Target size per file"),
        None,
        text_field("500KB / 2MB", target_size_text)
            .on_input(size_message)
            .width(Length::Fixed(ROW_CONTROL_WIDTH)),
    )
}

/// 尺寸行:百分比预设 + 自定义;勾选自定义时在同一卡片内追加宽度输入行。
fn resize_row(
    selection: &crate::app::convert::ResizeSelection,
    percent_message: impl Fn(u8) -> Message + Copy + 'static,
    custom_changed: impl Fn(String) -> Message + 'static,
    custom_toggle: Message,
) -> Vec<SettingsRow<'static>> {
    let mut choices: Vec<SegmentedChoice<Message>> = RESIZE_PERCENT_CHOICES
        .iter()
        .copied()
        .map(|percent| SegmentedChoice {
            label: format!("{percent}%"),
            selected: !selection.uses_custom_width() && selection.percent() == percent,
            message: Some(percent_message(percent)),
            tooltip: None,
        })
        .collect();
    choices.push(SegmentedChoice {
        label: tr!("Custom"),
        selected: selection.uses_custom_width(),
        message: Some(custom_toggle),
        tooltip: None,
    });

    let mut rows = vec![control_row(
        tr!("Resize"),
        None,
        segmented_choice_row(choices),
    )];
    if selection.uses_custom_width() {
        rows.push(control_row(
            tr!("Width"),
            None,
            text_field("1920", selection.custom_width_text())
                .on_input(custom_changed)
                .width(Length::Fixed(ROW_CONTROL_WIDTH)),
        ));
    }
    rows
}

fn fps_choices(selected: Option<u32>) -> Vec<SegmentedChoice<Message>> {
    let mut choices = vec![SegmentedChoice {
        label: tr!("Keep"),
        selected: selected.is_none(),
        message: Some(Message::Convert(ConvertMessage::VideoFpsSelected(None))),
        tooltip: None,
    }];
    choices.extend(FPS_CHOICES.iter().copied().map(|fps| SegmentedChoice {
        label: format!("{fps}fps"),
        selected: selected == Some(fps),
        message: Some(Message::Convert(ConvertMessage::VideoFpsSelected(Some(
            fps,
        )))),
        tooltip: None,
    }));
    choices
}

fn ffmpeg_notice(state: &ConvertState) -> Option<Element<'static, Message>> {
    let message = if !state.ffmpeg_probed() {
        tr!("Checking ffmpeg availability...")
    } else if state.ffmpeg_available() {
        return None;
    } else {
        tr!("ffmpeg is missing; video, audio, WebP and AVIF conversion is disabled. Install ffmpeg to enable them.")
    };
    Some(
        row![
            themed_icon(
                IconSymbol::TriangleAlert,
                IconTone::Normal,
                NOTICE_ICON_SIZE
            ),
            readable_text(message).width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    )
}

fn note_text(message: String) -> Element<'static, Message> {
    readable_text(message).into()
}

fn actions(state: &ConvertState) -> Element<'static, Message> {
    let convert_button = if state.can_submit() {
        suggested_button(readable_text(tr!("Convert")))
            .on_press(Message::Convert(ConvertMessage::Submitted))
    } else {
        suggested_button(readable_text(tr!("Convert")))
    };

    row![
        Space::new().width(Length::Fill),
        normal_button(readable_text(tr!("Cancel"))).on_press(Message::DismissFloating),
        convert_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn source_summary(state: &ConvertState) -> Element<'static, Message> {
    let summary = tr!(
        "%1 items · Converted files are written next to the originals; existing files are never replaced.",
        state.source_count()
    );
    container(
        readable_text(summary)
            .size(CAPTION_SIZE)
            .width(Length::Fill),
    )
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
