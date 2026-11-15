//! 动图的三种容器：GIF、WebP、APNG。按开头几个字节认格式，不看后缀（改了后缀的照样认）。
//! 帧时长只扫块表（WebP 的 ANMF、APNG 的 fcTL、GIF 的帧头），不解像素；
//! 「是不是动图」只看到能下结论为止：APNG 看 IDAT 之前有没有 acTL，WebP 看 VP8X 的动画位。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames, ImageDecoder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnimationContainer {
    Gif,
    WebP,
    Png,
}

/// 帧数超过这么多就不再一帧帧数时长（进度条按未知时长处理）。
pub(super) const FRAME_DELAY_SCAN_LIMIT: usize = 20_000;

impl AnimationContainer {
    pub(super) fn sniff(path: &Path) -> Option<Self> {
        let mut head = [0; 12];
        File::open(path).ok()?.read_exact(&mut head).ok()?;
        if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else {
            None
        }
    }
}

/// 两帧以上才算动图；单帧的 GIF、普通 PNG/WebP 按静态图片打开（能缩放、看原始像素）。
pub fn is_animated_image(path: &Path) -> bool {
    match AnimationContainer::sniff(path) {
        Some(AnimationContainer::Gif) => gif_has_second_frame(path).unwrap_or(false),
        Some(AnimationContainer::WebP) => webp_animation_flag(path).unwrap_or(false),
        Some(AnimationContainer::Png) => png_animation_frames(path).unwrap_or(0) > 1,
        None => false,
    }
}

/// 解码器给出的画布大小和逐帧（已合成到画布上的整帧）。
pub(super) fn open_frames(path: &Path) -> Result<((u32, u32), Frames<'static>), String> {
    let container = AnimationContainer::sniff(path)
        .ok_or_else(|| "this file is not a GIF, WebP or PNG animation".to_owned())?;
    let reader = BufReader::new(
        File::open(path).map_err(|error| format!("could not open animated image: {error}"))?,
    );
    let decode_error =
        |error: image::ImageError| format!("could not decode animated image: {error}");
    match container {
        AnimationContainer::Gif => {
            let decoder = GifDecoder::new(reader).map_err(decode_error)?;
            Ok((decoder.dimensions(), decoder.into_frames()))
        }
        AnimationContainer::WebP => {
            let decoder = WebPDecoder::new(reader).map_err(decode_error)?;
            Ok((decoder.dimensions(), decoder.into_frames()))
        }
        AnimationContainer::Png => {
            let decoder = PngDecoder::new(reader).map_err(decode_error)?;
            let dimensions = decoder.dimensions();
            let frames = decoder.apng().map_err(decode_error)?.into_frames();
            Ok((dimensions, frames))
        }
    }
}

/// WebP、APNG 每帧的时长（扫块表）；超过上限返回 None。GIF 由调用方用 gif 帧头扫。
pub(super) fn chunk_frame_delays(
    path: &Path,
    container: AnimationContainer,
) -> std::io::Result<Option<Vec<Duration>>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut delays = Vec::new();
    match container {
        AnimationContainer::WebP => {
            reader.seek(SeekFrom::Start(12))?;
            while let Some((kind, length)) = riff_chunk(&mut reader)? {
                if &kind == b"ANMF" && length >= 16 {
                    let mut header = [0; 16];
                    reader.read_exact(&mut header)?;
                    let milliseconds = u32::from_le_bytes([header[12], header[13], header[14], 0]);
                    delays.push(Duration::from_millis(u64::from(milliseconds)));
                    skip(&mut reader, padded(length) - 16)?;
                } else {
                    skip(&mut reader, padded(length))?;
                }
                if delays.len() > FRAME_DELAY_SCAN_LIMIT {
                    return Ok(None);
                }
            }
        }
        AnimationContainer::Png => {
            reader.seek(SeekFrom::Start(8))?;
            while let Some((kind, length)) = png_chunk(&mut reader)? {
                if &kind == b"fcTL" && length >= 26 {
                    let mut control = [0; 26];
                    reader.read_exact(&mut control)?;
                    let numerator = u16::from_be_bytes([control[20], control[21]]);
                    // 分母 0 按规范当 100（时长单位是百分之一秒）。
                    let denominator = match u16::from_be_bytes([control[22], control[23]]) {
                        0 => 100,
                        denominator => denominator,
                    };
                    delays.push(Duration::from_secs_f64(
                        f64::from(numerator) / f64::from(denominator),
                    ));
                    skip(&mut reader, u64::from(length) - 26 + 4)?;
                } else {
                    skip(&mut reader, u64::from(length) + 4)?;
                }
                if &kind == b"IEND" {
                    break;
                }
                if delays.len() > FRAME_DELAY_SCAN_LIMIT {
                    return Ok(None);
                }
            }
        }
        AnimationContainer::Gif => return Ok(None),
    }
    Ok(Some(delays))
}

fn gif_has_second_frame(path: &Path) -> Result<bool, gif::DecodingError> {
    let mut options = gif::DecodeOptions::new();
    options.skip_frame_decoding(true);
    let mut decoder = options.read_info(BufReader::new(File::open(path)?))?;
    Ok(decoder.read_next_frame()?.is_some() && decoder.read_next_frame()?.is_some())
}

/// VP8X 扩展头的第一个标志字节里，0x02 是「有动画」。
fn webp_animation_flag(path: &Path) -> std::io::Result<bool> {
    let mut head = [0; 21];
    File::open(path)?.read_exact(&mut head)?;
    Ok(&head[12..16] == b"VP8X" && head[20] & 0x02 != 0)
}

/// acTL 必须在第一个 IDAT 之前；看到 IDAT 还没有就是普通 PNG。
fn png_animation_frames(path: &Path) -> std::io::Result<u32> {
    let mut reader = BufReader::new(File::open(path)?);
    reader.seek(SeekFrom::Start(8))?;
    while let Some((kind, length)) = png_chunk(&mut reader)? {
        match &kind {
            b"acTL" if length >= 8 => {
                let mut control = [0; 4];
                reader.read_exact(&mut control)?;
                return Ok(u32::from_be_bytes(control));
            }
            b"IDAT" | b"IEND" => return Ok(0),
            _ => skip(&mut reader, u64::from(length) + 4)?,
        }
    }
    Ok(0)
}

/// RIFF 块头：类型 + 小端长度；读到文件尾返回 None。
fn riff_chunk(reader: &mut BufReader<File>) -> std::io::Result<Option<([u8; 4], u64)>> {
    let mut header = [0; 8];
    match reader.read_exact(&mut header) {
        Ok(()) => Ok(Some((
            [header[0], header[1], header[2], header[3]],
            u64::from(u32::from_le_bytes([
                header[4], header[5], header[6], header[7],
            ])),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

/// PNG 块头：大端长度 + 类型；数据之后还有 4 字节校验。
fn png_chunk(reader: &mut BufReader<File>) -> std::io::Result<Option<([u8; 4], u32)>> {
    let mut header = [0; 8];
    match reader.read_exact(&mut header) {
        Ok(()) => Ok(Some((
            [header[4], header[5], header[6], header[7]],
            u32::from_be_bytes([header[0], header[1], header[2], header[3]]),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

/// RIFF 块按偶数字节对齐。
fn padded(length: u64) -> u64 {
    length + (length & 1)
}

fn skip(reader: &mut BufReader<File>, bytes: u64) -> std::io::Result<()> {
    reader.seek_relative(i64::try_from(bytes).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
