//! 一段段读出 RGBA8 行（文件里存的方向），不解整张：缩小时边读边缩，超大图切块时边读边写。

use std::path::Path;

use image::{ImageFormat, RgbaImage};

use super::area_shrink::AreaShrink;
use super::jpeg_rows::JpegRows;
use super::png_rows::PngRows;
use super::tiff_rows::TiffRows;
use super::ImageDecodeError;

/// 一次读这么多行：行越多调用越少，内存仍只是几行（12000 宽时 16 行约 0.7 MB）。
pub(super) const BAND_ROWS: usize = 16;

pub(super) trait RowReader: Send {
    /// 文件里存的方向上的宽高。
    fn dimensions(&self) -> (u32, u32);

    /// 接着读，填满 `rgba` 能放下的整行（每行 宽 × 4 字节），返回读到几行；0 表示读完了。
    fn read_rows(&mut self, rgba: &mut [u8]) -> Result<usize, ImageDecodeError>;
}

/// 能逐行读的写法给出读取器：JPEG（系统 libjpeg-turbo）、非隔行 PNG、按段存的 TIFF。
/// 别的格式和写法（CMYK JPEG、隔行 PNG 等）返回 `None`，交给整张解码。
pub(super) fn open(
    path: &Path,
    format: ImageFormat,
) -> Result<Option<Box<dyn RowReader>>, ImageDecodeError> {
    fn boxed(reader: impl RowReader + 'static) -> Box<dyn RowReader> {
        Box::new(reader)
    }
    Ok(match format {
        ImageFormat::Jpeg => JpegRows::open(path)?.map(boxed),
        ImageFormat::Png => PngRows::open(path)?.map(boxed),
        ImageFormat::Tiff => TiffRows::open(path)?.map(boxed),
        _ => None,
    })
}

/// 边读边缩到 `target`（文件里存的方向，每边不大于原图）。
pub(super) fn shrink(
    reader: &mut dyn RowReader,
    target: (u32, u32),
) -> Result<RgbaImage, ImageDecodeError> {
    let (width, height) = reader.dimensions();
    let row_bytes = width as usize * 4;
    let mut shrink = AreaShrink::new((width, height), target);
    let mut band = vec![0; row_bytes * BAND_ROWS];
    loop {
        let rows = reader.read_rows(&mut band)?;
        if rows == 0 {
            return Ok(shrink.finish());
        }
        for row in band[..rows * row_bytes].chunks_exact(row_bytes) {
            shrink.push_row(row);
        }
    }
}
