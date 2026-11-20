//! `convert = "…"` 能写的格式名。同一个名字对不同的源可能是不同的转法：视频转 gif 是动图，
//! 图片转 gif 是一张图；视频和音频都能转成 m4a（视频是提取声音）。

use file_core::{AudioTargetFormat, ConversionTarget, ImageTargetFormat, VideoTargetFormat};

use crate::model::ConvertibleMediaKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConvertFormat {
    name: &'static str,
    from_image: Option<ConversionTarget>,
    from_video: Option<ConversionTarget>,
    from_audio: Option<ConversionTarget>,
}

const fn image(name: &'static str, format: ImageTargetFormat) -> ConvertFormat {
    ConvertFormat {
        name,
        from_image: Some(ConversionTarget::Image(format)),
        from_video: None,
        from_audio: None,
    }
}

const fn video(name: &'static str, format: VideoTargetFormat) -> ConvertFormat {
    ConvertFormat {
        name,
        from_image: None,
        from_video: Some(ConversionTarget::Video(format)),
        from_audio: None,
    }
}

const fn audio(name: &'static str, format: AudioTargetFormat) -> ConvertFormat {
    ConvertFormat {
        name,
        from_image: None,
        from_video: Some(ConversionTarget::Audio(format)),
        from_audio: Some(ConversionTarget::Audio(format)),
    }
}

/// 文档里的格式表照这张表写。
pub(crate) const CONVERT_FORMATS: &[ConvertFormat] = &[
    image("jpg", ImageTargetFormat::Jpeg),
    image("png", ImageTargetFormat::Png),
    image("webp", ImageTargetFormat::Webp),
    image("avif", ImageTargetFormat::Avif),
    image("jxl", ImageTargetFormat::Jxl),
    image("tiff", ImageTargetFormat::Tiff),
    image("bmp", ImageTargetFormat::Bmp),
    image("ico", ImageTargetFormat::Ico),
    ConvertFormat {
        name: "gif",
        from_image: Some(ConversionTarget::Image(ImageTargetFormat::Gif)),
        from_video: Some(ConversionTarget::Video(VideoTargetFormat::Gif)),
        from_audio: None,
    },
    video("mp4", VideoTargetFormat::Mp4H264),
    video("mp4-h265", VideoTargetFormat::Mp4H265),
    video("mkv", VideoTargetFormat::MkvH265),
    video("mkv-av1", VideoTargetFormat::MkvAv1),
    video("webm", VideoTargetFormat::WebmVp9),
    video("webm-av1", VideoTargetFormat::WebmAv1),
    video("mov", VideoTargetFormat::MovH264),
    video("avi", VideoTargetFormat::AviMpeg4),
    audio("mp3", AudioTargetFormat::Mp3),
    audio("m4a", AudioTargetFormat::M4a),
    audio("opus", AudioTargetFormat::Opus),
    audio("ogg", AudioTargetFormat::Ogg),
    audio("flac", AudioTargetFormat::Flac),
    audio("wav", AudioTargetFormat::Wav),
];

impl ConvertFormat {
    pub(crate) fn named(name: &str) -> Option<Self> {
        CONVERT_FORMATS
            .iter()
            .copied()
            .find(|format| format.name == name)
    }

    pub(crate) fn name(self) -> &'static str {
        self.name
    }

    /// 这类源转成这个格式时的目标；转不了时为 None。
    pub(crate) fn target_for(self, source: ConvertibleMediaKind) -> Option<ConversionTarget> {
        match source {
            ConvertibleMediaKind::Image => self.from_image,
            ConvertibleMediaKind::Video => self.from_video,
            ConvertibleMediaKind::Audio => self.from_audio,
        }
    }

    pub(crate) fn names() -> String {
        CONVERT_FORMATS
            .iter()
            .map(|format| format.name)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_unique_and_converts_something() {
        for (index, format) in CONVERT_FORMATS.iter().enumerate() {
            assert!(
                CONVERT_FORMATS[index + 1..]
                    .iter()
                    .all(|other| other.name != format.name),
                "{} appears twice",
                format.name
            );
            assert!(
                [
                    ConvertibleMediaKind::Image,
                    ConvertibleMediaKind::Video,
                    ConvertibleMediaKind::Audio
                ]
                .into_iter()
                .any(|kind| format.target_for(kind).is_some()),
                "{} converts nothing",
                format.name
            );
        }
    }

    #[test]
    fn audio_formats_take_sound_out_of_videos_too() {
        let m4a = ConvertFormat::named("m4a").unwrap();
        assert_eq!(
            m4a.target_for(ConvertibleMediaKind::Video),
            Some(ConversionTarget::Audio(AudioTargetFormat::M4a))
        );
        assert_eq!(m4a.target_for(ConvertibleMediaKind::Image), None);
        assert_eq!(ConvertFormat::named("jpeg"), None);
    }
}
