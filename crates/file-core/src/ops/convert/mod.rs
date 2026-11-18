use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::FileOperationControls;
use crate::FileError;

mod command_plan;
mod edits;
mod ffmpeg;
mod image_engine;
mod media_tools;
mod photo_metadata;

pub use edits::{MediaEdits, TrimRange, VideoRotation, VideoSound};

pub use media_tools::{
    available_conversion_encoders, locate_media_tool, media_duration, ConversionEncoderAvailability,
};

/// 转换目标格式。视频源可以转成音频(抽出声音);其余跨类型不受支持。
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

/// 质量与目标体积互斥:质量档位(Preset)或体积模式(TargetBytes);无损目标只接受 Lossless。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualitySpec {
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
    /// 较长的那条边不超过这么多像素;等比缩小,本来就不超过的不放大。
    LongEdge(u32),
}

/// 照片信息(拍摄时间、相机、GPS、颜色配置)带不带到输出里;仅图片目标使用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PhotoMetadata {
    #[default]
    Keep,
    Strip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannelSpec {
    Keep,
    Mono,
}

/// 输出文件放哪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputLocation {
    BesideSource,
    /// 不存在时转换前建好。
    Directory(PathBuf),
}

impl OutputLocation {
    /// 这个源文件的输出放进的文件夹;引擎占位和任务面板显示共用这一条规则。
    pub fn directory_for<'a>(&'a self, source: &'a Path) -> Option<&'a Path> {
        match self {
            Self::BesideSource => source.parent(),
            Self::Directory(directory) => Some(directory),
        }
    }
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
    pub edits: MediaEdits,
    pub photo_metadata: PhotoMetadata,
    pub output: OutputLocation,
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

    let output = reserve_output_path(&request.source, &request.output, request.target).await?;
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
    edits::validate_edits(&request.source, request.target, request.edits)?;
    match request.target {
        ConversionTarget::Image(format) => {
            validate_image_quality(&request.source, format, request.quality)?;
            validate_image_resize(&request.source, request.resize)
        }
        ConversionTarget::Video(format) => {
            match request.quality {
                QualitySpec::Lossless => {
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
        (ImageTargetFormat::Jpeg, QualitySpec::Preset(_) | QualitySpec::TargetBytes(_)) => Ok(()),
        (ImageTargetFormat::Jpeg, _) => {
            rejected("jpeg output only accepts a quality preset or target size")
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
        ResizeSpec::LongEdge(long_edge) if long_edge > 0 => Ok(()),
        _ => Err(FileError::InvalidInput {
            path: source.to_path_buf(),
            message: "resize must be a percentage in 1..=100 or a positive long edge".to_owned(),
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
    /// 能可靠写进照片信息的格式(img-parts 能改的容器)。AVIF、JPEG XL 的容器也能存,
    /// 但没有可靠的写入库,不写。
    pub fn carries_photo_metadata(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::Webp)
    }

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
            // 放在视频卡片里本身就说明是动图；不带要翻译的词。
            Self::Gif => "GIF",
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

/// 在输出目录内为输出文件挑选最终名:`原名.新扩展名`,重名时 `_1`、`_2` 递增,
/// 并以 create_new 原子占位防止并发转换竞争同名。失败路径由调用方删除占位文件。
async fn reserve_output_path(
    source: &Path,
    location: &OutputLocation,
    target: ConversionTarget,
) -> Result<PathBuf, FileError> {
    let parent = location
        .directory_for(source)
        .ok_or_else(|| FileError::InvalidInput {
            path: source.to_path_buf(),
            message: "source path has no parent directory".to_owned(),
        })?;
    // 原文件旁边的目录本来就在,这一步只是多一次 stat;选的文件夹不在时建好。
    fs::create_dir_all(parent)
        .await
        .map_err(|error| FileError::Convert {
            path: parent.to_path_buf(),
            message: format!("could not create output directory: {error}"),
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

/// 要带照片信息时:先读出来,按大小模式给画面留出它的字节数,引擎写完再补进输出。
async fn dispatch_conversion(
    request: &ConversionRequest,
    output: &Path,
    controls: &FileOperationControls,
) -> Result<u64, FileError> {
    let Some(carried) = carried_photo_metadata(request).await? else {
        return dispatch_engine(request, output, controls).await;
    };
    let mut budgeted = request.clone();
    if let QualitySpec::TargetBytes(target_bytes) = request.quality {
        let picture_bytes = target_bytes.saturating_sub(carried.byte_len()).max(1);
        budgeted.quality = QualitySpec::TargetBytes(picture_bytes);
    }
    dispatch_engine(&budgeted, output, controls).await?;
    let output_path = output.to_path_buf();
    tokio::task::spawn_blocking(move || {
        photo_metadata::embed_carried_metadata(&output_path, carried)
    })
    .await
    .map_err(|error| metadata_worker_failed(output, error))?
}

async fn carried_photo_metadata(
    request: &ConversionRequest,
) -> Result<Option<photo_metadata::CarriedMetadata>, FileError> {
    let ConversionTarget::Image(format) = request.target else {
        return Ok(None);
    };
    if request.photo_metadata == PhotoMetadata::Strip || !format.carries_photo_metadata() {
        return Ok(None);
    }
    let source = request.source.clone();
    let carried =
        tokio::task::spawn_blocking(move || photo_metadata::read_carried_metadata(&source))
            .await
            .map_err(|error| metadata_worker_failed(&request.source, error))?;
    Ok((!carried.is_empty()).then_some(carried))
}

fn metadata_worker_failed(path: &Path, error: tokio::task::JoinError) -> FileError {
    FileError::Convert {
        path: path.to_path_buf(),
        message: format!("photo metadata worker failed: {error}"),
    }
}

async fn dispatch_engine(
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
mod tests;
