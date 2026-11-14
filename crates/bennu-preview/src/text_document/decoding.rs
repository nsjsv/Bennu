//! 文本在文件里的字节形态：编码、正文起点（BOM 之后）、换行单元、超长行可以切开的位置。
//!
//! 切段位置只由文件字节决定，与从哪个方向、从哪里开始读无关：第 k 段从
//! `line_start + k × SEGMENT_BYTES` 处或之后的第一个安全切点开始。安全切点不切开字符；
//! Unicode 编码还不切开字形簇（组合符号、emoji 连接序列）。

use encoding_rs::{
    Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, ISO_2022_JP, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8,
};
use unicode_segmentation::UnicodeSegmentation;

use super::line_breaks::LineBreaks;

/// 超长行每段的目标字节数（英文约 1 万字、中文约 3 千字）。
pub(crate) const SEGMENT_BYTES: u64 = 10_000;
/// 找安全切点要看切点之后多少字节：最长的字形簇也落在这个范围里。
pub(crate) const CUT_LOOKAHEAD_BYTES: u64 = 128;
/// 字形簇边界前后各看这么多字节的上下文。
const GRAPHEME_CONTEXT_BYTES: u64 = 64;
/// 旧式多字节编码往回找同步点的最远距离；找不到就从调用方给的已知边界解码。
pub(crate) const MULTI_BYTE_RESYNC_BYTES: u64 = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextDecoding {
    encoding: &'static Encoding,
    data_start: u64,
}

/// 各编码找字符边界的办法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharBoundaries {
    Utf8,
    Utf16 {
        little_endian: bool,
    },
    /// 每个字节就是一个字符。
    SingleByte,
    /// GBK/GB18030、Big5、Shift_JIS、EUC-JP、EUC-KR：按首字节就能知道一个字几个字节；
    /// 低于 0x30 的字节一定是单字节字符（这些编码的后续字节都不落在这个范围，GB18030
    /// 四字节码的第 2、4 字节是 0x30–0x39），从它开始逐字往后数就能找到边界。
    MultiByte(LegacyMultiByte),
    /// ISO-2022-JP 靠转义序列切换单/双字节状态，行中间无法得知当前状态；实际文件里不会
    /// 有上万字节的一行，切点就落在目标字节上。
    Stateful,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyMultiByte {
    Gb18030,
    Big5,
    ShiftJis,
    EucJp,
    EucKr,
}

impl LegacyMultiByte {
    /// 从字符起点 `bytes[0]` 开始的这个字占几个字节（坏序列按首字节规则算，结果确定即可）。
    fn char_len(self, bytes: &[u8]) -> usize {
        let lead = bytes[0];
        let len = match self {
            Self::Gb18030 => match lead {
                0x81..=0xfe if bytes.get(1).is_some_and(u8::is_ascii_digit) => 4,
                0x81..=0xfe => 2,
                _ => 1,
            },
            Self::Big5 | Self::EucKr => match lead {
                0x81..=0xfe => 2,
                _ => 1,
            },
            Self::ShiftJis => match lead {
                0x81..=0x9f | 0xe0..=0xfc => 2,
                _ => 1,
            },
            Self::EucJp => match lead {
                0x8f => 3,
                0x8e | 0xa1..=0xfe => 2,
                _ => 1,
            },
        };
        len.min(bytes.len())
    }
}

impl TextDecoding {
    pub fn new(encoding: &'static Encoding, data_start: u64) -> Self {
        Self {
            encoding,
            data_start,
        }
    }

    pub fn encoding(self) -> &'static Encoding {
        self.encoding
    }

    /// 正文第一个字节（BOM 之后）。
    pub fn data_start(self) -> u64 {
        self.data_start
    }

    pub(crate) fn line_breaks(self) -> LineBreaks {
        LineBreaks::for_encoding(self.encoding)
    }

    /// `bytes` 起止都在字符边界上。认不出的字节变成替换符。
    pub(crate) fn decode(self, bytes: &[u8]) -> String {
        self.encoding
            .decode_without_bom_handling(bytes)
            .0
            .into_owned()
    }

    /// `target` 处或之后的第一个安全切点。`bytes` 是文件里从 `bytes_offset` 开始的一段，
    /// 至少要盖到 `target + CUT_LOOKAHEAD_BYTES` 或文件尾。`known_boundary` 是 `bytes` 里
    /// 一个已知的字符边界（行首或上一段的起点）；没有时（随机定位）旧式多字节编码只能在
    /// `bytes` 里找同步点，找不到返回 None，调用方从行首重读。
    pub(crate) fn segment_cut(
        self,
        bytes: &[u8],
        bytes_offset: u64,
        known_boundary: Option<u64>,
        target: u64,
    ) -> Option<u64> {
        let end = bytes_offset + bytes.len() as u64;
        let target = target.min(end);
        let at = |offset: u64| bytes[(offset - bytes_offset) as usize];
        match self.char_boundaries() {
            CharBoundaries::SingleByte | CharBoundaries::Stateful => Some(target),
            CharBoundaries::Utf8 | CharBoundaries::Utf16 { .. } => {
                let cut = self.char_boundary_at_or_after(bytes, bytes_offset, target);
                Some(self.grapheme_cut(bytes, bytes_offset, cut))
            }
            CharBoundaries::MultiByte(legacy) => {
                let lowest = known_boundary
                    .unwrap_or(bytes_offset)
                    .max(target.saturating_sub(MULTI_BYTE_RESYNC_BYTES));
                let mut cut = (lowest..target)
                    .rev()
                    .find(|offset| at(*offset) < 0x30)
                    .or(known_boundary)?;
                while cut < target {
                    cut += legacy.char_len(&bytes[(cut - bytes_offset) as usize..]) as u64;
                }
                Some(cut)
            }
        }
    }

    fn char_boundaries(self) -> CharBoundaries {
        let encoding = self.encoding;
        if encoding == UTF_8 {
            CharBoundaries::Utf8
        } else if encoding == UTF_16LE {
            CharBoundaries::Utf16 {
                little_endian: true,
            }
        } else if encoding == UTF_16BE {
            CharBoundaries::Utf16 {
                little_endian: false,
            }
        } else if encoding == GBK || encoding == GB18030 {
            CharBoundaries::MultiByte(LegacyMultiByte::Gb18030)
        } else if encoding == BIG5 {
            CharBoundaries::MultiByte(LegacyMultiByte::Big5)
        } else if encoding == SHIFT_JIS {
            CharBoundaries::MultiByte(LegacyMultiByte::ShiftJis)
        } else if encoding == EUC_JP {
            CharBoundaries::MultiByte(LegacyMultiByte::EucJp)
        } else if encoding == EUC_KR {
            CharBoundaries::MultiByte(LegacyMultiByte::EucKr)
        } else if encoding == ISO_2022_JP {
            CharBoundaries::Stateful
        } else {
            CharBoundaries::SingleByte
        }
    }

    /// `cut` 是字符边界；往后挪到字形簇边界（不把组合符号、emoji 序列拆到下一段）。
    fn grapheme_cut(self, bytes: &[u8], bytes_offset: u64, cut: u64) -> u64 {
        let end = bytes_offset + bytes.len() as u64;
        let window_start = self.char_boundary_at_or_after(
            bytes,
            bytes_offset,
            cut.saturating_sub(GRAPHEME_CONTEXT_BYTES).max(bytes_offset),
        );
        let window_end = self.char_boundary_at_or_after(
            bytes,
            bytes_offset,
            (cut + GRAPHEME_CONTEXT_BYTES).min(end),
        );
        let window =
            &bytes[(window_start - bytes_offset) as usize..(window_end - bytes_offset) as usize];
        let (text, offsets) = self.decode_with_offsets(window, window_start);
        let Some(cut_index) = offsets.iter().position(|offset| *offset >= cut) else {
            return cut;
        };
        let byte_index = text
            .char_indices()
            .nth(cut_index)
            .map_or(text.len(), |(index, _)| index);
        match text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .find(|index| *index >= byte_index)
        {
            Some(index) => {
                let char_index = text[..index].chars().count();
                offsets.get(char_index).copied().unwrap_or(window_end)
            }
            // 整个右侧上下文都在同一个字形簇里：切在窗口尾，宁可簇太长也不越界。
            None => window_end,
        }
    }

    fn char_boundary_at_or_after(self, bytes: &[u8], bytes_offset: u64, offset: u64) -> u64 {
        let end = bytes_offset + bytes.len() as u64;
        let at = |offset: u64| bytes[(offset - bytes_offset) as usize];
        match self.char_boundaries() {
            CharBoundaries::Utf8 => {
                let mut offset = offset;
                while offset < end && at(offset) & 0xc0 == 0x80 {
                    offset += 1;
                }
                offset
            }
            CharBoundaries::Utf16 { little_endian } => {
                let mut offset = offset + (offset - self.data_start) % 2;
                if offset + 1 < end {
                    let pair = [at(offset), at(offset + 1)];
                    let unit = if little_endian {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    };
                    // 低位代理是代理对的后半，停在它前面会拆开一个字。
                    if (0xdc00..=0xdfff).contains(&unit) {
                        offset += 2;
                    }
                }
                offset.min(end)
            }
            CharBoundaries::SingleByte
            | CharBoundaries::MultiByte(_)
            | CharBoundaries::Stateful => offset,
        }
    }

    /// 解码 Unicode 编码的一段，同时给出每个字在文件里的起始偏移。
    fn decode_with_offsets(self, bytes: &[u8], bytes_offset: u64) -> (String, Vec<u64>) {
        match self.char_boundaries() {
            CharBoundaries::Utf16 { little_endian } => {
                let units = bytes.as_chunks::<2>().0.iter().map(|unit| {
                    if little_endian {
                        u16::from_le_bytes(*unit)
                    } else {
                        u16::from_be_bytes(*unit)
                    }
                });
                let mut text = String::new();
                let mut offsets = Vec::new();
                let mut offset = bytes_offset;
                for decoded in char::decode_utf16(units) {
                    let character = decoded.unwrap_or(char::REPLACEMENT_CHARACTER);
                    offsets.push(offset);
                    offset += character.len_utf16() as u64 * 2;
                    text.push(character);
                }
                (text, offsets)
            }
            _ => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                let offsets = if text.len() == bytes.len() {
                    text.char_indices()
                        .map(|(index, _)| bytes_offset + index as u64)
                        .collect()
                } else {
                    // 有坏字节时字符与字节对不上，不做字形簇调整。
                    Vec::new()
                };
                (text, offsets)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use encoding_rs::{BIG5, EUC_JP, EUC_KR, GB18030, GBK, SHIFT_JIS, UTF_16LE, UTF_8};

    use super::*;

    /// 每个目标位置的切点都不早于目标、落在字符边界上（前后两半各自解码都没有替换符）。
    fn assert_cuts_on_char_boundaries(encoding: &'static Encoding, text: &str) {
        let bytes = encoding.encode(text).0.into_owned();
        let bytes = if encoding == UTF_16LE {
            text.encode_utf16().flat_map(u16::to_le_bytes).collect()
        } else {
            bytes
        };
        let decoding = TextDecoding::new(encoding, 0);
        for target in 0..bytes.len() as u64 {
            let cut = decoding
                .segment_cut(&bytes, 0, Some(0), target)
                .expect("known boundary");
            assert!(cut >= target, "{}: cut {cut} < {target}", encoding.name());
            let (head, tail) = bytes.split_at(cut as usize);
            assert!(
                !decoding.decode(head).contains('\u{fffd}')
                    && !decoding.decode(tail).contains('\u{fffd}'),
                "{}: cut {cut} for target {target} splits a character",
                encoding.name()
            );
        }
    }

    #[test]
    fn cuts_never_split_a_character_in_any_supported_encoding() {
        let chinese = "这是一段中文, 夹着 ASCII 与标点。".repeat(4);
        assert_cuts_on_char_boundaries(UTF_8, &chinese);
        assert_cuts_on_char_boundaries(GBK, &chinese);
        assert_cuts_on_char_boundaries(UTF_16LE, "表情😀和代理对😀😀a");
        // GB18030 四字节码的第 2、4 字节是数字，不能当同步点。
        assert_cuts_on_char_boundaries(GB18030, &"𠀀1𠀁2中".repeat(8));
        assert_cuts_on_char_boundaries(BIG5, &"這是繁體中文, 測試。".repeat(4));
        assert_cuts_on_char_boundaries(SHIFT_JIS, &"これは日本語ｶﾀｶﾅです。".repeat(4));
        assert_cuts_on_char_boundaries(EUC_JP, &"日本語のテキスト。".repeat(4));
        assert_cuts_on_char_boundaries(EUC_KR, &"한국어 텍스트입니다.".repeat(4));
    }

    #[test]
    fn unicode_cuts_keep_grapheme_clusters_together() {
        let decoding = TextDecoding::new(UTF_8, 0);
        // e + 组合尖音符：切点不能落在两者之间。
        let combining = "ae\u{301}b";
        let bytes = combining.as_bytes();
        assert_eq!(decoding.segment_cut(bytes, 0, Some(0), 2), Some(4));
        // 一家人 emoji：ZWJ 连起来的整串不能拆。
        let family = "x👨\u{200d}👩\u{200d}👧y";
        let bytes = family.as_bytes();
        let family_end = family.len() as u64 - 1;
        for target in 2..family_end {
            assert_eq!(
                decoding.segment_cut(bytes, 0, Some(0), target),
                Some(family_end)
            );
        }
    }

    #[test]
    fn multi_byte_cut_does_not_depend_on_the_known_boundary() {
        let text = "中文abc中文,def中文".repeat(50);
        let mut boundaries = vec![0_u64];
        let mut bytes = Vec::new();
        for character in text.chars() {
            bytes.extend_from_slice(&GBK.encode(character.encode_utf8(&mut [0; 4])).0);
            boundaries.push(bytes.len() as u64);
        }
        let decoding = TextDecoding::new(GBK, 0);
        for target in 0..bytes.len() as u64 {
            let from_start = decoding.segment_cut(&bytes, 0, Some(0), target);
            for known in boundaries.iter().copied().filter(|known| *known <= target) {
                assert_eq!(
                    decoding.segment_cut(&bytes, 0, Some(known), target),
                    from_start
                );
            }
            // 没有已知边界时，窗口里有同步点就得到同一个结果。
            if let Some(found) = decoding.segment_cut(&bytes, 0, None, target) {
                assert_eq!(Some(found), from_start);
            }
        }
    }
}
