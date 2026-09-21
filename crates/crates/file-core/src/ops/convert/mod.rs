use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::FileOperationControls;
use crate::FileError;

mod command_plan;
mod ffmpeg;
mod image_engine;

pub use ffmpeg::{
    available_conversion_encoders, locate_media_tool, media_duration, ConversionEncoderAvailability,
};

/// 转换目标格式;按源媒体类型分组,跨类型转换(如视频转音频)不受支持。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionTarget {
    Image(ImageTargetFormat),
    Video(VideoTargetFormat),
    Audio(AudioTargetFormat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageTargetFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
    /// JPEG XL;有损,走 ffmpeg libjxl。
    Jxl,
    Tiff,
    Bmp,
    /// GIF;调色板动画,走 ffmpeg,仅接受质量档位。
    Gif,
    Ico,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTargetFormat {
    Mp4H264,
    Mp4H265,
    MkvH265,
    MkvAv1,
    WebmVp9,
    WebmAv1,
    MovH264,
    /// AVI 容器 + MPEG-4 Part 2;面向老设备的兼容输出。
    AviMpeg4,
    /// 动图输出;调色板编码,仅接受质量档位。
    Gif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTargetFormat {
    Mp3,
    /// AAC 编码,M4A 容器;内置编码器。
    M4a,
    Opus,
    Flac,
    Ogg,
    /// PCM 16bit;无损原始采样。
    Wav,
}

/// 质量与目标体积互斥:质量模式(Level/Preset)或体积模式(TargetBytes);无损目标只接受 Lossless。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualitySpec {
    /// 图片质量滑块,0-100。
    Level(u8),
    Preset(ConversionQualityPreset),
    /// 体积模式:单个输出文件的目标字节数。
    TargetBytes(u64),
    Lossless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionQualityPreset {
    Low,
    Medium,
    High,
}

/// 等比缩放;仅图片与视频目标使用,音频忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSpec {
    Keep,
    /// 百分比 1-100。
    Percent(u8),
    /// 目标宽度像素;高度按比例自动计算。
    Width(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannelSpec {
    Keep,
    Mono,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionRequest {
    pub source: PathBuf,
    pub target: ConversionTarget,
    pub quality: QualitySpec,
    pub resize: ResizeSpec,
    /// 覆盖输出帧率(fps);仅视频目标使用。
    pub fps_override: Option<u32>,
    pub audio_channels: AudioChannelSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedFile {
    pub source: PathBuf,
    pub output: PathBuf,
    pub byte_count: u64,
    /// 体积模式下输出是否落在目标大小容差内;其他模式恒为 true。
    pub reached_target: bool,
}

/// 输出体积达标容差:码率估算与容器开销存在误差,目标 +10% 内视为达成。
pub(crate) const TARGET_BYTES_TOLERANCE_NUMERATOR: u64 = 11;
pub(crate) const TARGET_BYTES_TOLERANCE_DENOMINATOR: u64 = 10;

/// 体积模式二分/估算的收敛判定:输出不超过目标视为满足。
pub(crate) fn output_satisfies_target(byte_count: u64, target_bytes: u64) -> bool {
    byte_count <= target_bytes
}

pub(crate) fn output_reaches_target(byte_count: u64, target_bytes: u64) -> bool {
    byte_count * TARGET_BYTES_TOLERANCE_DENOMINATOR
        <= target_bytes * TARGET_BYTES_TOLERANCE_NUMERATOR
}

/// 单文件转换入口:校验请求、原子占位输出路径、分派引擎、失败时清理半成品。
pub async fn convert_file_with_controls(
    request: ConversionRequest,
    controls: &FileOperationControls,
) -> Result<ConvertedFile, FileError> {
    controls.checkpoint_now()?;
    validate_conversion_request(&request)?;

    let output = reserve_output_path(&request.source, request.target).await?;
    let convert_output = output.clone();
    let result = dispatch_conversion(&request, &output, controls).await;
    match result {
        Ok(byte_count) => Ok(ConvertedFile {
            source: request.source.clone(),
            output,
            byte_count,
            reached_target: match request.quality {
                QualitySpec::TargetBytes(target_bytes) => {
                    output_reaches_target(byte_count, target_bytes)
                }
                _ => true,
            },
        }),
        Err(error) => {
            remove_leftover_output(&convert_output).await;
            Err(error)
        }
    }
}

fn validate_conversion_request(request: &ConversionRequest) -> Result<(), FileError> {
    if let QualitySpec::TargetBytes(0) = request.quality {
        return Err(FileError::InvalidInput {
            path: request.source.clone(),
            message: "target size must be greater than zero".to_owned(),
        });
    }
    // SVG 是矢量源,image crate 无法解码,本期不支持作为转换源。
    if source_is_svg(&request.source) {
        return Err(FileError::InvalidInput {
            path: request.source.clone(),
            message: "svg sources are not supported for conversion".to_owned(),
        });
    }
    match request.target {
        ConversionTarget::Image(format) => {
            validate_image_quality(&request.source, format, request.quality)?;
            validate_image_resize(&request.source, request.resize)
        }
        ConversionTarget::Video(format) => {
            match request.quality {
                QualitySpec::Lossless | QualitySpec::Level(_) => {
                    return Err(invalid_quality_for_video(&request.source, format));
                }
                // GIF 调色板编码没有码率估算语义。
                QualitySpec::TargetBytes(_) if format == VideoTargetFormat::Gif => {
                    return Err(FileError::InvalidInput {
                        path: request.source.clone(),
                        message: "gif output only accepts a quality preset".to_owned(),
                    });
                }
                _ => {}
            }
            if let Some(fps) = request.fps_override {
                if fps == 0 {
                    return Err(FileError::InvalidInput {
                        path: request.source.clone(),
                        message: "fps override must be greater than zero".to_owned(),
                    });
                }
            }
            Ok(())
        }
        ConversionTarget::Audio(format) => match (format, request.quality) {
            (AudioTargetFormat::Flac | AudioTargetFormat::Wav, QualitySpec::Lossless) => Ok(()),
            (AudioTargetFormat::Flac | AudioTargetFormat::Wav, _) => Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "lossless audio output only accepts lossless quality".to_owned(),
            }),
            (_, QualitySpec::Lossless) => Err(FileError::InvalidInput {
                path: request.source.clone(),
                message: "lossy audio output does not accept lossless quality".to_owned(),
            }),
            _ => Ok(()),
        },
    }
}

fn source_is_svg(source: &Path) -> bool {
    source
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

fn validate_image_quality(
    source: &Path,
    format: ImageTargetFormat,
    quality: QualitySpec,
) -> Result<(), FileError> {
    let rejected = |message: &'static str| {
        Err(FileError::InvalidInput {
            path: source.to_path_buf(),
            message: message.to_owned(),
        })
    };
    match (format, quality) {
        (ImageTargetFormat::Jpeg, QualitySpec::Level(_) | QualitySpec::TargetBytes(_)) => Ok(()),
        (ImageTargetFormat::Jpeg, _) => {
            rejected("jpeg output only accepts a quality level or target size")
        }
        // 无损目标体积不可调,只接受 Lossless。
        (
            ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico,
            QualitySpec::Lossless,
        ) => Ok(()),
        (
            ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico,
            _,
        ) => rejected("this output format is lossless and only accepts lossless quality"),
        (ImageTargetFormat::Webp | ImageTargetFormat::Avif | ImageTargetFormat::Jxl, _) => {
            match quality {
                QualitySpec::Lossless => {
                    rejected("lossy image output does not accept lossless quality")
                }
                _ => Ok(()),
            }
        }
        // GIF 调色板编码没有连续质量域,只有档位。
        (ImageTargetFormat::Gif, QualitySpec::Preset(_)) => Ok(()),
        (ImageTargetFormat::Gif, _) => rejected("gif output only accepts a quality preset"),
    }
}

fn validate_image_resize(source: &Path, resize: ResizeSpec) -> Result<(), FileError> {
    match resize {
        ResizeSpec::Keep => Ok(()),
        ResizeSpec::Percent(percent) if (1..=100).contains(&percent) => Ok(()),
        ResizeSpec::Width(width) if width > 0 => Ok(()),
        _ => Err(FileError::InvalidInput {
            path: source.to_path_buf(),
            message: "resize must be a percentage in 1..=100 or a positive width".to_owned(),
        }),
    }
}

fn invalid_quality_for_video(source: &Path, format: VideoTargetFormat) -> FileError {
    FileError::InvalidInput {
        path: source.to_path_buf(),
        message: format!("{format:?} output does not accept lossless quality"),
    }
}

impl ImageTargetFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jxl => "jxl",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Ico => "ico",
        }
    }
}

impl VideoTargetFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4H264 | Self::Mp4H265 => "mp4",
            Self::MkvH265 | Self::MkvAv1 => "mkv",
            Self::WebmVp9 | Self::WebmAv1 => "webm",
            Self::MovH264 => "mov",
            Self::AviMpeg4 => "avi",
            Self::Gif => "gif",
        }
    }
}

impl AudioTargetFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Wav => "wav",
        }
    }
}

impl fmt::Display for ImageTargetFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Webp => "WebP",
            Self::Avif => "AVIF",
            Self::Jxl => "JPEG XL",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
            Self::Gif => "GIF",
            Self::Ico => "ICO",
        };
        formatter.write_str(label)
    }
}

impl fmt::Display for VideoTargetFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Mp4H264 => "MP4 (H.264)",
            Self::Mp4H265 => "MP4 (H.265)",
            Self::MkvH265 => "MKV (H.265)",
            Self::MkvAv1 => "MKV (AV1)",
            Self::WebmVp9 => "WebM (VP9)",
            Self::WebmAv1 => "WebM (AV1)",
            Self::MovH264 => "MOV (H.264)",
            Self::AviMpeg4 => "AVI (MPEG-4)",
            Self::Gif => "GIF (animation)",
        };
        formatter.write_str(label)
    }
}

impl fmt::Display for AudioTargetFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Mp3 => "MP3",
            Self::M4a => "M4A (AAC)",
            Self::Opus => "Opus",
            Self::Flac => "FLAC",
            Self::Ogg => "Ogg Vorbis",
            Self::Wav => "WAV",
        };
        formatter.write_str(label)
    }
}

impl ConversionTarget {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Image(format) => format.extension(),
            Self::Video(format) => format.extension(),
            Self::Audio(format) => format.extension(),
        }
    }
}

/// 在源目录内为输出文件挑选最终名:`原名.新扩展名`,重名时 `_1`、`_2` 递增,
/// 并以 create_new 原子占位防止并发转换竞争同名。失败路径由调用方删除占位文件。
async fn reserve_output_path(
    source: &Path,
    target: ConversionTarget,
) -> Result<PathBuf, FileError> {
    let parent = source.parent().ok_or_else(|| FileError::InvalidInput {
        path: source.to_path_buf(),
        message: "source path has no parent directory".to_owned(),
    })?;
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| FileError::InvalidInput {
            path: source.to_path_buf(),
            message: "source file name is not valid unicode".to_owned(),
        })?;

    let extension = target.extension();
    let mut candidate = parent.join(format!("{stem}.{extension}"));
    for index in 1..1000u32 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                candidate = parent.join(format!("{stem}_{index}.{extension}"));
            }
            Err(error) => {
                return Err(FileError::Convert {
                    path: candidate.clone(),
                    message: format!("could not reserve output file: {error}"),
                });
            }
        }
    }

    Err(FileError::Convert {
        path: parent.to_path_buf(),
        message: "could not find a free output name".to_owned(),
    })
}

async fn dispatch_conversion(
    request: &ConversionRequest,
    output: &Path,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let byte_count = match request.target {
        ConversionTarget::Image(
            ImageTargetFormat::Jpeg
            | ImageTargetFormat::Png
            | ImageTargetFormat::Tiff
            | ImageTargetFormat::Bmp
            | ImageTargetFormat::Ico,
        ) => image_engine::convert_with_image_crate(request, output, controls).await?,
        ConversionTarget::Image(_) => {
            ffmpeg::convert_losing_source(request, output, controls).await?
        }
        ConversionTarget::Video(_) | ConversionTarget::Audio(_) => {
            ffmpeg::convert_losing_source(request, output, controls).await?
        }
    };
    Ok(byte_count)
}

/// 删除失败或取消后遗留的占位/半成品输出;删除失败不掩盖原始错误。
async fn remove_leftover_output(output: &Path) {
    let _ = fs::remove_file(output).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_path() -> PathBuf {
        PathBuf::from("/tmp/convert-tests/photo.png")
    }

    fn base_request(target: ConversionTarget, quality: QualitySpec) -> ConversionRequest {
        ConversionRequest {
            source: source_path(),
            target,
            quality,
            resize: ResizeSpec::Keep,
            fps_override: None,
            audio_channels: AudioChannelSpec::Keep,
        }
    }

    #[test]
    fn jpeg_accepts_level_and_target_size_only() {
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::Level(80),
        ))
        .is_ok());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::TargetBytes(500 * 1024),
        ))
        .is_ok());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::Lossless,
        ))
        .is_err());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::Preset(ConversionQualityPreset::Medium),
        ))
        .is_err());
    }

    #[test]
    fn png_rejects_quality_levels() {
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Png),
            QualitySpec::Lossless,
        ))
        .is_ok());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Png),
            QualitySpec::Level(80),
        ))
        .is_err());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Png),
            QualitySpec::TargetBytes(1024),
        ))
        .is_err());
    }

    #[test]
    fn svg_sources_are_rejected() {
        let mut request = base_request(
            ConversionTarget::Image(ImageTargetFormat::Png),
            QualitySpec::Lossless,
        );
        request.source = PathBuf::from("/tmp/convert-tests/logo.svg");
        assert!(validate_conversion_request(&request).is_err());
    }

    #[test]
    fn lossless_audio_only_accepts_lossless() {
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Audio(AudioTargetFormat::Flac),
            QualitySpec::Lossless,
        ))
        .is_ok());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Audio(AudioTargetFormat::Flac),
            QualitySpec::TargetBytes(1024),
        ))
        .is_err());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Audio(AudioTargetFormat::Flac),
            QualitySpec::Preset(ConversionQualityPreset::High),
        ))
        .is_err());
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Audio(AudioTargetFormat::Mp3),
            QualitySpec::Lossless,
        ))
        .is_err());
    }

    #[test]
    fn zero_target_size_is_rejected() {
        assert!(validate_conversion_request(&base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::TargetBytes(0),
        ))
        .is_err());
    }

    #[test]
    fn resize_bounds_are_validated() {
        let mut request = base_request(
            ConversionTarget::Image(ImageTargetFormat::Jpeg),
            QualitySpec::Level(80),
        );
        request.resize = ResizeSpec::Percent(0);
        assert!(validate_conversion_request(&request).is_err());
        request.resize = ResizeSpec::Percent(100);
        assert!(validate_conversion_request(&request).is_ok());
        request.resize = ResizeSpec::Width(0);
        assert!(validate_conversion_request(&request).is_err());
        request.resize = ResizeSpec::Width(1920);
        assert!(validate_conversion_request(&request).is_ok());
    }

    #[test]
    fn video_rejects_zero_fps_and_lossless() {
        let mut request = base_request(
            ConversionTarget::Video(VideoTargetFormat::Mp4H264),
            QualitySpec::Preset(ConversionQualityPreset::Medium),
        );
        request.fps_override = Some(0);
        assert!(validate_conversion_request(&request).is_err());
        request.fps_override = Some(30);
        assert!(validate_conversion_request(&request).is_ok());
        request.quality = QualitySpec::Lossless;
        assert!(validate_conversion_request(&request).is_err());
    }

    #[test]
    fn gif_video_only_accepts_presets() {
        let mut request = base_request(
            ConversionTarget::Video(VideoTargetFormat::Gif),
            QualitySpec::Preset(ConversionQualityPreset::Medium),
        );
        assert!(validate_conversion_request(&request).is_ok());
        request.quality = QualitySpec::TargetBytes(1024);
        assert!(validate_conversion_request(&request).is_err());
        request.quality = QualitySpec::Level(80);
        assert!(validate_conversion_request(&request).is_err());
    }

    #[test]
    fn new_lossless_targets_only_accept_lossless() {
        for format in [
            ImageTargetFormat::Tiff,
            ImageTargetFormat::Bmp,
            ImageTargetFormat::Ico,
        ] {
            let mut request = base_request(ConversionTarget::Image(format), QualitySpec::Lossless);
            assert!(validate_conversion_request(&request).is_ok(), "{format:?}");
            request.quality = QualitySpec::Level(80);
            assert!(validate_conversion_request(&request).is_err(), "{format:?}");
            request.quality = QualitySpec::TargetBytes(4096);
            assert!(validate_conversion_request(&request).is_err(), "{format:?}");
        }
    }

    #[test]
    fn wav_audio_only_accepts_lossless() {
        let mut request = base_request(
            ConversionTarget::Audio(AudioTargetFormat::Wav),
            QualitySpec::Lossless,
        );
        assert!(validate_conversion_request(&request).is_ok());
        request.quality = QualitySpec::Preset(ConversionQualityPreset::High);
        assert!(validate_conversion_request(&request).is_err());
        request.quality = QualitySpec::TargetBytes(1024);
        assert!(validate_conversion_request(&request).is_err());
    }

    #[test]
    fn output_reaches_target_within_tolerance() {
        assert!(output_reaches_target(500 * 1024, 500 * 1024));
        assert!(output_reaches_target(540 * 1024, 500 * 1024));
        assert!(!output_reaches_target(560 * 1024, 500 * 1024));
        assert!(output_satisfies_target(499_999, 500_000));
    }

    #[tokio::test]
    async fn reserve_output_path_avoids_existing_names_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("photo.png");
        std::fs::write(&source, b"stub").expect("write source");

        let first = reserve_output_path(&source, ConversionTarget::Image(ImageTargetFormat::Jpeg))
            .await
            .expect("reserve first");
        assert_eq!(first, directory.path().join("photo.jpg"));

        let second = reserve_output_path(&source, ConversionTarget::Image(ImageTargetFormat::Jpeg))
            .await
            .expect("reserve second");
        assert_eq!(second, directory.path().join("photo_1.jpg"));

        std::fs::remove_file(&first).expect("drop first placeholder");
        let third = reserve_output_path(&source, ConversionTarget::Image(ImageTargetFormat::Webp))
            .await
            .expect("reserve third");
        assert_eq!(third, directory.path().join("photo.webp"));
    }

    #[tokio::test]
    async fn convert_failure_cleans_up_placeholder() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = directory.path().join("clip.mp4");
        std::fs::write(&source, b"not a real video").expect("write source");

        let request = ConversionRequest {
            source: source.clone(),
            target: ConversionTarget::Audio(AudioTargetFormat::Mp3),
            quality: QualitySpec::Preset(ConversionQualityPreset::Medium),
            resize: ResizeSpec::Keep,
            fps_override: None,
            audio_channels: AudioChannelSpec::Keep,
        };
        let controls = FileOperationControls::running(tokio_util::sync::CancellationToken::new());
        let result = convert_file_with_controls(request, &controls).await;
        // ffmpeg 缺失或输入非法都应报错,且目录里不残留占位文件。
        assert!(result.is_err(), "invalid input must fail");
        let leftovers = std::fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path() != source)
            .count();
        assert_eq!(leftovers, 0, "no leftover output files");
    }
}
