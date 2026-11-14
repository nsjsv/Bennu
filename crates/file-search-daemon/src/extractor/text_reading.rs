//! 文字类文件的正文：只读前 `max_output_bytes` 字节（再长的日志、小说也能搜到开头），
//! 认出编码转成文字；带标签的（网页、XML）再去掉标签；转换后的正文同样不超过这个字节数。
//!
//! 编码判定与「像不像文字」和文本预览共用 `file_core::text_encoding`；这里只管只读了
//! 开头时的截断处理和全文索引对 NUL 的要求。

use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8};
use file_core::text_encoding::{reads_as_text, sniff_text_encoding};
use tokio::io::AsyncReadExt;

use super::markup_text::markup_text;
use super::{ExtractionOutcome, ExtractionStatus};
use crate::error::{SearchError, SearchResult};

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
    let sniffed = sniff_text_encoding(bytes, !truncated)?;
    let body = &bytes[sniffed.bom_length..];
    let text = if sniffed.encoding == UTF_8 {
        utf8_prefix(body, truncated).unwrap_or_else(|| String::from_utf8_lossy(body).into())
    } else {
        let is_utf16 = sniffed.encoding == UTF_16LE || sniffed.encoding == UTF_16BE;
        let body = if truncated && is_utf16 {
            &body[..body.len() & !1]
        } else {
            body
        };
        sniffed
            .encoding
            .decode_without_bom_handling(body)
            .0
            .into_owned()
    };
    reads_as_text(&text).then(|| without_nul(text))
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
