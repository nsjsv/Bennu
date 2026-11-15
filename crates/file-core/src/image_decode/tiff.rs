//! TIFF 目录（IFD）读取：相机 RAW 大多是 TIFF 结构，JPEG 里的 EXIF 也是。只读要用的几种
//! 标签，按偏移随用随读，几十 MB 的 RAW 不整份读进内存。

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// 一个文件里最多看这么多个目录：坏文件的偏移可能互相指着转圈。
const MAX_VISITED_DIRECTORIES: usize = 64;
/// 一个标签最多读这么多个值（子目录偏移表之类），防坏文件报一个天文数字。
const MAX_VALUES_PER_TAG: u32 = 1024;

pub(crate) const TAG_COMPRESSION: u16 = 0x0103;
pub(crate) const TAG_STRIP_OFFSETS: u16 = 0x0111;
pub(crate) const TAG_ORIENTATION: u16 = 0x0112;
pub(crate) const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
pub(crate) const TAG_SUB_IFDS: u16 = 0x014A;
pub(crate) const TAG_JPEG_OFFSET: u16 = 0x0201;
pub(crate) const TAG_JPEG_LENGTH: u16 = 0x0202;
pub(crate) const TAG_EXIF_IFD: u16 = 0x8769;
pub(crate) const TAG_GPS_IFD: u16 = 0x8825;

const TYPE_BYTE: u16 = 1;
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_UNDEFINED: u16 = 7;
const TYPE_SSHORT: u16 = 8;
const TYPE_SLONG: u16 = 9;
const TYPE_SRATIONAL: u16 = 10;
const TYPE_IFD: u16 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

/// 可以按偏移读字节的来源：磁盘文件（RAW）或内存里的一段（JPEG 的 EXIF 块）。
pub(crate) trait ByteSource {
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;
}

impl ByteSource for File {
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.read_exact(buffer)
    }
}

impl<T: ByteSource> ByteSource for &mut T {
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        (**self).read_exact_at(offset, buffer)
    }
}

impl ByteSource for &[u8] {
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| truncated())?;
        let end = start.checked_add(buffer.len()).ok_or_else(truncated)?;
        let bytes = self.get(start..end).ok_or_else(truncated)?;
        buffer.copy_from_slice(bytes);
        Ok(())
    }
}

fn truncated() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "TIFF structure points past the data",
    )
}

fn malformed(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.to_owned())
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IfdEntry {
    pub(crate) tag: u16,
    field_type: u16,
    count: u32,
    /// 值放得下 4 字节时就是值本身，否则是值的偏移（相对 TIFF 头）。
    inline: [u8; 4],
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Ifd {
    pub(crate) entries: Vec<IfdEntry>,
    next: u32,
}

impl Ifd {
    pub(crate) fn entry(&self, tag: u16) -> Option<&IfdEntry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }
}

/// `base` 是 TIFF 头在来源里的位置：目录里的偏移都相对它。
pub(crate) struct TiffReader<S> {
    source: S,
    base: u64,
    order: ByteOrder,
    first_ifd: u32,
}

impl<S: ByteSource> TiffReader<S> {
    /// 认 `II`/`MM` 字节序；第二个字段（标准 TIFF 是 42）各家 RAW 写的不一样
    /// （ORF 写 `RO`、RW2 写 0x55），不检查。
    pub(crate) fn open(mut source: S, base: u64) -> io::Result<Self> {
        let mut header = [0; 8];
        source.read_exact_at(base, &mut header)?;
        let order = match &header[..2] {
            b"II" => ByteOrder::Little,
            b"MM" => ByteOrder::Big,
            _ => return Err(malformed("not a TIFF structure")),
        };
        let mut reader = Self {
            source,
            base,
            order,
            first_ifd: 0,
        };
        reader.first_ifd = reader.u32_from(&header[4..8]);
        Ok(reader)
    }

    /// IFD0 及其后续目录、它们的子目录和 EXIF 目录，按发现顺序；IFD0 排第一。
    pub(crate) fn directories(&mut self) -> io::Result<Vec<Ifd>> {
        let mut pending = vec![self.first_ifd];
        let mut visited = Vec::new();
        let mut directories = Vec::new();
        while let Some(offset) = pending.pop() {
            if offset == 0 || visited.contains(&offset) || visited.len() >= MAX_VISITED_DIRECTORIES
            {
                continue;
            }
            visited.push(offset);
            let ifd = match self.read_ifd(offset) {
                Ok(ifd) => ifd,
                // IFD0 读不出来就不是能用的 TIFF；后面的目录坏了只丢那一个。
                Err(error) if directories.is_empty() => return Err(error),
                Err(_) => continue,
            };
            let mut children = Vec::new();
            for tag in [TAG_SUB_IFDS, TAG_EXIF_IFD] {
                if let Some(entry) = ifd.entry(tag) {
                    children.extend(self.unsigned_values(entry).unwrap_or_default());
                }
            }
            // 栈后进先出：先压后续目录再压子目录，子目录先读，IFD0 的顺序保持在前。
            pending.push(ifd.next);
            pending.extend(children.into_iter().rev());
            directories.push(ifd);
        }
        Ok(directories)
    }

    pub(crate) fn first_directory(&mut self) -> io::Result<Ifd> {
        self.read_ifd(self.first_ifd)
    }

    /// 顺着指针标签（EXIF、GPS 目录）读子目录。
    pub(crate) fn linked_directory(&mut self, ifd: &Ifd, tag: u16) -> Option<Ifd> {
        let offset = self.first_unsigned(ifd, tag)?;
        self.read_ifd(offset).ok()
    }

    /// 文字标签，去掉末尾的 NUL 和空白；空的算没有。
    pub(crate) fn ascii(&mut self, ifd: &Ifd, tag: u16) -> Option<String> {
        let entry = *ifd.entry(tag)?;
        if entry.field_type != TYPE_ASCII || entry.count > MAX_VALUES_PER_TAG {
            return None;
        }
        let bytes = self.value_bytes(&entry, 1).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_owned())
    }

    /// 分数标签（分子, 分母）；有符号的按无符号读，曝光、光圈、坐标都不会是负数。
    pub(crate) fn rationals(&mut self, ifd: &Ifd, tag: u16) -> Option<Vec<(u32, u32)>> {
        let entry = *ifd.entry(tag)?;
        if !matches!(entry.field_type, TYPE_RATIONAL | TYPE_SRATIONAL)
            || entry.count > MAX_VALUES_PER_TAG
        {
            return None;
        }
        let bytes = self.value_bytes(&entry, 8).ok()?;
        Some(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|value| (self.u32_from(&value[..4]), self.u32_from(&value[4..])))
                .collect(),
        )
    }

    fn read_ifd(&mut self, offset: u32) -> io::Result<Ifd> {
        let position = self.base + u64::from(offset);
        let mut count = [0; 2];
        self.source.read_exact_at(position, &mut count)?;
        let count = self.u16_from(&count);
        let mut table = vec![0; usize::from(count) * 12 + 4];
        self.source.read_exact_at(position + 2, &mut table)?;
        let entries = table[..usize::from(count) * 12]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|raw| IfdEntry {
                tag: self.u16_from(&raw[0..2]),
                field_type: self.u16_from(&raw[2..4]),
                count: self.u32_from(&raw[4..8]),
                inline: [raw[8], raw[9], raw[10], raw[11]],
            })
            .collect();
        let next = self.u32_from(&table[usize::from(count) * 12..]);
        Ok(Ifd { entries, next })
    }

    /// 无符号整数标签的全部值（BYTE/SHORT/LONG/IFD）。
    pub(crate) fn unsigned_values(&mut self, entry: &IfdEntry) -> io::Result<Vec<u32>> {
        let width = match entry.field_type {
            TYPE_BYTE | TYPE_UNDEFINED => 1,
            TYPE_SHORT | TYPE_SSHORT => 2,
            TYPE_LONG | TYPE_SLONG | TYPE_IFD => 4,
            _ => return Err(malformed("TIFF tag is not an integer")),
        };
        if entry.count > MAX_VALUES_PER_TAG {
            return Err(malformed("TIFF tag has too many values"));
        }
        let bytes = self.value_bytes(entry, width)?;
        Ok(bytes
            .chunks_exact(width)
            .map(|value| match width {
                1 => u32::from(value[0]),
                2 => u32::from(self.u16_from(value)),
                _ => self.u32_from(value),
            })
            .collect())
    }

    pub(crate) fn first_unsigned(&mut self, ifd: &Ifd, tag: u16) -> Option<u32> {
        let entry = *ifd.entry(tag)?;
        self.unsigned_values(&entry).ok()?.first().copied()
    }

    /// UNDEFINED 类型标签的值在文件里的位置和长度（不读出来；RW2 的整张 JPEG 就这样存）。
    pub(crate) fn blob_range(&self, entry: &IfdEntry) -> Option<(u64, u64)> {
        if entry.field_type != TYPE_UNDEFINED || entry.count <= 4 {
            return None;
        }
        Some((
            self.base + u64::from(self.u32_from(&entry.inline)),
            u64::from(entry.count),
        ))
    }

    /// 目录里的偏移换成来源里的绝对位置。
    pub(crate) fn absolute(&self, offset: u32) -> u64 {
        self.base + u64::from(offset)
    }

    pub(crate) fn into_source(self) -> S {
        self.source
    }

    fn value_bytes(&mut self, entry: &IfdEntry, width: usize) -> io::Result<Vec<u8>> {
        let length = width * entry.count as usize;
        if length <= 4 {
            return Ok(entry.inline[..length].to_vec());
        }
        let mut bytes = vec![0; length];
        let offset = self.absolute(self.u32_from(&entry.inline));
        self.source.read_exact_at(offset, &mut bytes)?;
        Ok(bytes)
    }

    fn u16_from(&self, bytes: &[u8]) -> u16 {
        let bytes = [bytes[0], bytes[1]];
        match self.order {
            ByteOrder::Little => u16::from_le_bytes(bytes),
            ByteOrder::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32_from(&self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self.order {
            ByteOrder::Little => u32::from_le_bytes(bytes),
            ByteOrder::Big => u32::from_be_bytes(bytes),
        }
    }
}
