//! 文字文件的编码判定，搜索正文提取与文本预览共用一套规则：
//! BOM 说了算；没有 BOM 时开头有 NUL 的是二进制（扩展名写错的、被截断的数据文件）；
//! 合法 UTF-8 直接用；否则按字节分布猜编码（GBK、Big5、Shift_JIS 等）。
//! 猜出来的编码总能把字节「转」成字符，转成功不说明是文字，所以调用方解码后还要用
//! `reads_as_text` 再判一次。

pub use encoding_rs::Encoding;
use encoding_rs::UTF_8;

/// 判断是不是二进制只看开头这么多：数据文件开头就有 NUL；文字文件后面偶尔夹一个 NUL
/// 不该整篇不要。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SniffedEncoding {
    pub encoding: &'static Encoding,
    /// 开头 BOM 的字节数，正文从这里开始；没有 BOM 为 0。
    pub bom_length: usize,
}

/// 开头样本 → 编码；二进制时 None。`sample_is_whole_file` 为假表示只拿到了文件开头，
/// 结尾被截断的半个字不算 UTF-8 错误。
pub fn sniff_text_encoding(sample: &[u8], sample_is_whole_file: bool) -> Option<SniffedEncoding> {
    if let Some((encoding, bom_length)) = Encoding::for_bom(sample) {
        return Some(SniffedEncoding {
            encoding,
            bom_length,
        });
    }
    if sample[..sample.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let encoding = if is_utf8(sample, sample_is_whole_file) {
        UTF_8
    } else {
        // 前面已经确认不是合法 UTF-8；文件不会执行脚本，ISO-2022-JP 可以放行。
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
        detector.feed(sample, sample_is_whole_file);
        detector.guess(None, chardetng::Utf8Detection::Deny)
    };
    Some(SniffedEncoding {
        encoding,
        bom_length: 0,
    })
}

/// 真文字也偶尔夹几个不像文字的字符（打包的 JS、聊天记录），实测正常文件最多约 0.4%、
/// 二进制最少约 3.8%，所以超过 1% 才算二进制。
pub fn reads_as_text(text: &str) -> bool {
    let (characters, non_text) = text.chars().fold((0_usize, 0_usize), |(all, non_text), c| {
        (all + 1, non_text + usize::from(is_non_text(c)))
    });
    non_text * 100 <= characters
}

/// 响铃、退格、横竖制表、换行、换页、回车和 ESC 算文字（日志里的颜色码、终端输出的退格
/// 都是正常内容）；NUL、DEL、C1 控制符和替换符（认不出的字节）不是。
fn is_non_text(character: char) -> bool {
    matches!(
        character,
        '\0'..='\x06' | '\x0e'..='\x1a' | '\x1c'..='\x1f' | '\x7f'..='\u{9f}' | '\u{fffd}'
    )
}

fn is_utf8(bytes: &[u8], is_whole: bool) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) => !is_whole && error.error_len().is_none(),
    }
}

#[cfg(test)]
#[path = "text_encoding_tests.rs"]
mod tests;
