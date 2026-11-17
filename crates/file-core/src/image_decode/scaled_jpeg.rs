//! 大 JPEG 在解码时就缩小。JPEG 按 8×8 块存频率系数，只对低频那几个做反变换，直接得到 1/2、
//! 1/4、1/8 大小的图：1.08 亿像素的照片做缩略图不用先解出 324 MB 的整图，也快得多。
//!
//! 文件头也由这里读：image 的 JPEG 解码器一上来就把整个文件读进内存（这张照片 23 MB），
//! 缩小解码用不到它。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, GrayImage, RgbImage};
use jpeg_decoder::{Decoder, PixelFormat};

/// 读过文件头的 JPEG：尺寸、方向都知道了，像素还没解。
pub(super) struct JpegHeader {
    decoder: Decoder<BufReader<File>>,
    dimensions: (u32, u32),
    orientation: Orientation,
}

/// 这个解码器不认得的写法（算术编码等）返回 `None`，交给通用解码器，由它给出结果或错误。
pub(super) fn read_header(path: &Path) -> Option<JpegHeader> {
    let mut decoder = Decoder::new(BufReader::new(File::open(path).ok()?));
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    // EXIF 段在帧头前面，读完文件头就有了；取方向和 image 的 JPEG 解码器用的是同一个函数。
    let orientation = decoder
        .exif_data()
        .and_then(Orientation::from_exif_chunk)
        .unwrap_or(Orientation::NoTransforms);
    Some(JpegHeader {
        dimensions: (u32::from(info.width), u32::from(info.height)),
        orientation,
        decoder,
    })
}

impl JpegHeader {
    /// 文件里存的方向上的大小。
    pub(super) fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }

    pub(super) fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// 解成不小于 `target`（文件里存的方向）的最小一档。CMYK、12 位这类（通用解码器会转颜色）
    /// 和这个解码器解不了的返回 `None`，交给通用解码器。
    pub(super) fn decode_scaled(
        mut self,
        (target_width, target_height): (u32, u32),
    ) -> Option<DynamicImage> {
        let format = self.decoder.info()?.pixel_format;
        if !matches!(format, PixelFormat::L8 | PixelFormat::RGB24) {
            return None;
        }
        let (width, height) = self
            .decoder
            .scale(
                u16::try_from(target_width).unwrap_or(u16::MAX),
                u16::try_from(target_height).unwrap_or(u16::MAX),
            )
            .ok()?;
        let pixels = self.decoder.decode().ok()?;
        let (width, height) = (u32::from(width), u32::from(height));
        match format {
            PixelFormat::L8 => {
                GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8)
            }
            _ => RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8),
        }
    }
}
