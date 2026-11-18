use std::path::Path;
use std::time::Duration;

use super::ConversionTarget;
use crate::FileError;

/// 音视频源的剪辑；默认值 = 原样。图片目标只收默认值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaEdits {
    /// 视频与音频目标都适用（从视频里抽一段声音）。
    pub trim: TrimRange,
    /// 仅视频目标。
    pub rotation: VideoRotation,
    /// 仅视频目标；GIF 本来没声音。
    pub sound: VideoSound,
}

/// 只转这一段；两端都 None = 整个文件。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrimRange {
    pub start: Option<Duration>,
    pub end: Option<Duration>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VideoRotation {
    #[default]
    None,
    Clockwise,
    HalfTurn,
    CounterClockwise,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VideoSound {
    #[default]
    Keep,
    Remove,
}

impl TrimRange {
    pub fn is_whole(self) -> bool {
        self == Self::default()
    }

    /// 截取后的时长；按总长截断，截出来什么都没有时为 None。
    pub(super) fn clipped_duration(self, total: Duration) -> Option<Duration> {
        let end = self.end.map_or(total, |end| end.min(total));
        end.checked_sub(self.start.unwrap_or_default())
            .filter(|duration| !duration.is_zero())
    }
}

pub(super) fn validate_edits(
    source: &Path,
    target: ConversionTarget,
    edits: MediaEdits,
) -> Result<(), FileError> {
    let rejected = |message: &str| {
        Err(FileError::InvalidInput {
            path: source.to_path_buf(),
            message: message.to_owned(),
        })
    };
    if let (Some(start), Some(end)) = (edits.trim.start, edits.trim.end) {
        if end <= start {
            return rejected("trim end must be after its start");
        }
    }
    match target {
        ConversionTarget::Image(_) if edits != MediaEdits::default() => {
            rejected("image output does not accept trim, rotation or sound edits")
        }
        ConversionTarget::Audio(_)
            if edits.rotation != VideoRotation::None || edits.sound != VideoSound::Keep =>
        {
            rejected("audio output only accepts a trim")
        }
        _ => Ok(()),
    }
}

/// 秒数给 ffmpeg：毫秒精度足够定位到帧。
pub(super) fn ffmpeg_seconds(duration: Duration) -> String {
    format!("{:.3}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::convert::{AudioTargetFormat, ImageTargetFormat, VideoTargetFormat};

    const SOURCE: &str = "/tmp/convert-tests/clip.mkv";

    fn seconds(value: u64) -> Option<Duration> {
        Some(Duration::from_secs(value))
    }

    #[test]
    fn edits_are_validated_per_target() {
        let source = Path::new(SOURCE);
        let video = ConversionTarget::Video(VideoTargetFormat::Mp4H264);
        let audio = ConversionTarget::Audio(AudioTargetFormat::Mp3);
        let image = ConversionTarget::Image(ImageTargetFormat::Jpeg);
        let trim = MediaEdits {
            trim: TrimRange {
                start: seconds(10),
                end: seconds(20),
            },
            ..MediaEdits::default()
        };
        let rotate = MediaEdits {
            rotation: VideoRotation::Clockwise,
            sound: VideoSound::Remove,
            ..MediaEdits::default()
        };

        assert!(validate_edits(source, video, trim).is_ok());
        assert!(validate_edits(source, video, rotate).is_ok());
        assert!(validate_edits(source, audio, trim).is_ok());
        assert!(validate_edits(source, audio, rotate).is_err());
        assert!(validate_edits(source, image, trim).is_err());
        assert!(validate_edits(source, image, MediaEdits::default()).is_ok());

        let backwards = MediaEdits {
            trim: TrimRange {
                start: seconds(20),
                end: seconds(20),
            },
            ..MediaEdits::default()
        };
        assert!(validate_edits(source, video, backwards).is_err());
    }

    #[test]
    fn clipped_duration_stays_inside_the_media() {
        let total = Duration::from_secs(60);
        assert_eq!(TrimRange::default().clipped_duration(total), Some(total));
        let middle = TrimRange {
            start: seconds(10),
            end: seconds(20),
        };
        assert_eq!(middle.clipped_duration(total), seconds(10));
        let past_end = TrimRange {
            start: seconds(50),
            end: seconds(90),
        };
        assert_eq!(past_end.clipped_duration(total), seconds(10));
        let outside = TrimRange {
            start: seconds(70),
            end: None,
        };
        assert_eq!(outside.clipped_duration(total), None);
    }

    #[test]
    fn ffmpeg_seconds_keep_milliseconds() {
        assert_eq!(ffmpeg_seconds(Duration::from_millis(90_500)), "90.500");
    }
}
