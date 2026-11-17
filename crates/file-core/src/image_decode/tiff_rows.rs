//! 要缩小的 TIFF 按条带或图块一段段读、边读边缩：只留一排图块和缩好的图。image 的 TIFF 解码器
//! 整解时 tiff 库先解出一份，再拷进输出又是一份（4000×3000 的 16 位 RGB 扫描件 2 × 69 MB）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image::RgbaImage;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::{PhotometricInterpretation, PlanarConfiguration, Tag};
use tiff::ColorType;

use super::area_shrink::AreaShrink;
use super::{decoding_error, ImageDecodeError};

/// 一段（条带或一排图块）最多这么大还按段读；整张只有一段的大图分不了段，交给整图解码。
const MAX_CHUNK_BYTES: u64 = 16 << 20;

/// 缩到 `target`（文件里存的方向）。8/16 位的灰度、RGB（带不带透明）、各通道交错存放、
/// 黑为 0 或白为 0 的灰度、RGB 色彩之外的写法返回 `None`，交给整图解码。
pub(super) fn decode(
    path: &Path,
    target: (u32, u32),
) -> Result<Option<RgbaImage>, ImageDecodeError> {
    let error = |error| decoding_error(image::ImageFormat::Tiff, error);
    let mut decoder = Decoder::new(BufReader::new(File::open(path)?)).map_err(error)?;
    let (width, height) = decoder.dimensions().map_err(error)?;
    let Some(layout) = SampleLayout::of(&mut decoder) else {
        return Ok(None);
    };
    let (chunk_width, chunk_height) = decoder.chunk_dimensions();
    let chunk_bytes =
        u64::from(chunk_width) * u64::from(chunk_height) * u64::from(layout.channels) * 2;
    if chunk_bytes > MAX_CHUNK_BYTES {
        return Ok(None);
    }
    let chunks_across = width.div_ceil(chunk_width);
    let chunks_down = height.div_ceil(chunk_height);
    let row_bytes = width as usize * 4;
    let mut band = vec![0u8; chunk_height as usize * row_bytes];
    let mut shrink = AreaShrink::new((width, height), target);
    for chunk_row in 0..chunks_down {
        let mut band_height = 0;
        for chunk_column in 0..chunks_across {
            let index = chunk_row * chunks_across + chunk_column;
            let (data_width, data_height) = decoder.chunk_data_dimensions(index);
            band_height = data_height as usize;
            let samples = decoder.read_chunk(index).map_err(error)?;
            let left = (chunk_column * chunk_width) as usize * 4;
            for y in 0..data_height as usize {
                let destination = &mut band[y * row_bytes + left..][..data_width as usize * 4];
                let first = y * data_width as usize * layout.channels as usize;
                layout.expand_row(&samples, first, destination);
            }
        }
        for y in 0..band_height {
            shrink.push_row(&band[y * row_bytes..(y + 1) * row_bytes]);
        }
    }
    Ok(Some(shrink.finish()))
}

/// 每个像素几个通道、每个通道几位、灰度是不是白为 0。
struct SampleLayout {
    channels: u8,
    alpha: bool,
    white_is_zero: bool,
}

impl SampleLayout {
    fn of(decoder: &mut Decoder<BufReader<File>>) -> Option<Self> {
        let planar = decoder
            .get_tag_unsigned::<u16>(Tag::PlanarConfiguration)
            .unwrap_or(PlanarConfiguration::Chunky.to_u16());
        if planar != PlanarConfiguration::Chunky.to_u16() {
            return None;
        }
        let photometric = decoder
            .get_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
            .ok()
            .and_then(PhotometricInterpretation::from_u16)?;
        let white_is_zero = match photometric {
            PhotometricInterpretation::WhiteIsZero => true,
            PhotometricInterpretation::BlackIsZero | PhotometricInterpretation::RGB => false,
            _ => return None,
        };
        let (channels, alpha) = match decoder.colortype().ok()? {
            ColorType::Gray(8 | 16) => (1, false),
            ColorType::GrayA(8 | 16) => (2, true),
            ColorType::RGB(8 | 16) => (3, false),
            ColorType::RGBA(8 | 16) => (4, true),
            _ => return None,
        };
        Some(Self {
            channels,
            alpha,
            white_is_zero,
        })
    }

    /// 从第 `first` 个样本起取一行，展开成 RGBA8 写进 `rgba`（宽度由它定）。
    fn expand_row(&self, samples: &DecodingResult, first: usize, rgba: &mut [u8]) {
        let pixels = rgba.as_chunks_mut::<4>().0;
        let channels = self.channels as usize;
        let sample = |index: usize| -> u8 {
            match samples {
                DecodingResult::U8(values) => values[first + index],
                DecodingResult::U16(values) => (values[first + index] >> 8) as u8,
                _ => unreachable!("`SampleLayout::of` only accepts 8 and 16 bit samples"),
            }
        };
        for (x, pixel) in pixels.iter_mut().enumerate() {
            let base = x * channels;
            let alpha = if self.alpha {
                sample(base + channels - 1)
            } else {
                u8::MAX
            };
            *pixel = if channels >= 3 {
                [sample(base), sample(base + 1), sample(base + 2), alpha]
            } else {
                let gray = sample(base);
                let gray = if self.white_is_zero {
                    u8::MAX - gray
                } else {
                    gray
                };
                [gray, gray, gray, alpha]
            };
        }
    }
}
