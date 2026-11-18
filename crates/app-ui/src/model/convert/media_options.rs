//! 图片、音频的选项和三类共用的 `MediaOptions`；视频的在 `video_options.rs`。

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionTarget, ImageTargetFormat, MediaEdits, ResizeSpec,
};

use super::{
    audio_quality_control, first_available, image_quality_control, ConversionSettings,
    ConvertInputError, ConvertScene, QualityChoice, QualityControl, QualitySelection, ResizeChoice,
    ResizeSelection, AUDIO_TARGET_FORMATS, IMAGE_TARGET_FORMATS, WEB_LONG_EDGE,
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
    /// 提交时存进用户配置的那部分；一次性的剪辑（视频的旋转、截取……）不记。
    fn remembered(&self) -> Self {
        self.clone()
    }
}

/// 图片、音频的选项全都要记住。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageConvertOptions {
    pub(crate) target_format: ImageTargetFormat,
    pub(crate) quality: QualitySelection,
    pub(crate) resize: ResizeSelection,
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
            edits: MediaEdits::default(),
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
            edits: MediaEdits::default(),
        })
    }
}
