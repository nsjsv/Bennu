//! 预览、缩略图、格式转换共用的图片解码。像素图先给内置解码器（认得的格式又快又不起进程），
//! 不认得的交给 ffmpeg；相机 RAW 取里面自带的 JPEG 预览图。出来的都是按方向转正的 RGBA，
//! 可以要求不超过某个大小（预览按屏幕大小、缩略图按档位），大图不用先解成原尺寸再缩：
//! JPEG 在解码时缩小，大 PNG 边读边缩，其余整图解码共用一份内存预算。

mod area_shrink;
mod camera_raw;
mod facts;
mod ffmpeg_still;
mod iso_media;
mod jpeg_rows;
mod png_rows;
mod row_reader;
mod scaled_jpeg;
mod tiff;
mod tiff_rows;
pub mod tiles;

pub use facts::{image_facts, ImageFacts};

use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use thiserror::Error;

use crate::decode_budget;
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
    /// 有没有透明（半透明）的像素：有才在底下垫棋盘格。
    pub has_alpha: bool,
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
        RasterSource::Picture => with_ffmpeg_fallback(decode_builtin(path, bound), || {
            let plan = ffmpeg_still::probe(path)?;
            let pixels = ffmpeg_still::decode(path, &plan, bound)?;
            Ok(DecodedImage {
                has_alpha: has_transparent_pixel(&pixels),
                pixels,
                width: plan.width,
                height: plan.height,
            })
        }),
        RasterSource::CameraRaw => {
            let preview = camera_raw::read_embedded_preview(path)?;
            let (decoder, orientation) = preview.decoder()?;
            decode_whole(decoder, orientation, bound)
        }
    }
}

/// 内置解码器认得的格式，按 `decode_route` 解。
fn decode_builtin(
    path: &Path,
    bound: DecodeBound,
) -> Result<Option<DecodedImage>, ImageDecodeError> {
    let Some(reader) = guessed_reader(path)? else {
        return Ok(None);
    };
    let format = reader.format();
    // JPEG 先由缩小解码的解码器读文件头：image 的 JPEG 解码器一建就把整个文件读进内存。
    if format == Some(ImageFormat::Jpeg) {
        if let Some(header) = scaled_jpeg::read_header(path) {
            let plan = ShrinkPlan::new(header.dimensions(), header.orientation(), bound);
            if decode_route(format, &plan, 0) == DecodeRoute::ShrinkJpeg {
                if let Some(image) = header.decode_scaled(plan.stored_target) {
                    return Ok(Some(plan.finish(image)));
                }
            }
        }
    }
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let plan = ShrinkPlan::new(decoder.dimensions(), orientation, bound);
    let rows = match (decode_route(format, &plan, decoder.total_bytes()), format) {
        (DecodeRoute::Rows, Some(format)) => row_reader::open(path, format)?,
        _ => None,
    };
    match rows {
        Some(mut rows) => {
            let pixels = row_reader::shrink(rows.as_mut(), plan.stored_target)?;
            Ok(Some(plan.finish(DynamicImage::ImageRgba8(pixels))))
        }
        None => decode_whole(decoder, orientation, bound).map(Some),
    }
}

/// 内置解码器认得的图怎么解。都不交给 ffmpeg：它同样要解整图，还多一份格式转换（4000×3000 的
/// 16 位 TIFF 在 ffmpeg 里要 148 MB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeRoute {
    /// 每边至少缩一半的 JPEG 在解码时就缩小。
    ShrinkJpeg,
    /// 逐行读、边读边缩：要缩小、整图又超出解码预算的 PNG；要缩小的 TIFF 不管多大（image 的
    /// TIFF 解码器整解要两份）。
    Rows,
    /// 整图解出来再缩；超出预算的占满整个预算独自解。缩小解码不支持的写法（CMYK JPEG、
    /// 隔行 PNG）也退到这里。
    Whole,
}

fn decode_route(format: Option<ImageFormat>, plan: &ShrinkPlan, whole_bytes: u64) -> DecodeRoute {
    match format {
        Some(ImageFormat::Jpeg) if plan.halves() => DecodeRoute::ShrinkJpeg,
        Some(ImageFormat::Png) if plan.shrinks() && whole_bytes > decode_budget::BUDGET_BYTES => {
            DecodeRoute::Rows
        }
        Some(ImageFormat::Tiff) if plan.shrinks() => DecodeRoute::Rows,
        _ => DecodeRoute::Whole,
    }
}

/// 格式转换用：内置格式原样解（保留 16 位等色深）并按 EXIF 方向摆正，其余交给 ffmpeg
/// （它自己按旋转元数据摆正）。输出不再带原来的方向标记，像素必须是正的。
pub fn open_picture(path: &Path) -> Result<DynamicImage, ImageDecodeError> {
    with_ffmpeg_fallback(
        builtin_decoder(path).and_then(|decoder| decoder.map(decode_upright).transpose()),
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

fn decode_upright(mut decoder: impl ImageDecoder) -> Result<DynamicImage, ImageDecodeError> {
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn builtin_decoder(path: &Path) -> Result<Option<impl ImageDecoder>, ImageDecodeError> {
    guessed_reader(path)?
        .map(|reader| Ok(reader.into_decoder()?))
        .transpose()
}

/// 整图解出来再缩；解之前在共享的解码预算里占好位置。
fn decode_whole(
    mut decoder: impl ImageDecoder,
    orientation: Orientation,
    bound: DecodeBound,
) -> Result<DecodedImage, ImageDecodeError> {
    let _reservation = decode_budget::reserve(decoder.total_bytes());
    let mut limits = image::Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    let plan = ShrinkPlan::new(decoder.dimensions(), orientation, bound);
    Ok(plan.finish(DynamicImage::from_decoder(decoder)?))
}

/// 一张图要缩成多大。`width`×`height` 是转正后的原图大小；`stored_target` 是按文件里存的方向
/// 要缩到的大小（转正放在缩完以后，少转些像素）。
struct ShrinkPlan {
    orientation: Orientation,
    width: u32,
    height: u32,
    stored: (u32, u32),
    stored_target: (u32, u32),
}

impl ShrinkPlan {
    fn new(stored: (u32, u32), orientation: Orientation, bound: DecodeBound) -> Self {
        let (width, height) = oriented_dimensions(stored, orientation);
        let (fit_width, fit_height) = bound.fit(width, height);
        let stored_target = if swaps_axes(orientation) {
            (fit_height, fit_width)
        } else {
            (fit_width, fit_height)
        };
        Self {
            orientation,
            width,
            height,
            stored,
            stored_target,
        }
    }

    fn shrinks(&self) -> bool {
        self.stored_target != self.stored
    }

    /// 每边至少缩一半：JPEG 的 DCT 缩放从 1/2 起。
    fn halves(&self) -> bool {
        self.stored_target.0 * 2 <= self.stored.0 && self.stored_target.1 * 2 <= self.stored.1
    }

    /// 缩到目标大小、转正、换成 RGBA。`image` 不小于目标。
    fn finish(&self, image: DynamicImage) -> DecodedImage {
        let has_alpha_channel = image.color().has_alpha();
        let mut image = if (image.width(), image.height()) == self.stored_target {
            image
        } else {
            image.thumbnail(self.stored_target.0, self.stored_target.1)
        };
        image.apply_orientation(self.orientation);
        let pixels = image.into_rgba8();
        DecodedImage {
            // 没有透明通道的格式不用一个个像素看。
            has_alpha: has_alpha_channel && has_transparent_pixel(&pixels),
            pixels,
            width: self.width,
            height: self.height,
        }
    }
}

fn decoding_error(
    format: ImageFormat,
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> ImageDecodeError {
    ImageDecodeError::Image(image::ImageError::Decoding(
        image::error::DecodingError::new(image::error::ImageFormatHint::Exact(format), error),
    ))
}

fn has_transparent_pixel(pixels: &image::RgbaImage) -> bool {
    pixels.pixels().any(|pixel| pixel.0[3] < u8::MAX)
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

#[cfg(test)]
mod row_reader_tests;

#[cfg(test)]
mod shrink_tests;
