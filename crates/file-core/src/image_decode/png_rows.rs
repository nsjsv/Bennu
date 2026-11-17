//! 超出解码预算的大 PNG 逐行读、边读边缩：只留当前目标行的累加和与缩好的图，不解整张
//! （8000×8000 带透明的图整解要 256 MB）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image::RgbaImage;
use png::ColorType;

use super::area_shrink::AreaShrink;
use super::{decoding_error, ImageDecodeError};

/// 缩到 `target`（文件里存的方向）。隔行扫描的图一遍遍给的是稀疏的行，没法这样边读边缩，
/// 返回 `None`。
pub(super) fn decode(
    path: &Path,
    target: (u32, u32),
) -> Result<Option<RgbaImage>, ImageDecodeError> {
    let error = |error| decoding_error(image::ImageFormat::Png, error);
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(error)?;
    if reader.info().interlaced {
        return Ok(None);
    }
    let (width, height) = (reader.info().width, reader.info().height);
    let (color, _) = reader.output_color_type();
    let mut shrink = AreaShrink::new((width, height), target);
    let mut rgba = vec![0; width as usize * 4];
    while let Some(row) = reader.next_row().map_err(error)? {
        expand_to_rgba(row.data(), color, &mut rgba);
        shrink.push_row(&rgba);
    }
    Ok(Some(shrink.finish()))
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
