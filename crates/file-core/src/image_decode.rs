//! 预览、缩略图、格式转换共用的图片解码。像素图先给内置解码器（认得的格式又快又不起进程），
//! 不认得的交给 ffmpeg；相机 RAW 取里面自带的 JPEG 预览图。出来的都是按方向转正的 RGBA，
//! 可以要求不超过某个大小（预览按屏幕大小、缩略图按档位），大图不用先解成原尺寸再缩。

mod camera_raw;
mod ffmpeg_still;
mod tiff;

use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use thiserror::Error;

use crate::media::RasterSource;

#[derive(Debug, Error)]
pub enum ImageDecodeError {
    #[error("ffmpeg is needed to show this image but is not installed")]
    FfmpegMissing,
    #[error("could not read image: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode image: {0}")]
    Image(#[from] image::ImageError),
    #[error("this camera raw file has no preview picture that can be shown")]
    NoEmbeddedPreview,
    #[error("ffmpeg could not decode this image: {0}")]
    Ffmpeg(String),
}

/// 解出来的像素最多这么大（只缩不放，保持比例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeBound {
    max_width: u32,
    max_height: u32,
}

impl DecodeBound {
    pub const FULL: Self = Self {
        max_width: u32::MAX,
        max_height: u32::MAX,
    };

    pub fn within(max_width: u32, max_height: u32) -> Self {
        Self {
            max_width: max_width.max(1),
            max_height: max_height.max(1),
        }
    }

    /// `width`×`height` 放进这个框后的大小。
    pub fn fit(self, width: u32, height: u32) -> (u32, u32) {
        if width <= self.max_width && height <= self.max_height {
            return (width, height);
        }
        let scale = (f64::from(self.max_width) / f64::from(width))
            .min(f64::from(self.max_height) / f64::from(height));
        (
            ((f64::from(width) * scale).round() as u32).clamp(1, self.max_width),
            ((f64::from(height) * scale).round() as u32).clamp(1, self.max_height),
        )
    }
}

#[derive(Debug, Clone)]
pub struct DecodedImage {
    /// 转正后的像素，不超过要求的大小。
    pub pixels: image::RgbaImage,
    /// 转正后的原图大小（1:1 显示、图片信息用）。
    pub width: u32,
    pub height: u32,
}

/// 转正后的原图大小。内置格式和 RAW 只读文件头；ffmpeg 格式要跑一次 ffprobe。
pub fn image_dimensions(path: &Path, source: RasterSource) -> Result<(u32, u32), ImageDecodeError> {
    match source {
        RasterSource::Picture => with_ffmpeg_fallback(
            builtin_decoder(path).map(|decoder| {
                decoder.map(|mut decoder| {
                    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
                    oriented_dimensions(decoder.dimensions(), orientation)
                })
            }),
            || ffmpeg_still::probe(path).map(|plan| (plan.width, plan.height)),
        ),
        RasterSource::CameraRaw => {
            let preview = camera_raw::read_embedded_preview(path)?;
            let (decoder, orientation) = preview.decoder()?;
            Ok(oriented_dimensions(decoder.dimensions(), orientation))
        }
    }
}

pub fn decode_image(
    path: &Path,
    source: RasterSource,
    bound: DecodeBound,
) -> Result<DecodedImage, ImageDecodeError> {
    match source {
        RasterSource::Picture => with_ffmpeg_fallback(
            builtin_decoder(path).and_then(|decoder| {
                decoder
                    .map(|mut decoder| {
                        let orientation =
                            decoder.orientation().unwrap_or(Orientation::NoTransforms);
                        decode_with(decoder, orientation, bound)
                    })
                    .transpose()
            }),
            || {
                let plan = ffmpeg_still::probe(path)?;
                let pixels = ffmpeg_still::decode(path, &plan, bound)?;
                Ok(DecodedImage {
                    pixels,
                    width: plan.width,
                    height: plan.height,
                })
            },
        ),
        RasterSource::CameraRaw => {
            let preview = camera_raw::read_embedded_preview(path)?;
            let (decoder, orientation) = preview.decoder()?;
            decode_with(decoder, orientation, bound)
        }
    }
}

/// 格式转换用：内置格式原样解（保留 16 位等色深，和以前一样不转方向），其余交给 ffmpeg。
pub fn open_picture(path: &Path) -> Result<DynamicImage, ImageDecodeError> {
    with_ffmpeg_fallback(
        guessed_reader(path)
            .and_then(|reader| reader.map(|reader| Ok(reader.decode()?)).transpose()),
        || {
            let plan = ffmpeg_still::probe(path)?;
            Ok(DynamicImage::ImageRgba8(ffmpeg_still::decode(
                path,
                &plan,
                DecodeBound::FULL,
            )?))
        },
    )
}

/// 内置解码器认不得（`Ok(None)`）或认得格式但不支持这种写法（某些 AVIF 变体）时交给 ffmpeg；
/// 后一种 ffmpeg 也不行就报内置解码器的错——没装 ffmpeg 不该把它说成「缺 ffmpeg」。数据坏了
/// 不转交：ffmpeg 会把截断的图补成半张灰图显示出来。
fn with_ffmpeg_fallback<T>(
    builtin: Result<Option<T>, ImageDecodeError>,
    ffmpeg: impl FnOnce() -> Result<T, ImageDecodeError>,
) -> Result<T, ImageDecodeError> {
    match builtin {
        Ok(Some(value)) => Ok(value),
        Ok(None) => ffmpeg(),
        Err(builtin_error @ ImageDecodeError::Image(image::ImageError::Unsupported(_))) => {
            ffmpeg().map_err(|_| builtin_error)
        }
        Err(builtin_error) => Err(builtin_error),
    }
}

/// 按内容猜格式；猜不出或这个格式没编进来时返回 None。
fn guessed_reader(
    path: &Path,
) -> Result<Option<ImageReader<std::io::BufReader<std::fs::File>>>, ImageDecodeError> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    Ok(reader
        .format()
        .is_some_and(|format| format.reading_enabled())
        .then_some(reader))
}

fn builtin_decoder(path: &Path) -> Result<Option<impl ImageDecoder>, ImageDecodeError> {
    guessed_reader(path)?
        .map(|reader| Ok(reader.into_decoder()?))
        .transpose()
}

fn decode_with(
    mut decoder: impl ImageDecoder,
    orientation: Orientation,
    bound: DecodeBound,
) -> Result<DecodedImage, ImageDecodeError> {
    let mut limits = image::Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    let image = DynamicImage::from_decoder(decoder)?;
    let (width, height) = oriented_dimensions((image.width(), image.height()), orientation);
    let (fit_width, fit_height) = bound.fit(width, height);
    let mut image = if (fit_width, fit_height) == (width, height) {
        image
    } else if swaps_axes(orientation) {
        image.thumbnail(fit_height, fit_width)
    } else {
        image.thumbnail(fit_width, fit_height)
    };
    image.apply_orientation(orientation);
    Ok(DecodedImage {
        pixels: image.into_rgba8(),
        width,
        height,
    })
}

pub fn oriented_dimensions((width, height): (u32, u32), orientation: Orientation) -> (u32, u32) {
    if swaps_axes(orientation) {
        (height, width)
    } else {
        (width, height)
    }
}

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

#[cfg(test)]
mod tests;
