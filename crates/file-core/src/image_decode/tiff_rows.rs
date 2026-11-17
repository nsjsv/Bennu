//! 按条带或图块一段段读 TIFF、一行行交出去：缩小时边读边缩、切块时边读边写，只留一排图块。
//! image 的 TIFF 解码器整解时 tiff 库先解出一份，再拷进输出又是一份（4000×3000 的 16 位 RGB
//! 扫描件 2 × 69 MB）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::{PhotometricInterpretation, PlanarConfiguration, Tag};
use tiff::ColorType;

use super::row_reader::RowReader;
use super::{decoding_error, ImageDecodeError};

/// 一段（条带或一排里的一个图块）最多这么大还按段读；整张只有一段的大图分不了段，交给整图解码。
const MAX_CHUNK_BYTES: u64 = 16 << 20;

pub(super) struct TiffRows {
    decoder: Decoder<BufReader<File>>,
    layout: SampleLayout,
    width: u32,
    height: u32,
    chunk_width: u32,
    chunks_across: u32,
    chunks_down: u32,
    /// 解好的一排图块（RGBA），`band_rows` 行，已交出 `band_next` 行。
    band: Vec<u8>,
    band_rows: usize,
    band_next: usize,
    next_chunk_row: u32,
}

impl TiffRows {
    /// 8/16 位的灰度、RGB（带不带透明）、各通道交错存放、黑为 0 或白为 0 的灰度、RGB 色彩之外的
    /// 写法，以及一段太大的，返回 `None`，交给整图解码。
    pub(super) fn open(path: &Path) -> Result<Option<Self>, ImageDecodeError> {
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
        Ok(Some(Self {
            layout,
            width,
            height,
            chunk_width,
            chunks_across: width.div_ceil(chunk_width),
            chunks_down: height.div_ceil(chunk_height),
            band: vec![0; chunk_height as usize * width as usize * 4],
            band_rows: 0,
            band_next: 0,
            next_chunk_row: 0,
            decoder,
        }))
    }

    /// 解下一排图块进 `band`；读完了返回 false。
    fn decode_next_band(&mut self) -> Result<bool, ImageDecodeError> {
        if self.next_chunk_row == self.chunks_down {
            return Ok(false);
        }
        let row_bytes = self.width as usize * 4;
        for chunk_column in 0..self.chunks_across {
            let index = self.next_chunk_row * self.chunks_across + chunk_column;
            let (data_width, data_height) = self.decoder.chunk_data_dimensions(index);
            self.band_rows = data_height as usize;
            let samples = self.decoder.read_chunk(index).map_err(error)?;
            let left = (chunk_column * self.chunk_width) as usize * 4;
            for y in 0..data_height as usize {
                let destination = &mut self.band[y * row_bytes + left..][..data_width as usize * 4];
                let first = y * data_width as usize * self.layout.channels as usize;
                self.layout.expand_row(&samples, first, destination);
            }
        }
        self.next_chunk_row += 1;
        self.band_next = 0;
        Ok(true)
    }
}

impl RowReader for TiffRows {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn read_rows(&mut self, rgba: &mut [u8]) -> Result<usize, ImageDecodeError> {
        let row_bytes = self.width as usize * 4;
        let mut rows = 0;
        for slot in rgba.chunks_exact_mut(row_bytes) {
            if self.band_next == self.band_rows && !self.decode_next_band()? {
                break;
            }
            slot.copy_from_slice(&self.band[self.band_next * row_bytes..][..row_bytes]);
            self.band_next += 1;
            rows += 1;
        }
        Ok(rows)
    }
}

fn error(error: tiff::TiffError) -> ImageDecodeError {
    decoding_error(image::ImageFormat::Tiff, error)
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
