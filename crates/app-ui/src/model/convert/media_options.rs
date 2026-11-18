//! 三类媒体各自的选项：格式表、编辑、编码器回退、场景候选、交给引擎的设置。

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionTarget, ImageTargetFormat, ResizeSpec, VideoTargetFormat,
};

use super::{
    audio_quality_control, image_quality_control, video_quality_control, ConversionSettings,
    ConvertInputError, ConvertScene, QualityChoice, QualityControl, QualitySelection, ResizeChoice,
    ResizeSelection, AUDIO_TARGET_FORMATS, IMAGE_TARGET_FORMATS, VIDEO_TARGET_FORMATS,
    WEB_LONG_EDGE,
};

/// 三类媒体选项的共同部分：质量一行、编辑、格式回退、场景、产出设置。
pub(crate) trait MediaOptions: Clone + PartialEq {
    type Edit;
    fn quality(&self) -> &QualitySelection;
    fn quality_mut(&mut self) -> &mut QualitySelection;
    fn quality_control(&self) -> QualityControl;
    fn apply(&mut self, edit: Self::Edit);
    /// 当前格式在这台机器上编不了时，退到第一个能编的。
    fn fall_back_to_available(&mut self, encoders: &ConversionEncoderAvailability);
    /// 套用场景后的选项；这台机器一个候选格式都编不了时为 None。
    fn with_scene(
        &self,
        scene: ConvertScene,
        encoders: &ConversionEncoderAvailability,
    ) -> Option<Self>;
    fn settings(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<ConversionSettings, ConvertInputError>;
}

/// 每类选项就是「记住的设置」：提交时整份存进用户配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageConvertOptions {
    pub(crate) target_format: ImageTargetFormat,
    pub(crate) quality: QualitySelection,
    pub(crate) resize: ResizeSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoConvertOptions {
    pub(crate) target_format: VideoTargetFormat,
    pub(crate) quality: QualitySelection,
    pub(crate) resize: ResizeSelection,
    pub(crate) fps_override: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioConvertOptions {
    pub(crate) target_format: AudioTargetFormat,
    pub(crate) quality: QualitySelection,
    pub(crate) channels: AudioChannelSpec,
}

impl Default for ImageConvertOptions {
    fn default() -> Self {
        Self {
            target_format: ImageTargetFormat::Jpeg,
            quality: QualitySelection::default(),
            resize: ResizeSelection::default(),
        }
    }
}

impl Default for VideoConvertOptions {
    fn default() -> Self {
        Self {
            target_format: VideoTargetFormat::Mp4H264,
            quality: QualitySelection::default(),
            resize: ResizeSelection::default(),
            fps_override: None,
        }
    }
}

impl Default for AudioConvertOptions {
    fn default() -> Self {
        Self {
            target_format: AudioTargetFormat::Mp3,
            quality: QualitySelection::default(),
            channels: AudioChannelSpec::Keep,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImageEdit {
    Format(ImageTargetFormat),
    Resize(ResizeChoice),
    LongEdge(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoEdit {
    Format(VideoTargetFormat),
    Resize(ResizeChoice),
    LongEdge(String),
    Fps(Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AudioEdit {
    Format(AudioTargetFormat),
    Channels(AudioChannelSpec),
}

impl MediaOptions for ImageConvertOptions {
    type Edit = ImageEdit;

    fn quality(&self) -> &QualitySelection {
        &self.quality
    }

    fn quality_mut(&mut self) -> &mut QualitySelection {
        &mut self.quality
    }

    fn quality_control(&self) -> QualityControl {
        image_quality_control(self.target_format)
    }

    fn apply(&mut self, edit: ImageEdit) {
        match edit {
            ImageEdit::Format(format) => self.target_format = format,
            ImageEdit::Resize(choice) => self.resize.choice = choice,
            ImageEdit::LongEdge(text) => self.resize.long_edge_text = text,
        }
    }

    fn fall_back_to_available(&mut self, encoders: &ConversionEncoderAvailability) {
        self.target_format = first_available(self.target_format, &IMAGE_TARGET_FORMATS, |format| {
            encoders.supports_lossy_image(format)
        });
    }

    fn with_scene(
        &self,
        scene: ConvertScene,
        encoders: &ConversionEncoderAvailability,
    ) -> Option<Self> {
        use ConversionQualityPreset::{High, Low, Medium};
        use ImageTargetFormat::{Avif, Jpeg, Webp};
        let web = ResizeSpec::LongEdge(WEB_LONG_EDGE);
        let candidates: &[(ImageTargetFormat, ConversionQualityPreset, ResizeSpec)] = match scene {
            ConvertScene::MostCompatible => &[(Jpeg, High, ResizeSpec::Keep)],
            ConvertScene::ForWeb => &[(Webp, Medium, web), (Jpeg, Medium, web)],
            ConvertScene::Smallest => &[
                (Avif, Medium, ResizeSpec::Keep),
                (Webp, Medium, ResizeSpec::Keep),
                (Jpeg, Low, ResizeSpec::Keep),
            ],
        };
        let &(format, preset, resize) = candidates
            .iter()
            .find(|(format, ..)| encoders.supports_lossy_image(*format))?;
        let mut options = self.clone();
        options.target_format = format;
        options.quality.select(QualityChoice::Preset(preset));
        options.resize.set_spec(resize);
        Some(options)
    }

    fn settings(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<ConversionSettings, ConvertInputError> {
        if !encoders.supports_lossy_image(self.target_format) {
            return Err(ConvertInputError::MissingEncoder);
        }
        Ok(ConversionSettings {
            target: ConversionTarget::Image(self.target_format),
            quality: self.quality.spec(self.quality_control())?,
            resize: self.resize.spec()?,
            fps_override: None,
            audio_channels: AudioChannelSpec::Keep,
        })
    }
}

impl MediaOptions for VideoConvertOptions {
    type Edit = VideoEdit;

    fn quality(&self) -> &QualitySelection {
        &self.quality
    }

    fn quality_mut(&mut self) -> &mut QualitySelection {
        &mut self.quality
    }

    fn quality_control(&self) -> QualityControl {
        video_quality_control(self.target_format)
    }

    fn apply(&mut self, edit: VideoEdit) {
        match edit {
            VideoEdit::Format(format) => self.target_format = format,
            VideoEdit::Resize(choice) => self.resize.choice = choice,
            VideoEdit::LongEdge(text) => self.resize.long_edge_text = text,
            VideoEdit::Fps(fps) => self.fps_override = fps,
        }
    }

    fn fall_back_to_available(&mut self, encoders: &ConversionEncoderAvailability) {
        self.target_format = first_available(self.target_format, &VIDEO_TARGET_FORMATS, |format| {
            encoders.supports_video(format)
        });
    }

    fn with_scene(
        &self,
        scene: ConvertScene,
        encoders: &ConversionEncoderAvailability,
    ) -> Option<Self> {
        use ConversionQualityPreset::{Low, Medium};
        use VideoTargetFormat::{Mp4H264, Mp4H265};
        let candidates: &[(VideoTargetFormat, ConversionQualityPreset, ResizeSpec)] = match scene {
            ConvertScene::MostCompatible => &[(Mp4H264, Medium, ResizeSpec::Keep)],
            ConvertScene::ForWeb => &[(Mp4H264, Medium, ResizeSpec::LongEdge(WEB_LONG_EDGE))],
            ConvertScene::Smallest => &[
                (Mp4H265, Medium, ResizeSpec::Keep),
                (Mp4H264, Low, ResizeSpec::Keep),
            ],
        };
        let &(format, preset, resize) = candidates
            .iter()
            .find(|(format, ..)| encoders.supports_video(*format))?;
        let mut options = self.clone();
        options.target_format = format;
        options.quality.select(QualityChoice::Preset(preset));
        options.resize.set_spec(resize);
        Some(options)
    }

    fn settings(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<ConversionSettings, ConvertInputError> {
        if !encoders.supports_video(self.target_format) {
            return Err(ConvertInputError::MissingEncoder);
        }
        Ok(ConversionSettings {
            target: ConversionTarget::Video(self.target_format),
            quality: self.quality.spec(self.quality_control())?,
            resize: self.resize.spec()?,
            fps_override: self.fps_override,
            audio_channels: AudioChannelSpec::Keep,
        })
    }
}

impl MediaOptions for AudioConvertOptions {
    type Edit = AudioEdit;

    fn quality(&self) -> &QualitySelection {
        &self.quality
    }

    fn quality_mut(&mut self) -> &mut QualitySelection {
        &mut self.quality
    }

    fn quality_control(&self) -> QualityControl {
        audio_quality_control(self.target_format)
    }

    fn apply(&mut self, edit: AudioEdit) {
        match edit {
            AudioEdit::Format(format) => self.target_format = format,
            AudioEdit::Channels(channels) => self.channels = channels,
        }
    }

    fn fall_back_to_available(&mut self, encoders: &ConversionEncoderAvailability) {
        self.target_format = first_available(self.target_format, &AUDIO_TARGET_FORMATS, |format| {
            encoders.supports_audio(format)
        });
    }

    fn with_scene(
        &self,
        scene: ConvertScene,
        encoders: &ConversionEncoderAvailability,
    ) -> Option<Self> {
        use AudioTargetFormat::{M4a, Mp3, Opus};
        use ConversionQualityPreset::{High, Low, Medium};
        let candidates: &[(AudioTargetFormat, ConversionQualityPreset)] = match scene {
            ConvertScene::MostCompatible => &[(Mp3, High)],
            ConvertScene::ForWeb => &[(M4a, Medium)],
            ConvertScene::Smallest => &[(Opus, Low), (Mp3, Low)],
        };
        let &(format, preset) = candidates
            .iter()
            .find(|(format, _)| encoders.supports_audio(*format))?;
        let mut options = self.clone();
        options.target_format = format;
        options.quality.select(QualityChoice::Preset(preset));
        Some(options)
    }

    fn settings(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<ConversionSettings, ConvertInputError> {
        if !encoders.supports_audio(self.target_format) {
            return Err(ConvertInputError::MissingEncoder);
        }
        Ok(ConversionSettings {
            target: ConversionTarget::Audio(self.target_format),
            quality: self.quality.spec(self.quality_control())?,
            resize: ResizeSpec::Keep,
            fps_override: None,
            audio_channels: self.channels,
        })
    }
}

fn first_available<F: Copy>(current: F, formats: &[F], available: impl Fn(F) -> bool) -> F {
    if available(current) {
        return current;
    }
    formats
        .iter()
        .copied()
        .find(|format| available(*format))
        .unwrap_or(current)
}
