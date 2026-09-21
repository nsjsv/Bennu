use std::fmt;
use std::path::{Path, PathBuf};

use file_core::{
    available_conversion_encoders, AudioChannelSpec, AudioTargetFormat,
    ConversionEncoderAvailability, ConversionQualityPreset, ConversionRequest, ConversionTarget,
    ImageTargetFormat, QualitySpec, ResizeSpec, VideoTargetFormat,
};
use iced::Task;

use super::FileBrowser;
use crate::model::Message;
use crate::operation_queue::QueuedFileOperation;

pub(crate) const IMAGE_TARGET_FORMATS: [ImageTargetFormat; 9] = [
    ImageTargetFormat::Jpeg,
    ImageTargetFormat::Png,
    ImageTargetFormat::Webp,
    ImageTargetFormat::Avif,
    ImageTargetFormat::Jxl,
    ImageTargetFormat::Gif,
    ImageTargetFormat::Tiff,
    ImageTargetFormat::Bmp,
    ImageTargetFormat::Ico,
];
pub(crate) const VIDEO_TARGET_FORMATS: [VideoTargetFormat; 9] = [
    VideoTargetFormat::Mp4H264,
    VideoTargetFormat::Mp4H265,
    VideoTargetFormat::MkvH265,
    VideoTargetFormat::MkvAv1,
    VideoTargetFormat::WebmVp9,
    VideoTargetFormat::WebmAv1,
    VideoTargetFormat::MovH264,
    VideoTargetFormat::AviMpeg4,
    VideoTargetFormat::Gif,
];
pub(crate) const AUDIO_TARGET_FORMATS: [AudioTargetFormat; 6] = [
    AudioTargetFormat::Mp3,
    AudioTargetFormat::M4a,
    AudioTargetFormat::Opus,
    AudioTargetFormat::Ogg,
    AudioTargetFormat::Flac,
    AudioTargetFormat::Wav,
];
pub(crate) const QUALITY_PRESETS: [ConversionQualityPreset; 3] = [
    ConversionQualityPreset::Low,
    ConversionQualityPreset::Medium,
    ConversionQualityPreset::High,
];
pub(crate) const RESIZE_PERCENT_CHOICES: [u8; 3] = [100, 75, 50];
pub(crate) const FPS_CHOICES: [u32; 3] = [60, 30, 24];

const DEFAULT_TARGET_SIZE_TEXT: &str = "1MB";
const DEFAULT_IMAGE_QUALITY: u8 = 80;
const FFMPEG_MISSING_MESSAGE: &str =
    "ffmpeg is missing for the selected target format. Install ffmpeg and retry.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConvertMode {
    Quality,
    TargetSize,
}

/// 尺寸缩放选择:预设百分比或自定义宽度;100% 视为保持原尺寸。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResizeSelection {
    percent: u8,
    uses_custom_width: bool,
    custom_width_text: String,
}

impl Default for ResizeSelection {
    fn default() -> Self {
        Self {
            percent: 100,
            uses_custom_width: false,
            custom_width_text: String::new(),
        }
    }
}

impl ResizeSelection {
    fn to_resize_spec(&self) -> Result<ResizeSpec, String> {
        if !self.uses_custom_width {
            if self.percent >= 100 {
                return Ok(ResizeSpec::Keep);
            }
            return Ok(ResizeSpec::Percent(self.percent));
        }
        let width = self
            .custom_width_text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|width| *width > 0)
            .ok_or_else(|| "Enter a custom width in pixels.".to_owned())?;
        Ok(ResizeSpec::Width(width))
    }

    pub(crate) fn percent(&self) -> u8 {
        self.percent
    }

    pub(crate) fn uses_custom_width(&self) -> bool {
        self.uses_custom_width
    }

    pub(crate) fn custom_width_text(&self) -> &str {
        &self.custom_width_text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageConvertOptions {
    target_format: ImageTargetFormat,
    mode: ConvertMode,
    quality: u8,
    /// GIF 等档位型格式的质量预设。
    preset: ConversionQualityPreset,
    target_size_text: String,
    resize: ResizeSelection,
}

impl Default for ImageConvertOptions {
    fn default() -> Self {
        Self {
            target_format: ImageTargetFormat::Jpeg,
            mode: ConvertMode::Quality,
            quality: DEFAULT_IMAGE_QUALITY,
            preset: ConversionQualityPreset::Medium,
            target_size_text: default_target_size_text(),
            resize: ResizeSelection::default(),
        }
    }
}

impl ImageConvertOptions {
    pub(crate) fn target_format(&self) -> ImageTargetFormat {
        self.target_format
    }

    pub(crate) fn preset(&self) -> ConversionQualityPreset {
        self.preset
    }

    pub(crate) fn mode(&self) -> ConvertMode {
        self.mode
    }

    pub(crate) fn quality(&self) -> u8 {
        self.quality
    }

    pub(crate) fn target_size_text(&self) -> &str {
        &self.target_size_text
    }

    pub(crate) fn resize(&self) -> &ResizeSelection {
        &self.resize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoConvertOptions {
    target_format: VideoTargetFormat,
    mode: ConvertMode,
    preset: ConversionQualityPreset,
    target_size_text: String,
    resize: ResizeSelection,
    fps_override: Option<u32>,
}

impl Default for VideoConvertOptions {
    fn default() -> Self {
        Self {
            target_format: VideoTargetFormat::Mp4H264,
            mode: ConvertMode::Quality,
            preset: ConversionQualityPreset::Medium,
            target_size_text: default_target_size_text(),
            resize: ResizeSelection::default(),
            fps_override: None,
        }
    }
}

impl VideoConvertOptions {
    pub(crate) fn target_format(&self) -> VideoTargetFormat {
        self.target_format
    }

    pub(crate) fn mode(&self) -> ConvertMode {
        self.mode
    }

    pub(crate) fn preset(&self) -> ConversionQualityPreset {
        self.preset
    }

    pub(crate) fn target_size_text(&self) -> &str {
        &self.target_size_text
    }

    pub(crate) fn resize(&self) -> &ResizeSelection {
        &self.resize
    }

    pub(crate) fn fps_override(&self) -> Option<u32> {
        self.fps_override
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioConvertOptions {
    target_format: AudioTargetFormat,
    mode: ConvertMode,
    preset: ConversionQualityPreset,
    target_size_text: String,
    channels: AudioChannelSpec,
}

impl Default for AudioConvertOptions {
    fn default() -> Self {
        Self {
            target_format: AudioTargetFormat::Mp3,
            mode: ConvertMode::Quality,
            preset: ConversionQualityPreset::Medium,
            target_size_text: default_target_size_text(),
            channels: AudioChannelSpec::Keep,
        }
    }
}

impl AudioConvertOptions {
    pub(crate) fn target_format(&self) -> AudioTargetFormat {
        self.target_format
    }

    pub(crate) fn mode(&self) -> ConvertMode {
        self.mode
    }

    pub(crate) fn preset(&self) -> ConversionQualityPreset {
        self.preset
    }

    pub(crate) fn target_size_text(&self) -> &str {
        &self.target_size_text
    }

    pub(crate) fn channels(&self) -> AudioChannelSpec {
        self.channels
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConvertState {
    image_sources: Vec<PathBuf>,
    video_sources: Vec<PathBuf>,
    audio_sources: Vec<PathBuf>,
    image: Option<ImageConvertOptions>,
    video: Option<VideoConvertOptions>,
    audio: Option<AudioConvertOptions>,
    ffmpeg_available: bool,
    ffmpeg_probed: bool,
    encoders: ConversionEncoderAvailability,
    validation_error: Option<String>,
}

impl fmt::Debug for ConvertState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConvertState")
            .field("image_sources", &self.image_sources)
            .field("video_sources", &self.video_sources)
            .field("audio_sources", &self.audio_sources)
            .field("image", &self.image)
            .field("video", &self.video)
            .field("audio", &self.audio)
            .field("ffmpeg_available", &self.ffmpeg_available)
            .field("ffmpeg_probed", &self.ffmpeg_probed)
            .field("validation_error", &self.validation_error)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConvertMessage {
    OpenSelected,
    Submitted,
    FfmpegProbed {
        available: bool,
        encoders: ConversionEncoderAvailability,
    },
    ImageFormatSelected(ImageTargetFormat),
    ImagePresetSelected(ConversionQualityPreset),
    ImageModeSelected(ConvertMode),
    ImageQualityChanged(u8),
    ImageTargetSizeChanged(String),
    ImageResizePercentSelected(u8),
    ImageCustomWidthToggled,
    ImageCustomWidthChanged(String),
    VideoFormatSelected(VideoTargetFormat),
    VideoModeSelected(ConvertMode),
    VideoPresetSelected(ConversionQualityPreset),
    VideoTargetSizeChanged(String),
    VideoResizePercentSelected(u8),
    VideoCustomWidthToggled,
    VideoCustomWidthChanged(String),
    VideoFpsSelected(Option<u32>),
    AudioFormatSelected(AudioTargetFormat),
    AudioModeSelected(ConvertMode),
    AudioPresetSelected(ConversionQualityPreset),
    AudioTargetSizeChanged(String),
    AudioChannelsSelected(AudioChannelSpec),
}

impl FileBrowser {
    pub(super) fn handle_convert_message(&mut self, message: ConvertMessage) -> Task<Message> {
        match message {
            ConvertMessage::OpenSelected => self.open_convert(),
            ConvertMessage::Submitted => self.submit_convert(),
            ConvertMessage::FfmpegProbed {
                available,
                encoders,
            } => {
                if let Some(state) = &mut self.convert {
                    state.accept_ffmpeg_probe(available, encoders);
                }
                Task::none()
            }
            ConvertMessage::ImageFormatSelected(target_format) => {
                self.update_convert_image(|options| options.target_format = target_format);
                Task::none()
            }
            ConvertMessage::ImageModeSelected(mode) => {
                self.update_convert_image(|options| options.mode = mode);
                Task::none()
            }
            ConvertMessage::ImagePresetSelected(preset) => {
                self.update_convert_image(|options| options.preset = preset);
                Task::none()
            }
            ConvertMessage::ImageQualityChanged(quality) => {
                self.update_convert_image(|options| options.quality = quality);
                Task::none()
            }
            ConvertMessage::ImageTargetSizeChanged(text) => {
                self.update_convert_image(|options| options.target_size_text = text);
                Task::none()
            }
            ConvertMessage::ImageResizePercentSelected(percent) => {
                self.update_convert_image(|options| {
                    options.resize.percent = percent;
                    options.resize.uses_custom_width = false;
                });
                Task::none()
            }
            ConvertMessage::ImageCustomWidthToggled => {
                self.update_convert_image(|options| {
                    options.resize.uses_custom_width = !options.resize.uses_custom_width;
                });
                Task::none()
            }
            ConvertMessage::ImageCustomWidthChanged(text) => {
                self.update_convert_image(|options| options.resize.custom_width_text = text);
                Task::none()
            }
            ConvertMessage::VideoFormatSelected(target_format) => {
                self.update_convert_video(|options| options.target_format = target_format);
                Task::none()
            }
            ConvertMessage::VideoModeSelected(mode) => {
                self.update_convert_video(|options| options.mode = mode);
                Task::none()
            }
            ConvertMessage::VideoPresetSelected(preset) => {
                self.update_convert_video(|options| options.preset = preset);
                Task::none()
            }
            ConvertMessage::VideoTargetSizeChanged(text) => {
                self.update_convert_video(|options| options.target_size_text = text);
                Task::none()
            }
            ConvertMessage::VideoResizePercentSelected(percent) => {
                self.update_convert_video(|options| {
                    options.resize.percent = percent;
                    options.resize.uses_custom_width = false;
                });
                Task::none()
            }
            ConvertMessage::VideoCustomWidthToggled => {
                self.update_convert_video(|options| {
                    options.resize.uses_custom_width = !options.resize.uses_custom_width;
                });
                Task::none()
            }
            ConvertMessage::VideoCustomWidthChanged(text) => {
                self.update_convert_video(|options| options.resize.custom_width_text = text);
                Task::none()
            }
            ConvertMessage::VideoFpsSelected(fps_override) => {
                self.update_convert_video(|options| options.fps_override = fps_override);
                Task::none()
            }
            ConvertMessage::AudioFormatSelected(target_format) => {
                self.update_convert_audio(|options| options.target_format = target_format);
                Task::none()
            }
            ConvertMessage::AudioModeSelected(mode) => {
                self.update_convert_audio(|options| options.mode = mode);
                Task::none()
            }
            ConvertMessage::AudioPresetSelected(preset) => {
                self.update_convert_audio(|options| options.preset = preset);
                Task::none()
            }
            ConvertMessage::AudioTargetSizeChanged(text) => {
                self.update_convert_audio(|options| options.target_size_text = text);
                Task::none()
            }
            ConvertMessage::AudioChannelsSelected(channels) => {
                self.update_convert_audio(|options| options.channels = channels);
                Task::none()
            }
        }
    }

    fn update_convert_image(&mut self, update: impl FnOnce(&mut ImageConvertOptions)) {
        if let Some(state) = &mut self.convert {
            if let Some(options) = &mut state.image {
                update(options);
                state.validation_error = None;
            }
        }
    }

    fn update_convert_video(&mut self, update: impl FnOnce(&mut VideoConvertOptions)) {
        if let Some(state) = &mut self.convert {
            if let Some(options) = &mut state.video {
                update(options);
                state.validation_error = None;
            }
        }
    }

    fn update_convert_audio(&mut self, update: impl FnOnce(&mut AudioConvertOptions)) {
        if let Some(state) = &mut self.convert {
            if let Some(options) = &mut state.audio {
                update(options);
                state.validation_error = None;
            }
        }
    }

    fn open_convert(&mut self) -> Task<Message> {
        if self.is_trash_view {
            return Task::none();
        }

        let mut state = ConvertState::default();
        for path in self.selected_paths_for_operation() {
            match convertible_media_kind(&path) {
                Some(ConvertibleMediaKind::Image) => {
                    state.image_sources.push(path);
                }
                Some(ConvertibleMediaKind::Video) => {
                    state.video_sources.push(path);
                }
                Some(ConvertibleMediaKind::Audio) => {
                    state.audio_sources.push(path);
                }
                None => {}
            }
        }
        state.image = (!state.image_sources.is_empty()).then(ImageConvertOptions::default);
        state.video = (!state.video_sources.is_empty()).then(VideoConvertOptions::default);
        state.audio = (!state.audio_sources.is_empty()).then(AudioConvertOptions::default);
        if state.image.is_none() && state.video.is_none() && state.audio.is_none() {
            return Task::none();
        }

        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();

        self.convert = Some(state);
        probe_media_tools_command()
    }

    fn submit_convert(&mut self) -> Task<Message> {
        let Some(state) = &mut self.convert else {
            return Task::none();
        };

        match state.build_requests() {
            Ok(requests) => {
                let operation = QueuedFileOperation::Convert { requests };
                self.convert = None;
                self.enqueue_file_operation(operation)
            }
            Err(error) => {
                state.validation_error = Some(error);
                Task::none()
            }
        }
    }
}

fn probe_media_tools_command() -> Task<Message> {
    Task::perform(available_conversion_encoders(), |encoders| {
        let available = !encoders.is_empty();
        Message::Convert(ConvertMessage::FfmpegProbed {
            available,
            encoders,
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConvertibleMediaKind {
    Image,
    Video,
    Audio,
}

/// 转换源按扩展名分类;SVG 是矢量源,引擎无法处理,不进入对话框。
fn convertible_media_kind(path: &Path) -> Option<ConvertibleMediaKind> {
    let is_svg = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"));
    if is_svg {
        return None;
    }
    match file_core::supported_media_kind_for_path(path) {
        Some(file_core::SupportedMediaKind::Image) => Some(ConvertibleMediaKind::Image),
        Some(file_core::SupportedMediaKind::Video) => Some(ConvertibleMediaKind::Video),
        Some(file_core::SupportedMediaKind::Audio) => Some(ConvertibleMediaKind::Audio),
        None => None,
    }
}

impl Default for ConvertState {
    fn default() -> Self {
        Self {
            image_sources: Vec::new(),
            video_sources: Vec::new(),
            audio_sources: Vec::new(),
            image: None,
            video: None,
            audio: None,
            ffmpeg_available: false,
            ffmpeg_probed: false,
            encoders: ConversionEncoderAvailability::unavailable(),
            validation_error: None,
        }
    }
}

impl ConvertState {
    fn accept_ffmpeg_probe(&mut self, available: bool, encoders: ConversionEncoderAvailability) {
        self.ffmpeg_available = available;
        self.ffmpeg_probed = true;
        // 探测可能发现当前选中的目标格式编码器缺失,回退到首个可用格式。
        if let Some(options) = &mut self.video {
            if !encoders.supports_video(options.target_format) {
                options.target_format = VIDEO_TARGET_FORMATS
                    .into_iter()
                    .find(|format| encoders.supports_video(*format))
                    .unwrap_or(options.target_format);
            }
        }
        if let Some(options) = &mut self.audio {
            if !encoders.supports_audio(options.target_format) {
                options.target_format = AUDIO_TARGET_FORMATS
                    .into_iter()
                    .find(|format| encoders.supports_audio(*format))
                    .unwrap_or(options.target_format);
            }
        }
        self.encoders = encoders;
    }

    pub(crate) fn source_count(&self) -> usize {
        self.image_sources.len() + self.video_sources.len() + self.audio_sources.len()
    }

    pub(crate) fn image_source_count(&self) -> usize {
        self.image_sources.len()
    }

    pub(crate) fn video_source_count(&self) -> usize {
        self.video_sources.len()
    }

    pub(crate) fn audio_source_count(&self) -> usize {
        self.audio_sources.len()
    }

    pub(crate) fn image(&self) -> Option<&ImageConvertOptions> {
        self.image.as_ref()
    }

    pub(crate) fn video(&self) -> Option<&VideoConvertOptions> {
        self.video.as_ref()
    }

    pub(crate) fn audio(&self) -> Option<&AudioConvertOptions> {
        self.audio.as_ref()
    }

    pub(crate) fn ffmpeg_available(&self) -> bool {
        self.ffmpeg_available
    }

    pub(crate) fn ffmpeg_probed(&self) -> bool {
        self.ffmpeg_probed
    }

    pub(crate) fn encoders(&self) -> &ConversionEncoderAvailability {
        &self.encoders
    }

    pub(crate) fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub(crate) fn can_submit(&self) -> bool {
        self.ffmpeg_probed && self.build_requests().is_ok()
    }

    fn build_requests(&self) -> Result<Vec<ConversionRequest>, String> {
        let mut requests = Vec::new();
        if let Some(options) = &self.image {
            if !self.encoders.supports_lossy_image(options.target_format) {
                return Err(FFMPEG_MISSING_MESSAGE.to_owned());
            }
            let quality = match options.target_format {
                ImageTargetFormat::Png
                | ImageTargetFormat::Tiff
                | ImageTargetFormat::Bmp
                | ImageTargetFormat::Ico => QualitySpec::Lossless,
                // GIF 调色板编码只有档位语义,固定走质量模式。
                ImageTargetFormat::Gif => QualitySpec::Preset(options.preset),
                ImageTargetFormat::Jpeg
                | ImageTargetFormat::Webp
                | ImageTargetFormat::Avif
                | ImageTargetFormat::Jxl => match options.mode {
                    ConvertMode::Quality => QualitySpec::Level(options.quality),
                    ConvertMode::TargetSize => {
                        QualitySpec::TargetBytes(parse_target_size_text(&options.target_size_text)?)
                    }
                },
            };
            for source in &self.image_sources {
                requests.push(ConversionRequest {
                    source: source.clone(),
                    target: ConversionTarget::Image(options.target_format),
                    quality,
                    resize: options.resize.to_resize_spec()?,
                    fps_override: None,
                    audio_channels: AudioChannelSpec::Keep,
                });
            }
        }
        if let Some(options) = &self.video {
            if !self.encoders.supports_video(options.target_format) {
                return Err(FFMPEG_MISSING_MESSAGE.to_owned());
            }
            // GIF 调色板编码只有档位语义,固定走质量模式。
            let quality = if options.target_format == VideoTargetFormat::Gif {
                QualitySpec::Preset(options.preset)
            } else {
                match options.mode {
                    ConvertMode::Quality => QualitySpec::Preset(options.preset),
                    ConvertMode::TargetSize => {
                        QualitySpec::TargetBytes(parse_target_size_text(&options.target_size_text)?)
                    }
                }
            };
            for source in &self.video_sources {
                requests.push(ConversionRequest {
                    source: source.clone(),
                    target: ConversionTarget::Video(options.target_format),
                    quality,
                    resize: options.resize.to_resize_spec()?,
                    fps_override: options.fps_override,
                    audio_channels: AudioChannelSpec::Keep,
                });
            }
        }
        if let Some(options) = &self.audio {
            if !self.encoders.supports_audio(options.target_format) {
                return Err(FFMPEG_MISSING_MESSAGE.to_owned());
            }
            let quality = match options.target_format {
                AudioTargetFormat::Flac | AudioTargetFormat::Wav => QualitySpec::Lossless,
                AudioTargetFormat::Mp3
                | AudioTargetFormat::M4a
                | AudioTargetFormat::Opus
                | AudioTargetFormat::Ogg => match options.mode {
                    ConvertMode::Quality => QualitySpec::Preset(options.preset),
                    ConvertMode::TargetSize => {
                        QualitySpec::TargetBytes(parse_target_size_text(&options.target_size_text)?)
                    }
                },
            };
            for source in &self.audio_sources {
                requests.push(ConversionRequest {
                    source: source.clone(),
                    target: ConversionTarget::Audio(options.target_format),
                    quality,
                    resize: ResizeSpec::Keep,
                    fps_override: None,
                    audio_channels: options.channels,
                });
            }
        }
        if requests.is_empty() {
            return Err("Select at least one convertible file.".to_owned());
        }
        Ok(requests)
    }
}

fn default_target_size_text() -> String {
    DEFAULT_TARGET_SIZE_TEXT.to_owned()
}

/// 体积文本解析:数字 + 可选 KB/MB/B 单位;无单位按 KB。
fn parse_target_size_text(text: &str) -> Result<u64, String> {
    let normalized = text.trim().to_uppercase().replace(' ', "");
    let (number_part, multiplier) = if let Some(number) = normalized.strip_suffix("MB") {
        (number, 1024 * 1024)
    } else if let Some(number) = normalized.strip_suffix("KB") {
        (number, 1024)
    } else if let Some(number) = normalized.strip_suffix('B') {
        (number, 1)
    } else {
        (normalized.as_str(), 1024)
    };
    number_part
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.saturating_mul(multiplier))
        .ok_or_else(|| "Enter a target size like 500KB or 2MB.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_size_with_units() {
        assert_eq!(parse_target_size_text("500KB").expect("kb"), 500 * 1024);
        assert_eq!(parse_target_size_text("2MB").expect("mb"), 2 * 1024 * 1024);
        assert_eq!(
            parse_target_size_text("2 MB").expect("spaced"),
            2 * 1024 * 1024
        );
        assert_eq!(parse_target_size_text("512b").expect("bytes"), 512);
        assert_eq!(parse_target_size_text("100").expect("bare"), 100 * 1024);
        assert!(parse_target_size_text("").is_err());
        assert!(parse_target_size_text("0KB").is_err());
        assert!(parse_target_size_text("abc").is_err());
    }

    #[test]
    fn resize_selection_maps_to_specs() {
        let selection = ResizeSelection::default();
        assert_eq!(selection.to_resize_spec(), Ok(ResizeSpec::Keep));

        let mut selection = ResizeSelection::default();
        selection.percent = 50;
        assert_eq!(selection.to_resize_spec(), Ok(ResizeSpec::Percent(50)));

        selection.uses_custom_width = true;
        selection.custom_width_text = "1920".to_owned();
        assert_eq!(selection.to_resize_spec(), Ok(ResizeSpec::Width(1920)));

        selection.custom_width_text = "0".to_owned();
        assert!(selection.to_resize_spec().is_err());
        selection.custom_width_text = "wide".to_owned();
        assert!(selection.to_resize_spec().is_err());
    }

    #[test]
    fn convertible_media_kind_skips_svg() {
        assert_eq!(
            convertible_media_kind(Path::new("/tmp/photo.png")),
            Some(ConvertibleMediaKind::Image)
        );
        assert_eq!(convertible_media_kind(Path::new("/tmp/logo.svg")), None);
        assert_eq!(convertible_media_kind(Path::new("/tmp/notes.txt")), None);
    }
}
