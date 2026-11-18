//! 转换对话框记住的设置在 `state.sqlite` 里的形态。格式、档位这些枚举存稳定的
//! 字符串键：file-core 改变体名不该悄悄改掉用户磁盘上的数据。

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionQualityPreset, ImageTargetFormat,
    VideoTargetFormat,
};
use serde::{Deserialize, Serialize};

use super::unrecognized_settings::UnrecognizedSettings;
use crate::model::{
    AudioConvertOptions, ConvertDefaults, ImageConvertOptions, QualitySelection, ResizeChoice,
    ResizeSelection, VideoConvertOptions,
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
const CHANNEL_KEYS: [(AudioChannelSpec, &str); 2] = [
    (AudioChannelSpec::Keep, "keep"),
    (AudioChannelSpec::Mono, "mono"),
];
const UNRECOGNIZED_KEY: &str = "convert_defaults";

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredVideoConvert {
    pub format: String,
    pub quality: StoredQuality,
    pub resize: StoredResize,
    pub fps: Option<u32>,
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
    pub target_size: String,
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
            },
            video: StoredVideoConvert {
                format: key_of(&VIDEO_FORMAT_KEYS, video.target_format).to_owned(),
                quality: StoredQuality::from_selection(&video.quality),
                resize: StoredResize::from_selection(&video.resize),
                fps: video.fps_override,
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
            },
            video: VideoConvertOptions {
                target_format: keys.read(
                    &VIDEO_FORMAT_KEYS,
                    &self.video.format,
                    fallback.video.target_format,
                ),
                quality: self.video.quality.to_selection(&mut keys),
                resize: self.video.resize.to_selection(),
                fps_override: self.video.fps.filter(|fps| *fps > 0),
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
        }
    }

    fn to_selection(&self, keys: &mut KeyReader<'_>) -> QualitySelection {
        QualitySelection {
            preset: keys.read(
                &PRESET_KEYS,
                &self.preset,
                QualitySelection::default().preset,
            ),
            by_target_size: self.by_target_size,
            target_size_text: self.target_size.clone(),
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
        let value = table
            .iter()
            .find(|(_, candidate)| *candidate == key)
            .map(|(value, _)| *value);
        self.unrecognized
            .value_or_default(UNRECOGNIZED_KEY, value, fallback)
    }
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
        assert_eq!(
            IMAGE_FORMAT_KEYS.len(),
            crate::model::IMAGE_TARGET_FORMATS.len()
        );
        assert_eq!(
            VIDEO_FORMAT_KEYS.len(),
            crate::model::VIDEO_TARGET_FORMATS.len()
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
        defaults.video.target_format = VideoTargetFormat::WebmVp9;
        defaults.video.quality.by_target_size = true;
        defaults.video.quality.target_size_text = "8 MiB".to_owned();
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
