//! 换行在字节流里的样子随编码而变。ASCII 兼容编码（UTF-8、GBK、Big5、Shift_JIS、
//! EUC-*、单字节编码）的多字节字符尾字节都不落在 0x0A，0x0A 只会是换行；UTF-16 的换行是
//! 两字节单元，单看 0x0A 会把「上」（U+4E0A，小端 `0A 4E`）这类字符误当换行。

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineBreaks {
    SingleByte,
    Utf16Le,
    Utf16Be,
}

impl LineBreaks {
    pub(crate) fn for_encoding(encoding: &'static Encoding) -> Self {
        if encoding == UTF_16LE {
            Self::Utf16Le
        } else if encoding == UTF_16BE {
            Self::Utf16Be
        } else {
            Self::SingleByte
        }
    }

    /// `bytes` 从一个编码单元的边界开始；返回第一个换行单元之后的位置。
    pub(crate) fn next_line_end(self, bytes: &[u8]) -> Option<usize> {
        match self {
            Self::SingleByte => memchr::memchr(b'\n', bytes).map(|index| index + 1),
            Self::Utf16Le => Self::next_utf16_line_end(bytes, [b'\n', 0]),
            Self::Utf16Be => Self::next_utf16_line_end(bytes, [0, b'\n']),
        }
    }

    /// `bytes` 里完整编码单元占的长度：没找到换行时下次从这里接着找，被块边界切开的
    /// 半个 UTF-16 单元留到下一块补齐。
    pub(crate) fn complete_len(self, len: usize) -> usize {
        match self {
            Self::SingleByte => len,
            Self::Utf16Le | Self::Utf16Be => len & !1,
        }
    }

    /// `bytes` 从一个编码单元的边界开始；数其中完整的换行单元。
    pub(crate) fn count(self, mut bytes: &[u8]) -> usize {
        let mut count = 0;
        while let Some(end) = self.next_line_end(bytes) {
            count += 1;
            bytes = &bytes[end..];
        }
        count
    }

    /// `bytes` 从编码单元边界开始且只含完整单元时，最后一个单元是不是换行。
    pub(crate) fn ends_with_line_break(self, bytes: &[u8]) -> bool {
        match self {
            Self::SingleByte => bytes.ends_with(b"\n"),
            Self::Utf16Le => bytes.ends_with(&[b'\n', 0]),
            Self::Utf16Be => bytes.ends_with(&[0, b'\n']),
        }
    }

    fn next_utf16_line_end(bytes: &[u8], newline: [u8; 2]) -> Option<usize> {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .position(|unit| *unit == newline)
            .map(|index| index * 2 + 2)
    }
}

#[cfg(test)]
mod tests {
    use encoding_rs::{GBK, UTF_8};

    use super::*;

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn ascii_compatible_encodings_break_on_every_line_feed_byte() {
        let (gbk, _, _) = GBK.encode("第一行\n第二行\n");
        let breaks = LineBreaks::for_encoding(GBK);
        assert_eq!(breaks.count(&gbk), 2);
        assert_eq!(
            LineBreaks::for_encoding(UTF_8).next_line_end(b"ab\ncd"),
            Some(3)
        );
    }

    #[test]
    fn utf16_breaks_only_on_whole_newline_units() {
        // 「上」的小端编码是 0A 4E，不是换行。
        let bytes = utf16le("上\n下");
        let breaks = LineBreaks::for_encoding(UTF_16LE);
        assert_eq!(breaks.next_line_end(&bytes), Some(4));
        assert_eq!(breaks.count(&bytes), 1);

        let big_endian: Vec<u8> = "上\n".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(LineBreaks::for_encoding(UTF_16BE).count(&big_endian), 1);
    }

    #[test]
    fn half_utf16_unit_is_left_for_the_next_block() {
        let breaks = LineBreaks::for_encoding(UTF_16LE);
        assert_eq!(breaks.complete_len(5), 4);
        assert_eq!(LineBreaks::SingleByte.complete_len(5), 5);
    }
}
