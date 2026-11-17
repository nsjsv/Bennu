//! 逐行读 PNG：缩小时边读边缩、切块时边读边写，不解整张（8000×8000 带透明的图整解要 256 MB）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use png::ColorType;

use super::row_reader::RowReader;
use super::{decoding_error, ImageDecodeError};

pub(super) struct PngRows {
    reader: png::Reader<BufReader<File>>,
    color: ColorType,
    width: u32,
    height: u32,
}

impl PngRows {
    /// 隔行扫描的图一遍遍给的是稀疏的行，没法逐行读，返回 `None`。
    pub(super) fn open(path: &Path) -> Result<Option<Self>, ImageDecodeError> {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let reader = decoder.read_info().map_err(error)?;
        if reader.info().interlaced {
            return Ok(None);
        }
        let (width, height) = (reader.info().width, reader.info().height);
        let (color, _) = reader.output_color_type();
        Ok(Some(Self {
            reader,
            color,
            width,
            height,
        }))
    }
}

impl RowReader for PngRows {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn read_rows(&mut self, rgba: &mut [u8]) -> Result<usize, ImageDecodeError> {
        let mut rows = 0;
        for slot in rgba.chunks_exact_mut(self.width as usize * 4) {
            let Some(row) = self.reader.next_row().map_err(error)? else {
                break;
            };
            expand_to_rgba(row.data(), self.color, slot);
            rows += 1;
        }
        Ok(rows)
    }
}

fn error(error: png::DecodingError) -> ImageDecodeError {
    decoding_error(image::ImageFormat::Png, error)
}

/// `normalize_to_color8` 之后只剩这四种 8 位格式。
fn expand_to_rgba(row: &[u8], color: ColorType, rgba: &mut [u8]) {
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    match color {
        ColorType::Grayscale => {
            for (pixel, &gray) in pixels.iter_mut().zip(row) {
                *pixel = [gray, gray, gray, u8::MAX];
            }
        }
        ColorType::GrayscaleAlpha => {
            for (pixel, &[gray, alpha]) in pixels.iter_mut().zip(row.as_chunks::<2>().0) {
                *pixel = [gray, gray, gray, alpha];
            }
        }
        ColorType::Rgb => {
            for (pixel, &[red, green, blue]) in pixels.iter_mut().zip(row.as_chunks::<3>().0) {
                *pixel = [red, green, blue, u8::MAX];
            }
        }
        ColorType::Rgba | ColorType::Indexed => rgba.copy_from_slice(&row[..rgba.len()]),
    }
}
