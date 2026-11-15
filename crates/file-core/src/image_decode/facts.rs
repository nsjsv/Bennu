//! 图片信息层要的事实：格式、色深、颜色配置、拍摄参数（EXIF）。只读文件头和元数据块，
//! 不解像素；读不到的项留空（信息层不显示那一行），这里不报错。

use std::fs::File;
use std::io::{self, Cursor};
use std::path::Path;

use image::codecs::jpeg::JpegDecoder;
use image::{ImageDecoder, ImageFormat};

use super::iso_media::{iso_boxes, IsoBox};
use super::tiff::{ByteSource, Ifd, TiffReader, TAG_EXIF_IFD, TAG_GPS_IFD};
use super::{camera_raw, guessed_reader};
use crate::media::RasterSource;

const TAG_MAKE: u16 = 0x010F;
const TAG_MODEL: u16 = 0x0110;
const TAG_DATE_TIME: u16 = 0x0132;
const TAG_EXPOSURE_TIME: u16 = 0x829A;
const TAG_F_NUMBER: u16 = 0x829D;
const TAG_ISO: u16 = 0x8827;
const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
const TAG_FOCAL_LENGTH: u16 = 0x920A;
const TAG_FOCAL_LENGTH_35MM: u16 = 0xA405;
const TAG_LENS_MODEL: u16 = 0xA434;
const TAG_GPS_LATITUDE_REF: u16 = 0x0001;
const TAG_GPS_LATITUDE: u16 = 0x0002;
const TAG_GPS_LONGITUDE_REF: u16 = 0x0003;
const TAG_GPS_LONGITUDE: u16 = 0x0004;
/// HEIF 的元数据目录（iinf、iloc）只有几 KB，超过这么大说明是坏文件。
const MAX_METADATA_BOX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageFacts {
    /// 格式名（JPEG、PNG……；内置解码器不认的用后缀，相机 RAW 是「RAW (CR2)」）。
    pub format: Option<String>,
    pub bits_per_channel: Option<u8>,
    /// 内嵌颜色配置文件的名字（Display P3、Adobe RGB……）。
    pub color_profile: Option<String>,
    /// 相机型号（厂商名重复的去掉）。
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// 拍摄时间，`2024-05-01 12:34:56`。
    pub taken_at: Option<String>,
    pub focal_length_mm: Option<f32>,
    pub focal_length_35mm: Option<u32>,
    pub f_number: Option<f32>,
    /// 曝光时间，秒的分数（分子，分母）。
    pub exposure: Option<(u32, u32)>,
    pub iso: Option<u32>,
    /// 纬度、经度（度；南纬、西经为负）。
    pub location: Option<(f64, f64)>,
}

pub fn image_facts(path: &Path, source: RasterSource) -> ImageFacts {
    let mut facts = ImageFacts::default();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_uppercase());
    match source {
        RasterSource::CameraRaw => {
            facts.format = Some(match &extension {
                Some(extension) => format!("RAW ({extension})"),
                None => "RAW".to_owned(),
            });
            let _ = read_camera_raw_metadata(path, &mut facts);
        }
        RasterSource::Picture => match guessed_reader(path) {
            Ok(Some(reader)) => {
                let format = reader.format();
                facts.format = format.map(format_name);
                let _ = read_builtin_metadata(path, reader, format, &mut facts);
            }
            _ => {
                facts.format = extension;
                let _ = read_container_exif(path, &mut facts);
            }
        },
    }
    facts
}

fn format_name(format: ImageFormat) -> String {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Avif => "AVIF",
        other => return format!("{other:?}").to_uppercase(),
    }
    .to_owned()
}

fn read_builtin_metadata(
    path: &Path,
    reader: image::ImageReader<io::BufReader<File>>,
    format: Option<ImageFormat>,
    facts: &mut ImageFacts,
) -> io::Result<()> {
    let mut decoder = reader.into_decoder().map_err(io::Error::other)?;
    let color = decoder.original_color_type();
    let channels = u16::from(color.channel_count()).max(1);
    facts.bits_per_channel = u8::try_from(color.bits_per_pixel() / channels).ok();
    facts.color_profile = decoder
        .icc_profile()
        .ok()
        .flatten()
        .and_then(|profile| icc_description(&profile));
    match format {
        // TIFF 的 EXIF 就在文件自己的目录里；AVIF 在容器的元数据条目里。
        Some(ImageFormat::Tiff) => {
            let mut reader = TiffReader::open(File::open(path)?, 0)?;
            read_tiff_exif(&mut reader, facts);
        }
        Some(ImageFormat::Avif) => read_container_exif(path, facts)?,
        _ => {
            if let Some(exif) = decoder.exif_metadata().ok().flatten() {
                read_exif_block(&exif, facts);
            }
        }
    }
    Ok(())
}

/// JPEG/PNG/WebP 给的 EXIF 块：有的带 `Exif\0\0` 前缀，有的直接是 TIFF。
fn read_exif_block(exif: &[u8], facts: &mut ImageFacts) {
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    if let Ok(mut reader) = TiffReader::open(tiff, 0) {
        read_tiff_exif(&mut reader, facts);
    }
}

fn read_camera_raw_metadata(path: &Path, facts: &mut ImageFacts) -> io::Result<()> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    match camera_raw::raw_layout(&mut file, file_len)? {
        camera_raw::RawLayout::Tiff => {
            let mut reader = TiffReader::open(file, 0)?;
            read_tiff_exif(&mut reader, facts);
        }
        camera_raw::RawLayout::Cr3(metadata) => {
            if let Some((mut reader, ifd)) = cr3_section(&mut file, metadata.primary) {
                read_primary(&mut reader, &ifd, facts);
            }
            if let Some((mut reader, ifd)) = cr3_section(&mut file, metadata.exif) {
                read_exposure(&mut reader, &ifd, facts);
            }
            if let Some((mut reader, ifd)) = cr3_section(&mut file, metadata.gps) {
                read_location(&mut reader, &ifd, facts);
            }
        }
        // 富士的拍摄参数只在预览图自带的 EXIF 里。
        camera_raw::RawLayout::Raf => {
            let preview = camera_raw::read_embedded_preview(path).map_err(io::Error::other)?;
            let mut decoder =
                JpegDecoder::new(Cursor::new(preview.jpeg())).map_err(io::Error::other)?;
            if let Some(exif) = decoder.exif_metadata().ok().flatten() {
                read_exif_block(&exif, facts);
            }
        }
    }
    Ok(())
}

/// CR3 把 IFD0、EXIF、GPS 三个目录分别存成三段 TIFF，每段的第一个目录就是要的那个。
fn cr3_section(file: &mut File, start: Option<u64>) -> Option<(TiffReader<&mut File>, Ifd)> {
    let mut reader = TiffReader::open(file, start?).ok()?;
    let ifd = reader.first_directory().ok()?;
    Some((reader, ifd))
}

/// HEIF/AVIF 的 EXIF 是元数据目录里一个 `Exif` 条目；JPEG XL 容器里是一个 `Exif` 盒子。
/// 两者开头都有 4 字节说明 TIFF 头还要往后跳多少。
fn read_container_exif(path: &Path, facts: &mut ImageFacts) -> io::Result<()> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut exif_start = None;
    for top in iso_boxes(&mut file, 0, file_len)? {
        match &top.kind {
            b"Exif" => exif_start = Some(top.payload),
            b"meta" => exif_start = heif_exif_item(&mut file, &top)?,
            _ => continue,
        }
        break;
    }
    let Some(start) = exif_start else {
        return Ok(());
    };
    let mut skip = [0; 4];
    file.read_exact_at(start, &mut skip)?;
    let tiff_start = start + 4 + u64::from(u32::from_be_bytes(skip));
    let mut reader = TiffReader::open(file, tiff_start)?;
    read_tiff_exif(&mut reader, facts);
    Ok(())
}

fn heif_exif_item(file: &mut File, meta: &IsoBox) -> io::Result<Option<u64>> {
    // meta 是带版本的盒子：子盒子前有 4 字节版本和标志。
    let children = iso_boxes(file, meta.payload + 4, meta.end)?;
    let (Some(info), Some(locations)) = (
        children.iter().find(|child| &child.kind == b"iinf"),
        children.iter().find(|child| &child.kind == b"iloc"),
    ) else {
        return Ok(None);
    };
    let Some(exif_id) = exif_item_id(file, info)? else {
        return Ok(None);
    };
    let locations = read_box(file, locations)?;
    Ok(item_offset(&locations, exif_id))
}

fn read_box(file: &mut File, iso_box: &IsoBox) -> io::Result<Vec<u8>> {
    let length = iso_box.end - iso_box.payload;
    if length > MAX_METADATA_BOX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "metadata box too large",
        ));
    }
    let mut bytes = vec![0; length as usize];
    file.read_exact_at(iso_box.payload, &mut bytes)?;
    Ok(bytes)
}

fn exif_item_id(file: &mut File, info: &IsoBox) -> io::Result<Option<u32>> {
    let mut version = [0; 1];
    file.read_exact_at(info.payload, &mut version)?;
    let count_len = if version[0] == 0 { 2 } else { 4 };
    for entry in iso_boxes(file, info.payload + 4 + count_len, info.end)? {
        if &entry.kind != b"infe" {
            continue;
        }
        let bytes = read_box(file, &entry)?;
        let mut cursor = ByteCursor::new(&bytes);
        let version = cursor.read(1)?;
        cursor.skip(3)?;
        if version < 2 {
            continue;
        }
        let item_id = cursor.read(if version == 2 { 2 } else { 4 })?;
        cursor.skip(2)?;
        if cursor.bytes(4)? == b"Exif" {
            return Ok(Some(item_id as u32));
        }
    }
    Ok(None)
}

/// iloc：每个条目的位置写成「基址 + 第一段偏移」，各字段多宽由开头两字节说明。
fn item_offset(locations: &[u8], wanted: u32) -> Option<u64> {
    let mut cursor = ByteCursor::new(locations);
    let version = cursor.read(1).ok()?;
    cursor.skip(3).ok()?;
    let sizes = cursor.read(1).ok()?;
    let (offset_size, length_size) = ((sizes >> 4) as usize, (sizes & 0x0F) as usize);
    let sizes = cursor.read(1).ok()?;
    let base_offset_size = (sizes >> 4) as usize;
    let index_size = if version == 0 {
        0
    } else {
        (sizes & 0x0F) as usize
    };
    let id_size = if version < 2 { 2 } else { 4 };
    let item_count = cursor.read(id_size).ok()?;
    for _ in 0..item_count {
        let item_id = cursor.read(id_size).ok()?;
        let construction = if version == 0 {
            0
        } else {
            cursor.read(2).ok()? & 0x0F
        };
        cursor.skip(2).ok()?;
        let base_offset = cursor.read(base_offset_size).ok()?;
        let extent_count = cursor.read(2).ok()?;
        let mut first_offset = None;
        for _ in 0..extent_count {
            cursor.skip(index_size).ok()?;
            let offset = cursor.read(offset_size).ok()?;
            cursor.skip(length_size).ok()?;
            first_offset.get_or_insert(offset);
        }
        // 只认直接写在文件里的（构造方式 0）；放在 idat 里的照片不常见。
        if item_id == u64::from(wanted) && construction == 0 {
            return Some(base_offset + first_offset?);
        }
    }
    None
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn bytes(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self.position + length;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        self.position = end;
        Ok(bytes)
    }

    fn skip(&mut self, length: usize) -> io::Result<()> {
        self.bytes(length).map(|_| ())
    }

    /// 大端无符号整数，宽 0–8 字节（宽 0 是 0）。
    fn read(&mut self, length: usize) -> io::Result<u64> {
        Ok(self
            .bytes(length)?
            .iter()
            .fold(0, |value, byte| (value << 8) | u64::from(*byte)))
    }
}

fn read_tiff_exif<S: ByteSource>(reader: &mut TiffReader<S>, facts: &mut ImageFacts) {
    let Ok(ifd0) = reader.first_directory() else {
        return;
    };
    read_primary(reader, &ifd0, facts);
    if let Some(exif) = reader.linked_directory(&ifd0, TAG_EXIF_IFD) {
        read_exposure(reader, &exif, facts);
    }
    if let Some(gps) = reader.linked_directory(&ifd0, TAG_GPS_IFD) {
        read_location(reader, &gps, facts);
    }
}

fn read_primary<S: ByteSource>(reader: &mut TiffReader<S>, ifd: &Ifd, facts: &mut ImageFacts) {
    let make = reader.ascii(ifd, TAG_MAKE);
    let model = reader.ascii(ifd, TAG_MODEL);
    facts.camera = camera_name(make, model);
    if facts.taken_at.is_none() {
        facts.taken_at = reader.ascii(ifd, TAG_DATE_TIME).map(readable_exif_time);
    }
}

fn read_exposure<S: ByteSource>(reader: &mut TiffReader<S>, ifd: &Ifd, facts: &mut ImageFacts) {
    let fraction = |reader: &mut TiffReader<S>, tag| {
        reader
            .rationals(ifd, tag)
            .and_then(|values| values.first().copied())
            .filter(|(_, denominator)| *denominator != 0)
    };
    if let Some(taken_at) = reader.ascii(ifd, TAG_DATE_TIME_ORIGINAL) {
        facts.taken_at = Some(readable_exif_time(taken_at));
    }
    facts.exposure = fraction(reader, TAG_EXPOSURE_TIME).filter(|(numerator, _)| *numerator != 0);
    facts.f_number = fraction(reader, TAG_F_NUMBER)
        .map(|(numerator, denominator)| numerator as f32 / denominator as f32)
        .filter(|f_number| *f_number > 0.0);
    facts.focal_length_mm = fraction(reader, TAG_FOCAL_LENGTH)
        .map(|(numerator, denominator)| numerator as f32 / denominator as f32)
        .filter(|length| *length > 0.0);
    facts.focal_length_35mm = reader
        .first_unsigned(ifd, TAG_FOCAL_LENGTH_35MM)
        .filter(|length| *length > 0);
    facts.iso = reader.first_unsigned(ifd, TAG_ISO).filter(|iso| *iso > 0);
    facts.lens = reader.ascii(ifd, TAG_LENS_MODEL);
}

fn read_location<S: ByteSource>(reader: &mut TiffReader<S>, ifd: &Ifd, facts: &mut ImageFacts) {
    let mut coordinate = |value_tag, reference_tag, negative: &str| {
        let parts = reader.rationals(ifd, value_tag)?;
        let [degrees, minutes, seconds] = parts.as_slice() else {
            return None;
        };
        let part = |(numerator, denominator): (u32, u32)| {
            (denominator != 0).then(|| f64::from(numerator) / f64::from(denominator))
        };
        let value = part(*degrees)? + part(*minutes)? / 60.0 + part(*seconds)? / 3600.0;
        let sign = if reader.ascii(ifd, reference_tag).as_deref() == Some(negative) {
            -1.0
        } else {
            1.0
        };
        Some(sign * value)
    };
    let latitude = coordinate(TAG_GPS_LATITUDE, TAG_GPS_LATITUDE_REF, "S");
    let longitude = coordinate(TAG_GPS_LONGITUDE, TAG_GPS_LONGITUDE_REF, "W");
    // 没定位到的相机会写全 0。
    facts.location = latitude
        .zip(longitude)
        .filter(|(latitude, longitude)| *latitude != 0.0 || *longitude != 0.0);
}

/// 厂商名去掉公司后缀（`OLYMPUS CORPORATION` → `OLYMPUS`）；型号里已经带了品牌
/// （`Canon EOS R5`、`NIKON Z 6`、理光的 `PENTAX K-3 Mark III`）就只显示型号。
fn camera_name(make: Option<String>, model: Option<String>) -> Option<String> {
    const COMPANY_WORDS: [&str; 10] = [
        "CORPORATION",
        "CORP.",
        "CORP",
        "COMPANY",
        "IMAGING",
        "CO.,",
        "LTD.",
        "LTD",
        "INC.",
        "AG",
    ];
    let make = make
        .map(|make| {
            make.split_whitespace()
                .filter(|word| !COMPANY_WORDS.contains(&word.to_uppercase().as_str()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|make| !make.is_empty());
    match (make, model) {
        (Some(make), Some(model)) => {
            let brand = make
                .split_whitespace()
                .next()
                .unwrap_or(&make)
                .to_lowercase();
            // 型号的第一个词是全大写字母的品牌名（没有数字），后面还有型号。
            let mut words = model.split_whitespace();
            let model_starts_with_brand = words.next().is_some_and(|word| {
                word.len() >= 3 && word.chars().all(|character| character.is_ascii_uppercase())
            }) && words.next().is_some();
            if model.to_lowercase().contains(&brand) || model_starts_with_brand {
                Some(model)
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (make, model) => model.or(make),
    }
}

/// EXIF 的时间写成 `2024:05:01 12:34:56`，日期部分换成横线。
fn readable_exif_time(time: String) -> String {
    match time.split_once(' ') {
        Some((date, clock)) => format!("{} {clock}", date.replace(':', "-")),
        None => time,
    }
}

/// ICC 配置文件里 `desc` 标签的文字：v2 是 ASCII（`desc` 类型），v4 是 UTF-16（`mluc` 类型）。
fn icc_description(profile: &[u8]) -> Option<String> {
    let be32 = |at: usize| {
        profile
            .get(at..at + 4)
            .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let tag_count = be32(128)?;
    let tag = (0..tag_count.min(256)).find_map(|index| {
        let entry = 132 + index * 12;
        (profile.get(entry..entry + 4)? == b"desc").then(|| (be32(entry + 4), be32(entry + 8)))
    })?;
    let (offset, size) = (tag.0?, tag.1?);
    let data = profile.get(offset..offset.checked_add(size)?)?;
    let text = match data.get(..4)? {
        b"desc" => {
            let length = u32::from_be_bytes(data.get(8..12)?.try_into().ok()?) as usize;
            String::from_utf8_lossy(data.get(12..12 + length)?).into_owned()
        }
        b"mluc" => {
            let record_offset = u32::from_be_bytes(data.get(24..28)?.try_into().ok()?) as usize;
            let record_length = u32::from_be_bytes(data.get(20..24)?.try_into().ok()?) as usize;
            let units: Vec<u16> = data
                .get(record_offset..record_offset + record_length)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let text = text.trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests;
