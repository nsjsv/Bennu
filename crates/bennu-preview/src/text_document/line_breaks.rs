//! 换行在字节流里的样子随编码而变。ASCII 兼容编码（UTF-8、GBK、Big5、Shift_JIS、
//! EUC-*、单字节编码）的多字节字符尾字节都不落在 0x0A，0x0A 只会是换行；UTF-16 的换行是
//! 两字节单元，单看 0x0A 会把「上」（U+4E0A，小端 `0A 4E`）这类字符误当换行。
//! 所有方法的 `bytes` 都从一个编码单元的边界开始。

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

    pub(crate) fn unit_len(self) -> usize {
        match self {
            Self::SingleByte => 1,
            Self::Utf16Le | Self::Utf16Be => 2,
        }
    }

    /// 第一个换行单元之后的位置。
    pub(crate) fn next_line_end(self, bytes: &[u8]) -> Option<usize> {
        match self {
            Self::SingleByte => memchr::memchr(b'\n', bytes).map(|index| index + 1),
            Self::Utf16Le | Self::Utf16Be => self
                .units(bytes)
                .iter()
                .position(|unit| *unit == self.newline())
                .map(|index| index * 2 + 2),
        }
    }

    /// 最后一个换行单元之后的位置。
    pub(crate) fn last_line_end(self, bytes: &[u8]) -> Option<usize> {
        match self {
            Self::SingleByte => memchr::memrchr(b'\n', bytes).map(|index| index + 1),
            Self::Utf16Le | Self::Utf16Be => self
                .units(bytes)
                .iter()
                .rposition(|unit| *unit == self.newline())
                .map(|index| index * 2 + 2),
        }
    }

    /// 完整换行单元的个数。
    pub(crate) fn count(self, bytes: &[u8]) -> u64 {
        match self {
            Self::SingleByte => memchr::memchr_iter(b'\n', bytes).count() as u64,
            Self::Utf16Le | Self::Utf16Be => self
                .units(bytes)
                .iter()
                .filter(|unit| **unit == self.newline())
                .count() as u64,
        }
    }

    /// `bytes` 里完整编码单元占的长度：被块边界切开的半个 UTF-16 单元留到下一块。
    pub(crate) fn complete_len(self, len: usize) -> usize {
        len - len % self.unit_len()
    }

    /// 以换行单元结尾的一行末尾，换行（连同紧挨着它的回车）占几个字节；不以换行结尾为 0。
    /// 单独的回车不算换行（终端进度条输出里很常见），只在换行前面才一起去掉。
    pub(crate) fn terminator_len(self, line: &[u8]) -> usize {
        let unit = self.unit_len();
        let complete = &line[..self.complete_len(line.len())];
        if complete.len() < unit || !self.is_unit(&complete[complete.len() - unit..], b'\n') {
            return 0;
        }
        let before_newline = &complete[..complete.len() - unit];
        if before_newline.len() >= unit
            && self.is_unit(&before_newline[before_newline.len() - unit..], b'\r')
        {
            unit * 2
        } else {
            unit
        }
    }

    fn newline(self) -> [u8; 2] {
        self.unit_bytes(b'\n')
    }

    fn unit_bytes(self, ascii: u8) -> [u8; 2] {
        match self {
            Self::Utf16Be => [0, ascii],
            Self::SingleByte | Self::Utf16Le => [ascii, 0],
        }
    }

    fn is_unit(self, unit: &[u8], ascii: u8) -> bool {
        match self {
            Self::SingleByte => unit == [ascii],
            Self::Utf16Le | Self::Utf16Be => unit == self.unit_bytes(ascii),
        }
    }

    fn units(self, bytes: &[u8]) -> &[[u8; 2]] {
        bytes.as_chunks::<2>().0
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
        assert_eq!(LineBreaks::SingleByte.last_line_end(b"a\nb\nc"), Some(4));
    }

    #[test]
    fn utf16_breaks_only_on_whole_newline_units() {
        // 「上」的小端编码是 0A 4E，不是换行。
        let bytes = utf16le("上\n下\n");
        let breaks = LineBreaks::for_encoding(UTF_16LE);
        assert_eq!(breaks.next_line_end(&bytes), Some(4));
        assert_eq!(breaks.last_line_end(&bytes), Some(8));
        assert_eq!(breaks.count(&bytes), 2);

        let big_endian: Vec<u8> = "上\n".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(LineBreaks::for_encoding(UTF_16BE).count(&big_endian), 1);
    }

    #[test]
    fn half_utf16_unit_is_left_for_the_next_block() {
        assert_eq!(LineBreaks::Utf16Le.complete_len(5), 4);
        assert_eq!(LineBreaks::SingleByte.complete_len(5), 5);
    }

    #[test]
    fn terminator_covers_a_carriage_return_only_before_a_newline() {
        let breaks = LineBreaks::SingleByte;
        assert_eq!(breaks.terminator_len(b"ab\r\n"), 2);
        assert_eq!(breaks.terminator_len(b"ab\n"), 1);
        assert_eq!(breaks.terminator_len(b"ab\r"), 0);
        assert_eq!(breaks.terminator_len(b"ab"), 0);
        assert_eq!(LineBreaks::Utf16Le.terminator_len(&utf16le("ab\r\n")), 4);
        assert_eq!(LineBreaks::Utf16Le.terminator_len(&utf16le("ab\n")), 2);
    }
}
