//! 相机 RAW 不真正去解（要另装解码库、一张几秒），而是取相机拍摄时存在文件里的 JPEG
//! 预览图：和相机屏幕上看到的一样，而且很快。大多数 RAW 是 TIFF 结构，预览图挂在某个目录里；
//! 富士 RAF 在文件头写了预览图的位置；佳能 CR3 是 ISO 媒体结构，第一条轨道就是整张 JPEG。

use std::fs::File;
use std::io::{self, Cursor};
use std::path::Path;

use image::codecs::jpeg::JpegDecoder;
use image::metadata::Orientation;
use image::ImageDecoder;

use super::iso_media::{box_uuid, descend, iso_boxes, IsoBox};
use super::tiff::{
    ByteSource, Ifd, TiffReader, TAG_COMPRESSION, TAG_JPEG_LENGTH, TAG_JPEG_OFFSET,
    TAG_ORIENTATION, TAG_STRIP_BYTE_COUNTS, TAG_STRIP_OFFSETS,
};
use super::ImageDecodeError;

const RAF_SIGNATURE: &[u8] = b"FUJIFILMCCD-RAW ";
/// RAF 文件头里预览图偏移、长度所在位置（大端 u32）。
const RAF_PREVIEW_OFFSET_AT: u64 = 84;
/// 奥林巴斯把预览图放在厂商私有区（EXIF 的 MakerNote）里：私有区以 `OLYMPUS\0` 加字节序开头，
/// 第一个目录在第 12 字节，偏移从私有区开头算；CameraSettings 子目录里记着预览图的位置和长度。
const TAG_MAKER_NOTE: u16 = 0x927C;
const OLYMPUS_MAKER_NOTE: &[u8] = b"OLYMPUS\0";
const OLYMPUS_FIRST_IFD: u32 = 12;
const TAG_OLYMPUS_CAMERA_SETTINGS: u16 = 0x2020;
const TAG_OLYMPUS_PREVIEW_START: u16 = 0x0101;
const TAG_OLYMPUS_PREVIEW_LENGTH: u16 = 0x0102;
/// RW2 把整张 JPEG 放在 IFD0 的这个标签里。
const TAG_RW2_JPEG_FROM_RAW: u16 = 0x002E;
/// TIFF 压缩方式 6、7 是 JPEG；但原始数据本身也常用无损 JPEG 存，要看帧头分辨。
const JPEG_COMPRESSIONS: [u32; 2] = [6, 7];
/// 看帧头只读开头这么多：前面最多一个 64 KB 的 EXIF 段。
const JPEG_HEADER_PREFIX_BYTES: u64 = 128 * 1024;
/// 小于这个的是列表用的小缩略图，不当预览。
const MIN_PREVIEW_BYTES: u64 = 1024;
const CANON_METADATA_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
const CANON_PREVIEW_UUID: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

type PreviewDecoder<'a> = JpegDecoder<Cursor<&'a [u8]>>;

/// 选中的预览图：JPEG 字节，加上 RAW 记的方向（RAF 的预览图自己带 EXIF 方向，这里是 None）。
pub(super) struct EmbeddedPreview {
    jpeg: Vec<u8>,
    raw_orientation: Option<Orientation>,
}

impl EmbeddedPreview {
    pub(super) fn jpeg(&self) -> &[u8] {
        &self.jpeg
    }

    /// 解码器（只读了头）、该用的方向。
    pub(super) fn decoder(&self) -> Result<(PreviewDecoder<'_>, Orientation), ImageDecodeError> {
        let mut decoder = JpegDecoder::new(Cursor::new(self.jpeg.as_slice()))?;
        let orientation = match self.raw_orientation {
            Some(orientation) => orientation,
            None => decoder.orientation()?,
        };
        Ok((decoder, orientation))
    }
}

/// RAW 文件的三种结构，预览图和拍摄参数各在不同地方。
pub(super) enum RawLayout {
    Tiff,
    Raf,
    Cr3(Cr3Metadata),
}

/// CR3 把 IFD0、EXIF、GPS 三个目录分别存成三段 TIFF（CMT1、CMT2、CMT4），这里是各段的开头。
#[derive(Default)]
pub(super) struct Cr3Metadata {
    pub(super) primary: Option<u64>,
    pub(super) exif: Option<u64>,
    pub(super) gps: Option<u64>,
}

pub(super) fn raw_layout(file: &mut File, file_len: u64) -> io::Result<RawLayout> {
    let mut head = [0; 16];
    let head_len = file_len.min(16) as usize;
    file.read_exact_at(0, &mut head[..head_len])?;
    Ok(if head == RAF_SIGNATURE {
        RawLayout::Raf
    } else if &head[4..8] == b"ftyp" {
        RawLayout::Cr3(cr3_metadata(file, file_len)?)
    } else {
        RawLayout::Tiff
    })
}

pub(super) fn read_embedded_preview(path: &Path) -> Result<EmbeddedPreview, ImageDecodeError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let (candidates, raw_orientation) = match raw_layout(&mut file, file_len)? {
        RawLayout::Raf => (raf_candidates(&mut file)?, None),
        RawLayout::Cr3(metadata) => {
            let orientation = match metadata.primary {
                Some(start) => {
                    tiff_orientation(&mut file, start).unwrap_or(Orientation::NoTransforms)
                }
                None => Orientation::NoTransforms,
            };
            (cr3_candidates(&mut file, file_len)?, Some(orientation))
        }
        RawLayout::Tiff => {
            let mut reader = TiffReader::open(file, 0)?;
            let directories = reader.directories()?;
            let orientation = directories
                .first()
                .and_then(|ifd0| reader.first_unsigned(ifd0, TAG_ORIENTATION))
                .map_or(Orientation::NoTransforms, exif_orientation);
            let candidates = tiff_candidates(&mut reader, &directories);
            file = reader.into_source();
            (candidates, Some(orientation))
        }
    };
    let jpeg = largest_displayable_jpeg(&mut file, file_len, candidates)?;
    Ok(EmbeddedPreview {
        jpeg,
        raw_orientation,
    })
}

/// 文件里一段可能是 JPEG 的数据：（绝对位置，长度）。
type ByteRange = (u64, u64);

fn tiff_candidates<S: ByteSource>(
    reader: &mut TiffReader<S>,
    directories: &[Ifd],
) -> Vec<ByteRange> {
    let mut candidates = Vec::new();
    for (index, ifd) in directories.iter().enumerate() {
        if let (Some(offset), Some(length)) = (
            reader.first_unsigned(ifd, TAG_JPEG_OFFSET),
            reader.first_unsigned(ifd, TAG_JPEG_LENGTH),
        ) {
            candidates.push((reader.absolute(offset), u64::from(length)));
        }
        let compressed_as_jpeg = reader
            .first_unsigned(ifd, TAG_COMPRESSION)
            .is_some_and(|compression| JPEG_COMPRESSIONS.contains(&compression));
        if compressed_as_jpeg {
            let offsets = ifd.entry(TAG_STRIP_OFFSETS).copied();
            let lengths = ifd.entry(TAG_STRIP_BYTE_COUNTS).copied();
            if let (Some(offsets), Some(lengths)) = (offsets, lengths) {
                // 分成多条存的是原始数据，预览图都是一整块。
                if let (Ok(offsets), Ok(lengths)) = (
                    reader.unsigned_values(&offsets),
                    reader.unsigned_values(&lengths),
                ) {
                    if let ([offset], [length]) = (offsets.as_slice(), lengths.as_slice()) {
                        candidates.push((reader.absolute(*offset), u64::from(*length)));
                    }
                }
            }
        }
        if index == 0 {
            if let Some(range) = ifd
                .entry(TAG_RW2_JPEG_FROM_RAW)
                .and_then(|entry| reader.blob_range(entry))
            {
                candidates.push(range);
            }
        }
    }
    candidates.extend(olympus_preview(reader, directories));
    candidates
}

fn olympus_preview<S: ByteSource>(
    reader: &mut TiffReader<S>,
    directories: &[Ifd],
) -> Option<ByteRange> {
    let (start, _) = directories.iter().find_map(|ifd| {
        ifd.entry(TAG_MAKER_NOTE)
            .and_then(|entry| reader.blob_range(entry))
    })?;
    let mut header = [0; 12];
    reader.source_mut().read_exact_at(start, &mut header).ok()?;
    if !header.starts_with(OLYMPUS_MAKER_NOTE) {
        return None;
    }
    let little_endian = &header[8..10] == b"II";
    let mut note =
        TiffReader::embedded(reader.source_mut(), start, little_endian, OLYMPUS_FIRST_IFD);
    let ifd = note.first_directory().ok()?;
    let settings = note.linked_directory(&ifd, TAG_OLYMPUS_CAMERA_SETTINGS)?;
    let offset = note.first_unsigned(&settings, TAG_OLYMPUS_PREVIEW_START)?;
    let length = note.first_unsigned(&settings, TAG_OLYMPUS_PREVIEW_LENGTH)?;
    Some((note.absolute(offset), u64::from(length)))
}

fn raf_candidates(file: &mut File) -> io::Result<Vec<ByteRange>> {
    let mut location = [0; 8];
    file.read_exact_at(RAF_PREVIEW_OFFSET_AT, &mut location)?;
    let offset = u32::from_be_bytes([location[0], location[1], location[2], location[3]]);
    let length = u32::from_be_bytes([location[4], location[5], location[6], location[7]]);
    Ok(vec![(u64::from(offset), u64::from(length))])
}

/// CR3：第一条轨道的第一帧是整张 JPEG；另有一个中等大小的预览框（PRVW）。方向记在元数据框
/// CMT1 里（一段 TIFF）。
fn cr3_metadata(file: &mut File, file_len: u64) -> io::Result<Cr3Metadata> {
    let mut metadata = Cr3Metadata::default();
    for top in iso_boxes(file, 0, file_len)? {
        if &top.kind != b"moov" {
            continue;
        }
        for child in iso_boxes(file, top.payload, top.end)? {
            if &child.kind != b"uuid" || box_uuid(file, &child)? != CANON_METADATA_UUID {
                continue;
            }
            for section in iso_boxes(file, child.payload + 16, child.end)? {
                match &section.kind {
                    b"CMT1" => metadata.primary = Some(section.payload),
                    b"CMT2" => metadata.exif = Some(section.payload),
                    b"CMT4" => metadata.gps = Some(section.payload),
                    _ => {}
                }
            }
        }
    }
    Ok(metadata)
}

/// CR3：第一条轨道的第一帧是整张 JPEG；另有一个中等大小的预览框（PRVW）。
fn cr3_candidates(file: &mut File, file_len: u64) -> io::Result<Vec<ByteRange>> {
    let mut candidates = Vec::new();
    for top in iso_boxes(file, 0, file_len)? {
        match &top.kind {
            b"moov" => {
                if let Some(range) = first_track_sample(file, &top)? {
                    candidates.push(range);
                }
            }
            b"uuid" if box_uuid(file, &top)? == CANON_PREVIEW_UUID => {
                // UUID 之后还有 8 字节才是 PRVW 框；框里几个尺寸字段之后才是 JPEG。
                for preview in iso_boxes(file, top.payload + 24, top.end)? {
                    if &preview.kind == b"PRVW" {
                        let mut head = [0; 64];
                        let readable = (preview.end - preview.payload).min(64) as usize;
                        file.read_exact_at(preview.payload, &mut head[..readable])?;
                        if let Some(start) = head[..readable]
                            .windows(2)
                            .position(|pair| pair == [0xFF, 0xD8])
                        {
                            let start = preview.payload + start as u64;
                            candidates.push((start, preview.end - start));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(candidates)
}

fn tiff_orientation(file: &mut File, tiff_start: u64) -> io::Result<Orientation> {
    let mut reader = TiffReader::open(file, tiff_start)?;
    let directories = reader.directories()?;
    Ok(directories
        .first()
        .and_then(|ifd0| reader.first_unsigned(ifd0, TAG_ORIENTATION))
        .map_or(Orientation::NoTransforms, exif_orientation))
}

fn first_track_sample(file: &mut File, moov: &IsoBox) -> io::Result<Option<ByteRange>> {
    let Some(track) = iso_boxes(file, moov.payload, moov.end)?
        .into_iter()
        .find(|child| &child.kind == b"trak")
    else {
        return Ok(None);
    };
    let Some(table) = descend(file, &track, &[b"mdia", b"minf", b"stbl"])? else {
        return Ok(None);
    };
    let mut size = None;
    let mut offset = None;
    for child in iso_boxes(file, table.payload, table.end)? {
        let mut fields = [0; 16];
        match &child.kind {
            // 版本/标志 4 字节，统一大小 4 字节（0 表示后面逐帧列出），帧数 4 字节，第一帧大小。
            b"stsz" => {
                file.read_exact_at(child.payload, &mut fields)?;
                let uniform = u32::from_be_bytes([fields[4], fields[5], fields[6], fields[7]]);
                size = Some(if uniform != 0 {
                    uniform
                } else {
                    u32::from_be_bytes([fields[12], fields[13], fields[14], fields[15]])
                });
            }
            // 版本/标志 4 字节，项数 4 字节，第一项偏移（co64 8 字节、stco 4 字节）。
            b"co64" => {
                file.read_exact_at(child.payload, &mut fields)?;
                offset = Some(u64::from_be_bytes([
                    fields[8], fields[9], fields[10], fields[11], fields[12], fields[13],
                    fields[14], fields[15],
                ]));
            }
            b"stco" => {
                file.read_exact_at(child.payload, &mut fields[..12])?;
                offset = Some(u64::from(u32::from_be_bytes([
                    fields[8], fields[9], fields[10], fields[11],
                ])));
            }
            _ => {}
        }
    }
    Ok(size
        .zip(offset)
        .map(|(size, offset)| (offset, u64::from(size))))
}

/// 候选里最大的、看帧头是普通 JPEG（不是存原始数据用的无损 JPEG）的那段。
fn largest_displayable_jpeg(
    file: &mut File,
    file_len: u64,
    mut candidates: Vec<ByteRange>,
) -> Result<Vec<u8>, ImageDecodeError> {
    candidates.retain(|(offset, length)| {
        *length >= MIN_PREVIEW_BYTES
            && offset
                .checked_add(*length)
                .is_some_and(|end| end <= file_len)
    });
    candidates.sort_by_key(|(_, length)| std::cmp::Reverse(*length));
    candidates.dedup();
    for (offset, length) in candidates {
        let mut prefix = vec![0; length.min(JPEG_HEADER_PREFIX_BYTES) as usize];
        file.read_exact_at(offset, &mut prefix)?;
        if !is_displayable_jpeg(&prefix) {
            continue;
        }
        let mut jpeg = vec![0; length as usize];
        file.read_exact_at(offset, &mut jpeg)?;
        return Ok(jpeg);
    }
    Err(ImageDecodeError::NoEmbeddedPreview)
}

/// 走一遍段标记找帧头：基线/扩展/渐进（SOF0–2）能显示，无损和算术编码的不行。
fn is_displayable_jpeg(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return false;
    }
    let mut position = 2;
    while position + 4 <= bytes.len() {
        if bytes[position] != 0xFF {
            return false;
        }
        let marker = bytes[position + 1];
        match marker {
            0xFF => {
                position += 1;
                continue;
            }
            0xC0..=0xC2 => return true,
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xD9 | 0xDA => return false,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([
            bytes[position + 2],
            bytes[position + 3],
        ]));
        position += 2 + length;
    }
    false
}

pub(super) fn exif_orientation(value: u32) -> Orientation {
    u8::try_from(value)
        .ok()
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms)
}

#[cfg(test)]
pub(super) mod test_files {
    //! 合成的 RAW：IFD0（方向 + 一张小缩略图）+ 子目录（大预览图），够测选图和转向。

    pub(in super::super) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        // 有起伏的像素，压出来才不会小到被当成列表缩略图。
        let pixels: Vec<u8> = (0..width as usize * height as usize * 3)
            .map(|index| (index * 7 % 251) as u8)
            .collect();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(&pixels, width, height, image::ExtendedColorType::Rgb8)
            .expect("encode test jpeg");
        bytes
    }

    /// 小端 TIFF：IFD0 = 方向 + 子目录 + 一段 JPEG 缩略图（压缩方式 6 的单条）；子目录 = JPEG 偏移/长度。
    pub(in super::super) fn tiff_raw(
        orientation: u16,
        thumbnail: &[u8],
        preview: &[u8],
    ) -> Vec<u8> {
        let ifd0_offset = 8u32;
        let ifd0_entries = 5u32;
        let ifd0_len = 2 + ifd0_entries * 12 + 4;
        let sub_offset = ifd0_offset + ifd0_len;
        let sub_entries = 2u32;
        let sub_len = 2 + sub_entries * 12 + 4;
        let thumbnail_offset = sub_offset + sub_len;
        let preview_offset = thumbnail_offset + thumbnail.len() as u32;

        let mut bytes = b"II*\0".to_vec();
        bytes.extend(ifd0_offset.to_le_bytes());
        let entry = |bytes: &mut Vec<u8>, tag: u16, field_type: u16, value: u32| {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(field_type.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        };
        bytes.extend((ifd0_entries as u16).to_le_bytes());
        entry(&mut bytes, 0x0103, 3, 6);
        entry(&mut bytes, 0x0111, 4, thumbnail_offset);
        entry(&mut bytes, 0x0112, 3, u32::from(orientation));
        entry(&mut bytes, 0x0117, 4, thumbnail.len() as u32);
        entry(&mut bytes, 0x014A, 4, sub_offset);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend((sub_entries as u16).to_le_bytes());
        entry(&mut bytes, 0x0201, 4, preview_offset);
        entry(&mut bytes, 0x0202, 4, preview.len() as u32);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend_from_slice(thumbnail);
        bytes.extend_from_slice(preview);
        bytes
    }

    /// ORF：IFD0 → EXIF 目录 → MakerNote（`OLYMPUS\0II` 头，偏移从私有区开头算）
    /// → CameraSettings 子目录记着预览图的位置和长度。
    pub(in super::super) fn olympus_orf(preview: &[u8]) -> Vec<u8> {
        let entry = |bytes: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32| {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(field_type.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        };
        let ifd0 = 8u32;
        let exif = ifd0 + 2 + 12 + 4;
        let note = exif + 2 + 12 + 4;
        // 私有区里：12 字节头，一个目录（1 项），CameraSettings（2 项），然后是 JPEG。
        let settings_relative = 12 + 2 + 12 + 4;
        let preview_relative = settings_relative + 2 + 2 * 12 + 4;
        let note_len = preview_relative + preview.len() as u32;

        let mut bytes = b"IIRO".to_vec();
        bytes.extend(ifd0.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        entry(&mut bytes, 0x8769, 4, 1, exif);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        entry(&mut bytes, 0x927C, 7, note_len, note);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(b"OLYMPUS\0II\x03\0");
        bytes.extend(1u16.to_le_bytes());
        entry(&mut bytes, 0x2020, 13, 1, settings_relative);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        entry(&mut bytes, 0x0101, 4, 1, preview_relative);
        entry(&mut bytes, 0x0102, 4, 1, preview.len() as u32);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend_from_slice(preview);
        bytes
    }

    /// RAF：文件头签名，84 处写预览图位置。
    pub(in super::super) fn raf(preview: &[u8]) -> Vec<u8> {
        let mut bytes = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        bytes.resize(84, 0);
        let offset = 160u32;
        bytes.extend(offset.to_be_bytes());
        bytes.extend((preview.len() as u32).to_be_bytes());
        bytes.resize(offset as usize, 0);
        bytes.extend_from_slice(preview);
        bytes
    }

    /// CR3：ftyp + moov（一条轨道，stsz/co64 指向 mdat 里的 JPEG）+ mdat。
    pub(in super::super) fn cr3(preview: &[u8]) -> Vec<u8> {
        fn iso_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(kind);
            bytes.extend_from_slice(payload);
            bytes
        }
        let ftyp = iso_box(b"ftyp", b"crx \0\0\0\x01crx isom");
        let build = |offset: u64| {
            let mut stsz = vec![0; 4];
            stsz.extend((preview.len() as u32).to_be_bytes());
            stsz.extend(1u32.to_be_bytes());
            let mut co64 = vec![0; 4];
            co64.extend(1u32.to_be_bytes());
            co64.extend(offset.to_be_bytes());
            let stbl = [iso_box(b"stsz", &stsz), iso_box(b"co64", &co64)].concat();
            let minf = iso_box(b"minf", &iso_box(b"stbl", &stbl));
            let trak = iso_box(b"trak", &iso_box(b"mdia", &minf));
            iso_box(b"moov", &trak)
        };
        let moov_len = build(0).len() as u64;
        let offset = (ftyp.len() as u64) + moov_len + 8;
        [ftyp, build(offset), iso_box(b"mdat", preview)].concat()
    }
}
