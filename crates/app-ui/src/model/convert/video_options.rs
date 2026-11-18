//! 视频卡片的选项：可以转成另一种视频，也可以只要声音；外加一次性的剪辑。

use std::time::Duration;

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionTarget, MediaEdits, PhotoMetadata, ResizeSpec, TrimRange, VideoRotation, VideoSound,
    VideoTargetFormat,
};

use super::media_options::MediaOptions;
use super::{
    audio_quality_control, first_available, video_quality_control, ConversionSettings,
    ConvertInputError, ConvertScene, QualityChoice, QualityControl, QualitySelection, ResizeChoice,
    ResizeSelection, AUDIO_TARGET_FORMATS, VIDEO_TARGET_FORMATS, WEB_LONG_EDGE,
};

/// 视频卡片的目标格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoTarget {
    Video(VideoTargetFormat),
    /// 只要声音：从视频里抽出音轨。
    Audio(AudioTargetFormat),
}

impl VideoTarget {
    /// 下拉里的顺序：视频格式在前，只要声音的在后。
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        VIDEO_TARGET_FORMATS
            .into_iter()
            .map(Self::Video)
            .chain(AUDIO_TARGET_FORMATS.into_iter().map(Self::Audio))
    }

    fn is_same_kind(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Video(_), Self::Video(_)) | (Self::Audio(_), Self::Audio(_))
        )
    }

    pub(crate) fn is_available(self, encoders: &ConversionEncoderAvailability) -> bool {
        match self {
            Self::Video(format) => encoders.supports_video(format),
            Self::Audio(format) => encoders.supports_audio(format),
        }
    }
}

/// 只对这一次有效的剪辑，不记住：下一批视频多半不需要同样的旋转和截取。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VideoClip {
    pub(crate) rotation: VideoRotation,
    pub(crate) sound: VideoSound,
    pub(crate) trim_start_text: String,
    pub(crate) trim_end_text: String,
}

impl VideoClip {
    fn trim(&self) -> Result<TrimRange, ConvertInputError> {
        let trim = TrimRange {
            start: parse_clip_time(&self.trim_start_text)?,
            end: parse_clip_time(&self.trim_end_text)?,
        };
        match (trim.start, trim.end) {
            (Some(start), Some(end)) if end <= start => Err(ConvertInputError::Trim),
            _ => Ok(trim),
        }
    }
}

/// `秒`、`分:秒`、`时:分:秒`，秒可带小数；空 = 不截这一端。
/// 有上一级时，分和秒都得小于 60。
fn parse_clip_time(text: &str) -> Result<Option<Duration>, ConvertInputError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(':').collect();
    let (seconds_text, larger_units) = parts.split_last().ok_or(ConvertInputError::Trim)?;
    if larger_units.len() > 2 {
        return Err(ConvertInputError::Trim);
    }
    let seconds = seconds_text
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .filter(|seconds| larger_units.is_empty() || *seconds < 60.0)
        .ok_or(ConvertInputError::Trim)?;
    let mut total = seconds;
    for (index, unit_text) in larger_units.iter().rev().enumerate() {
        let value = unit_text
            .parse::<u32>()
            .map_err(|_| ConvertInputError::Trim)?;
        let is_minutes_under_hours = index == 0 && larger_units.len() == 2;
        if is_minutes_under_hours && value >= 60 {
            return Err(ConvertInputError::Trim);
        }
        total += f64::from(value) * 60f64.powi(index as i32 + 1);
    }
    Ok(Some(Duration::from_secs_f64(total)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoConvertOptions {
    pub(crate) target: VideoTarget,
    pub(crate) quality: QualitySelection,
    pub(crate) resize: ResizeSelection,
    pub(crate) fps_override: Option<u32>,
    /// 只要声音时用。
    pub(crate) channels: AudioChannelSpec,
    pub(crate) clip: VideoClip,
}

impl Default for VideoConvertOptions {
    fn default() -> Self {
        Self {
            target: VideoTarget::Video(VideoTargetFormat::Mp4H264),
            quality: QualitySelection::default(),
            resize: ResizeSelection::default(),
            fps_override: None,
            channels: AudioChannelSpec::Keep,
            clip: VideoClip::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoEdit {
    Target(VideoTarget),
    Resize(ResizeChoice),
    LongEdge(String),
    Fps(Option<u32>),
    Channels(AudioChannelSpec),
    Rotation(VideoRotation),
    Sound(VideoSound),
    TrimStart(String),
    TrimEnd(String),
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
        match self.target {
            VideoTarget::Video(format) => video_quality_control(format),
            VideoTarget::Audio(format) => audio_quality_control(format),
        }
    }

    fn apply(&mut self, edit: VideoEdit) {
        match edit {
            VideoEdit::Target(target) => self.target = target,
            VideoEdit::Resize(choice) => self.resize.choice = choice,
            VideoEdit::LongEdge(text) => self.resize.long_edge_text = text,
            VideoEdit::Fps(fps) => self.fps_override = fps,
            VideoEdit::Channels(channels) => self.channels = channels,
            VideoEdit::Rotation(rotation) => self.clip.rotation = rotation,
            VideoEdit::Sound(sound) => self.clip.sound = sound,
            VideoEdit::TrimStart(text) => self.clip.trim_start_text = text,
            VideoEdit::TrimEnd(text) => self.clip.trim_end_text = text,
        }
    }

    /// 先在同一种里找（记住的是“只要声音”就退到别的音频格式），找不到再换另一种。
    fn fall_back_to_available(&mut self, encoders: &ConversionEncoderAvailability) {
        let mut targets: Vec<VideoTarget> = VideoTarget::all().collect();
        targets.sort_by_key(|target| !self.target.is_same_kind(*target));
        self.target = first_available(self.target, &targets, |target| {
            target.is_available(encoders)
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
        options.target = VideoTarget::Video(format);
        options.quality.select(QualityChoice::Preset(preset));
        options.resize.set_spec(resize);
        Some(options)
    }

    fn settings(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<ConversionSettings, ConvertInputError> {
        if !self.target.is_available(encoders) {
            return Err(ConvertInputError::MissingEncoder);
        }
        let quality = self.quality.spec(self.quality_control())?;
        let trim = self.clip.trim()?;
        Ok(match self.target {
            VideoTarget::Video(format) => ConversionSettings {
                target: ConversionTarget::Video(format),
                quality,
                resize: self.resize.spec()?,
                fps_override: self.fps_override,
                audio_channels: AudioChannelSpec::Keep,
                edits: MediaEdits {
                    trim,
                    rotation: self.clip.rotation,
                    sound: self.clip.sound,
                },
                photo_metadata: PhotoMetadata::Keep,
            },
            // 只要声音：画面上的缩放、帧率、旋转都用不上。
            VideoTarget::Audio(format) => ConversionSettings {
                target: ConversionTarget::Audio(format),
                quality,
                resize: ResizeSpec::Keep,
                fps_override: None,
                audio_channels: self.channels,
                edits: MediaEdits {
                    trim,
                    ..MediaEdits::default()
                },
                photo_metadata: PhotoMetadata::Keep,
            },
        })
    }

    fn remembered(&self) -> Self {
        Self {
            clip: VideoClip::default(),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: f64) -> Result<Option<Duration>, ConvertInputError> {
        Ok(Some(Duration::from_secs_f64(value)))
    }

    #[test]
    fn clip_times_read_seconds_minutes_and_hours() {
        assert_eq!(parse_clip_time(""), Ok(None));
        assert_eq!(parse_clip_time("  "), Ok(None));
        assert_eq!(parse_clip_time("90"), seconds(90.0));
        assert_eq!(parse_clip_time("1:30"), seconds(90.0));
        assert_eq!(parse_clip_time("75:00"), seconds(4500.0));
        assert_eq!(parse_clip_time("1:02:03.5"), seconds(3723.5));
        for invalid in ["1:60", "1:60:00", "abc", "1::2", "-5", "1:2:3:4", "1.5:00"] {
            assert_eq!(
                parse_clip_time(invalid),
                Err(ConvertInputError::Trim),
                "{invalid}"
            );
        }
    }

    #[test]
    fn trim_end_must_come_after_its_start() {
        let mut clip = VideoClip {
            trim_start_text: "0:10".to_owned(),
            trim_end_text: "0:20".to_owned(),
            ..VideoClip::default()
        };
        assert_eq!(
            clip.trim(),
            Ok(TrimRange {
                start: Some(Duration::from_secs(10)),
                end: Some(Duration::from_secs(20)),
            })
        );
        clip.trim_end_text = "0:10".to_owned();
        assert_eq!(clip.trim(), Err(ConvertInputError::Trim));
        clip.trim_start_text.clear();
        assert!(clip.trim().is_ok());
    }
}
