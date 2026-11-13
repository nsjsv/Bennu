//! 文字类文件的正文：只读前 `max_output_bytes` 字节（再长的日志、小说也能搜到开头），
//! 认出编码转成文字；带标签的（网页、XML）再去掉标签；转换后的正文同样不超过这个字节数。
//!
//! 编码：BOM 说了算；没有 BOM 时开头有 NUL 的是二进制（扩展名写错的、被截断的数据文件）；
//! 合法 UTF-8 直接用；否则按字节分布猜编码（GBK、Big5、Shift_JIS 等）再转换。
//! 转出来不像文字的字符太多的也是二进制（开头没有 NUL 的游戏资源、数据库文件）。

use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use encoding_rs::{Encoding, UTF_8};
use tokio::io::AsyncReadExt;

use super::markup_text::markup_text;
use super::{ExtractionOutcome, ExtractionStatus};
use crate::error::{SearchError, SearchResult};

/// 判断是不是二进制只看开头这么多：数据文件开头就有 NUL；文字文件后面偶尔夹一个 NUL
/// 不该整篇不要（换成空格）。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextForm {
    Plain,
    /// 网页与 XML：只要标签之间的文字。
    Markup,
}

pub(super) async fn extract_text(
    path: &Path,
    max_output_bytes: u64,
    form: TextForm,
) -> SearchResult<ExtractionOutcome> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(source) => return classify_text_read_error(path, source),
    };
    let mut bytes = Vec::new();
    let mut limited_file = file.take(max_output_bytes.saturating_add(1));
    let read_outcome = limited_file.read_to_end(&mut bytes).await;
    release_file_pages(limited_file.get_ref());
    if let Err(source) = read_outcome {
        return classify_text_read_error(path, source);
    }
    let truncated = bytes.len() as u64 > max_output_bytes;
    if truncated {
        bytes.truncate(max_output_bytes as usize);
    }
    let Some(text) = decode_text(&bytes, truncated) else {
        return Ok(ExtractionOutcome::skipped(ExtractionStatus::Unsupported));
    };
    let mut text = match form {
        TextForm::Plain => text,
        TextForm::Markup => markup_text(&text),
    };
    // 转成 UTF-8 会变长（GBK 汉字 2 → 3 字节，单字节编码的高位字节 1 → 2～3 字节）；
    // 上限必须按存进索引的字节算，否则一个文件就能撑破写入线程的单批预算。
    text.truncate(text.floor_char_boundary(max_output_bytes as usize));
    Ok(ExtractionOutcome::text(text))
}

/// 字节转文字；是二进制时 None。`truncated` 表示只读了文件开头，结尾半个字要丢掉。
pub(super) fn decode_text(bytes: &[u8], truncated: bool) -> Option<String> {
    let text = if let Some((encoding, bom_length)) = Encoding::for_bom(bytes) {
        let body = &bytes[bom_length..];
        let body = if truncated && encoding != UTF_8 {
            &body[..body.len() & !1]
        } else {
            body
        };
        if encoding == UTF_8 {
            utf8_prefix(body, truncated).unwrap_or_else(|| String::from_utf8_lossy(body).into())
        } else {
            encoding.decode_without_bom_handling(body).0.into_owned()
        }
    } else {
        let sample = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
        if sample.contains(&0) {
            return None;
        }
        utf8_prefix(bytes, truncated).unwrap_or_else(|| {
            // 前面已经确认不是合法 UTF-8；文件不会执行脚本，ISO-2022-JP 可以放行。
            let mut detector =
                chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
            detector.feed(bytes, !truncated);
            let encoding = detector.guess(None, chardetng::Utf8Detection::Deny);
            encoding.decode_without_bom_handling(bytes).0.into_owned()
        })
    };
    reads_as_text(&text).then(|| without_nul(text))
}

/// 猜编码总能把字节「转」成字符（单字节编码里每个字节都对应一个字），转成功不说明是文字；
/// 二进制转出来夹满控制符和认不出的字节。真文字也偶尔夹几个（打包的 JS、聊天记录），
/// 实测正常文件最多约 0.4%、二进制最少约 3.8%，所以超过 1% 才算二进制。
fn reads_as_text(text: &str) -> bool {
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

/// 合法 UTF-8（只读了开头时，结尾被截断的半个字不算错）。
fn utf8_prefix(bytes: &[u8], truncated: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        Err(error) if truncated && error.error_len().is_none() => Some(
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .ok()?
                .to_owned(),
        ),
        Err(_) => None,
    }
}

/// 全文索引按 C 字符串处理文字，中间的 NUL 会截断；换成空格。
fn without_nul(text: String) -> String {
    if text.contains('\0') {
        text.replace('\0', " ")
    } else {
        text
    }
}

fn release_file_pages(file: &tokio::fs::File) {
    // 正文已经复制进有界 Vec；保留源文件干净页只会抬高 service cgroup 的空闲工作集。
    let _ = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

pub(super) fn classify_text_read_error(
    path: &Path,
    source: io::Error,
) -> SearchResult<ExtractionOutcome> {
    if source.kind() == io::ErrorKind::PermissionDenied {
        Err(SearchError::Inaccessible {
            path: path.to_path_buf(),
            source,
        })
    } else {
        Ok(ExtractionOutcome::skipped(ExtractionStatus::ReadFailed {
            message: source.to_string(),
        }))
    }
}

#[cfg(test)]
#[path = "text_reading_tests.rs"]
mod tests;
