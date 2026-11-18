//! 转换对话框记住的设置在 `state.sqlite` 里的形态。格式、档位这些枚举存稳定的
//! 字符串键：file-core 改变体名不该悄悄改掉用户磁盘上的数据。

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionQualityPreset, ImageTargetFormat, PhotoMetadata,
    VideoTargetFormat,
};
use serde::{Deserialize, Serialize};

use super::unrecognized_settings::UnrecognizedSettings;
use crate::model::{
    AudioConvertOptions, ConvertDefaults, ImageConvertOptions, QualitySelection, ResizeChoice,
    ResizeSelection, SizeUnit, VideoClip, VideoConvertOptions, VideoTarget,
};

const IMAGE_FORMAT_KEYS: [(ImageTargetFormat, &str); 9] = [
    (ImageTargetFormat::Jpeg, "jpeg"),
    (ImageTargetFormat::Png, "png"),
    (ImageTargetFormat::Webp, "webp"),
    (ImageTargetFormat::Avif, "avif"),
    (ImageTargetFormat::Jxl, "jxl"),
    (ImageTargetFormat::Gif, "gif"),
    (ImageTargetFormat::Tiff, "tiff"),
    (ImageTargetFormat::Bmp, "bmp"),
    (ImageTargetFormat::Ico, "ico"),
];
const VIDEO_FORMAT_KEYS: [(VideoTargetFormat, &str); 9] = [
    (VideoTargetFormat::Mp4H264, "mp4_h264"),
    (VideoTargetFormat::Mp4H265, "mp4_h265"),
    (VideoTargetFormat::MkvH265, "mkv_h265"),
    (VideoTargetFormat::MkvAv1, "mkv_av1"),
    (VideoTargetFormat::WebmVp9, "webm_vp9"),
    (VideoTargetFormat::WebmAv1, "webm_av1"),
    (VideoTargetFormat::MovH264, "mov_h264"),
    (VideoTargetFormat::AviMpeg4, "avi_mpeg4"),
    (VideoTargetFormat::Gif, "gif"),
];
const AUDIO_FORMAT_KEYS: [(AudioTargetFormat, &str); 6] = [
    (AudioTargetFormat::Mp3, "mp3"),
    (AudioTargetFormat::M4a, "m4a"),
    (AudioTargetFormat::Opus, "opus"),
    (AudioTargetFormat::Ogg, "ogg"),
    (AudioTargetFormat::Flac, "flac"),
    (AudioTargetFormat::Wav, "wav"),
];
const PRESET_KEYS: [(ConversionQualityPreset, &str); 3] = [
    (ConversionQualityPreset::Low, "low"),
    (ConversionQualityPreset::Medium, "medium"),
    (ConversionQualityPreset::High, "high"),
];
const PHOTO_METADATA_KEYS: [(PhotoMetadata, &str); 2] = [
    (PhotoMetadata::Keep, "keep"),
    (PhotoMetadata::Strip, "strip"),
];
const SIZE_UNIT_KEYS: [(SizeUnit, &str); 3] = [
    (SizeUnit::KiB, "kib"),
    (SizeUnit::MiB, "mib"),
    (SizeUnit::GiB, "gib"),
];
const CHANNEL_KEYS: [(AudioChannelSpec, &str); 2] = [
    (AudioChannelSpec::Keep, "keep"),
    (AudioChannelSpec::Mono, "mono"),
];
const UNRECOGNIZED_KEY: &str = "convert_defaults";
/// 视频卡片“只要声音”的目标存 `audio_<音频格式键>`，和视频格式键不会撞。
const SOUND_ONLY_PREFIX: &str = "audio_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredConvertDefaults {
    pub image: StoredImageConvert,
    pub video: StoredVideoConvert,
    pub audio: StoredAudioConvert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredImageConvert {
    pub format: String,
    pub quality: StoredQuality,
    pub resize: StoredResize,
    /// 照片信息;None = 早先没存过,按保留。
    #[serde(default)]
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredVideoConvert {
    pub format: String,
    pub quality: StoredQuality,
    pub resize: StoredResize,
    pub fps: Option<u32>,
    /// 只要声音时的声道;None = 早先没存过,按保持。
    #[serde(default)]
    pub channels: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredAudioConvert {
    pub format: String,
    pub quality: StoredQuality,
    pub channels: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredQuality {
    pub preset: String,
    pub by_target_size: bool,
    /// 目标大小的数字部分。
    pub target_size: String,
    /// None = 早先没存过,按 MiB。
    #[serde(default)]
    pub target_size_unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredResize {
    pub choice: StoredResizeChoice,
    pub long_edge: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredResizeChoice {
    Percent(u8),
    LongEdge,
}

impl StoredConvertDefaults {
    pub(crate) fn from_defaults(defaults: &ConvertDefaults) -> Self {
        let ConvertDefaults {
            image,
            video,
            audio,
        } = defaults;
        Self {
            image: StoredImageConvert {
                format: key_of(&IMAGE_FORMAT_KEYS, image.target_format).to_owned(),
                quality: StoredQuality::from_selection(&image.quality),
                resize: StoredResize::from_selection(&image.resize),
                metadata: Some(key_of(&PHOTO_METADATA_KEYS, image.metadata).to_owned()),
            },
            video: StoredVideoConvert {
                format: video_target_key(video.target),
                quality: StoredQuality::from_selection(&video.quality),
                resize: StoredResize::from_selection(&video.resize),
                fps: video.fps_override,
                channels: Some(key_of(&CHANNEL_KEYS, video.channels).to_owned()),
            },
            audio: StoredAudioConvert {
                format: key_of(&AUDIO_FORMAT_KEYS, audio.target_format).to_owned(),
                quality: StoredQuality::from_selection(&audio.quality),
                channels: key_of(&CHANNEL_KEYS, audio.channels).to_owned(),
            },
        }
    }

    /// 认不出的键换成默认值，并记为读不懂的设置。
    pub(crate) fn to_defaults(&self, unrecognized: &mut UnrecognizedSettings) -> ConvertDefaults {
        let fallback = ConvertDefaults::default();
        let mut keys = KeyReader { unrecognized };
        ConvertDefaults {
            image: ImageConvertOptions {
                target_format: keys.read(
                    &IMAGE_FORMAT_KEYS,
                    &self.image.format,
                    fallback.image.target_format,
                ),
                quality: self.image.quality.to_selection(&mut keys),
                resize: self.image.resize.to_selection(),
                metadata: match &self.image.metadata {
                    Some(metadata) => {
                        keys.read(&PHOTO_METADATA_KEYS, metadata, fallback.image.metadata)
                    }
                    None => fallback.image.metadata,
                },
            },
            video: VideoConvertOptions {
                target: keys.recognize(
                    video_target_from_key(&self.video.format),
                    fallback.video.target,
                ),
                quality: self.video.quality.to_selection(&mut keys),
                resize: self.video.resize.to_selection(),
                fps_override: self.video.fps.filter(|fps| *fps > 0),
                channels: match &self.video.channels {
                    Some(channels) => keys.read(&CHANNEL_KEYS, channels, fallback.video.channels),
                    None => fallback.video.channels,
                },
                clip: VideoClip::default(),
            },
            audio: AudioConvertOptions {
                target_format: keys.read(
                    &AUDIO_FORMAT_KEYS,
                    &self.audio.format,
                    fallback.audio.target_format,
                ),
                quality: self.audio.quality.to_selection(&mut keys),
                channels: keys.read(&CHANNEL_KEYS, &self.audio.channels, fallback.audio.channels),
            },
        }
    }
}

impl StoredQuality {
    fn from_selection(quality: &QualitySelection) -> Self {
        Self {
            preset: key_of(&PRESET_KEYS, quality.preset).to_owned(),
            by_target_size: quality.by_target_size,
            target_size: quality.target_size_text.clone(),
            target_size_unit: Some(key_of(&SIZE_UNIT_KEYS, quality.target_size_unit).to_owned()),
        }
    }

    fn to_selection(&self, keys: &mut KeyReader<'_>) -> QualitySelection {
        let fallback = QualitySelection::default();
        QualitySelection {
            preset: keys.read(&PRESET_KEYS, &self.preset, fallback.preset),
            by_target_size: self.by_target_size,
            target_size_text: self.target_size.clone(),
            target_size_unit: match &self.target_size_unit {
                Some(unit) => keys.read(&SIZE_UNIT_KEYS, unit, fallback.target_size_unit),
                None => fallback.target_size_unit,
            },
        }
    }
}

impl StoredResize {
    fn from_selection(resize: &ResizeSelection) -> Self {
        Self {
            choice: match resize.choice {
                ResizeChoice::Percent(percent) => StoredResizeChoice::Percent(percent),
                ResizeChoice::LongEdge => StoredResizeChoice::LongEdge,
            },
            long_edge: resize.long_edge_text.clone(),
        }
    }

    fn to_selection(&self) -> ResizeSelection {
        ResizeSelection {
            choice: match self.choice {
                // 只有 1-100 有意义；越界按原尺寸。
                StoredResizeChoice::Percent(percent) if (1..=100).contains(&percent) => {
                    ResizeChoice::Percent(percent)
                }
                StoredResizeChoice::Percent(_) => ResizeChoice::Percent(100),
                StoredResizeChoice::LongEdge => ResizeChoice::LongEdge,
            },
            long_edge_text: self.long_edge.clone(),
        }
    }
}

struct KeyReader<'a> {
    unrecognized: &'a mut UnrecognizedSettings,
}

impl KeyReader<'_> {
    fn read<T: Copy>(&mut self, table: &[(T, &str)], key: &str, fallback: T) -> T {
        self.recognize(value_of(table, key), fallback)
    }

    fn recognize<T>(&mut self, value: Option<T>, fallback: T) -> T {
        self.unrecognized
            .value_or_default(UNRECOGNIZED_KEY, value, fallback)
    }
}

fn video_target_key(target: VideoTarget) -> String {
    match target {
        VideoTarget::Video(format) => key_of(&VIDEO_FORMAT_KEYS, format).to_owned(),
        VideoTarget::Audio(format) => {
            format!("{SOUND_ONLY_PREFIX}{}", key_of(&AUDIO_FORMAT_KEYS, format))
        }
    }
}

fn video_target_from_key(key: &str) -> Option<VideoTarget> {
    match key.strip_prefix(SOUND_ONLY_PREFIX) {
        Some(audio_key) => value_of(&AUDIO_FORMAT_KEYS, audio_key).map(VideoTarget::Audio),
        None => value_of(&VIDEO_FORMAT_KEYS, key).map(VideoTarget::Video),
    }
}

fn value_of<T: Copy>(table: &[(T, &str)], key: &str) -> Option<T> {
    table
        .iter()
        .find(|(_, candidate)| *candidate == key)
        .map(|(value, _)| *value)
}

fn key_of<T: Copy + PartialEq>(table: &[(T, &'static str)], value: T) -> &'static str {
    table
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, key)| *key)
        .expect("every variant has a stored key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_unique_stored_key() {
        fn assert_unique<T>(table: &[(T, &str)]) {
            let mut keys: Vec<_> = table.iter().map(|(_, key)| *key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), table.len());
        }
        assert_unique(&IMAGE_FORMAT_KEYS);
        assert_unique(&VIDEO_FORMAT_KEYS);
        assert_unique(&AUDIO_FORMAT_KEYS);
        assert_unique(&PRESET_KEYS);
        assert_unique(&CHANNEL_KEYS);
        assert_unique(&PHOTO_METADATA_KEYS);
        assert_unique(&SIZE_UNIT_KEYS);
        assert_eq!(
            IMAGE_FORMAT_KEYS.len(),
            crate::model::IMAGE_TARGET_FORMATS.len()
        );
        assert_eq!(
            VIDEO_FORMAT_KEYS.len(),
            VideoTarget::all()
                .filter(|target| matches!(target, VideoTarget::Video(_)))
                .count()
        );
        assert_eq!(
            AUDIO_FORMAT_KEYS.len(),
            crate::model::AUDIO_TARGET_FORMATS.len()
        );
    }

    #[test]
    fn stored_defaults_round_trip() {
        let mut defaults = ConvertDefaults::default();
        defaults.image.target_format = ImageTargetFormat::Avif;
        defaults.image.resize.choice = ResizeChoice::LongEdge;
        defaults.image.resize.long_edge_text = "1280".to_owned();
        defaults.image.metadata = PhotoMetadata::Strip;
        defaults.video.target = VideoTarget::Audio(AudioTargetFormat::Flac);
        defaults.video.channels = AudioChannelSpec::Mono;
        defaults.video.quality.by_target_size = true;
        defaults.video.quality.target_size_text = "8".to_owned();
        defaults.video.quality.target_size_unit = SizeUnit::GiB;
        defaults.video.fps_override = Some(30);
        defaults.audio.target_format = AudioTargetFormat::Opus;
        defaults.audio.quality.preset = ConversionQualityPreset::Low;
        defaults.audio.channels = AudioChannelSpec::Mono;

        let json = serde_json::to_string(&StoredConvertDefaults::from_defaults(&defaults))
            .expect("serialize");
        let stored: StoredConvertDefaults = serde_json::from_str(&json).expect("deserialize");
        let mut unrecognized = UnrecognizedSettings::default();
        assert_eq!(stored.to_defaults(&mut unrecognized), defaults);
        assert!(unrecognized.into_keys().is_empty());

        defaults.video.target = VideoTarget::Video(VideoTargetFormat::WebmVp9);
        let stored = StoredConvertDefaults::from_defaults(&defaults);
        assert_eq!(stored.video.format, "webm_vp9");
        let mut unrecognized = UnrecognizedSettings::default();
        assert_eq!(stored.to_defaults(&mut unrecognized), defaults);
    }

    #[test]
    fn one_off_clip_edits_are_not_stored() {
        let mut defaults = ConvertDefaults::default();
        defaults.video.clip.trim_start_text = "0:10".to_owned();
        let stored = StoredConvertDefaults::from_defaults(&defaults);
        let mut unrecognized = UnrecognizedSettings::default();
        assert_eq!(
            stored.to_defaults(&mut unrecognized).video.clip,
            VideoClip::default()
        );
    }

    #[test]
    fn video_defaults_stored_before_sound_only_targets_still_load() {
        let json = r#"{"format":"mp4_h264","quality":{"preset":"medium","by_target_size":false,"target_size":"1 MiB"},"resize":{"choice":{"percent":100},"long_edge":"1920"},"fps":null}"#;
        let stored: StoredVideoConvert = serde_json::from_str(json).expect("older shape");
        assert_eq!(stored.channels, None);
    }

    #[test]
    fn unknown_keys_fall_back_and_are_reported() {
        let mut stored = StoredConvertDefaults::from_defaults(&ConvertDefaults::default());
        stored.image.format = "heif".to_owned();
        stored.video.resize.choice = StoredResizeChoice::Percent(0);
        stored.audio.quality.preset = "ultra".to_owned();

        let mut unrecognized = UnrecognizedSettings::default();
        assert_eq!(
            stored.to_defaults(&mut unrecognized),
            ConvertDefaults::default()
        );
        assert_eq!(unrecognized.into_keys(), vec![UNRECOGNIZED_KEY]);
    }
}
