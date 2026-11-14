//! 打开文本预览：先判定是不是文字、用什么编码（只看开头 64 KB），再读第一个窗口。
//! 全部阻塞 I/O 放在 `spawn_blocking` 里：网络挂载上读一次可能要几十毫秒。

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use encoding_rs::Encoding;
use file_core::mime::{MimeDatabase, NameType, TEXT_PLAIN};
use file_core::text_encoding::{reads_as_text, sniff_text_encoding, SniffedEncoding};

use crate::preview::PreviewContent;
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeKind};
use crate::text_document::{open_text, ByteSource, TextDecoding, TextJump, TextOpened};
use crate::text_preview::TextPreviewFormat;

/// 判类型和编码读的开头样本：盖住类型库最长的特征范围，也给编码检测足够的字节。
const TEXT_PREVIEW_SAMPLE_BYTES: u64 = 64 * 1024;
const TEXT_MARKDOWN: &str = "text/markdown";

/// 打开时判定的结论：按什么编码、从哪个字节开始解码、按什么格式显示。
struct TextFileInspection {
    encoding: SniffedEncoding,
    format: TextPreviewFormat,
}

/// 编码怎么定：打开时识别，或用户手选（只对这次预览有效，文件改写重新打开时保留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncodingChoice {
    Detect,
    Chosen(&'static Encoding),
}

pub async fn load_initial_text_preview(
    path: PathBuf,
    rules: &PreviewExtensionRules,
) -> Result<PreviewContent, String> {
    let treat_as_text = rules.matches(PreviewFileSizeKind::Text, &path);
    blocking(move || {
        let file = open_file(&path)?;
        let inspection = inspect_text_file(&path, &file, treat_as_text)?;
        let decoding = TextDecoding::new(
            inspection.encoding.encoding,
            inspection.encoding.bom_length as u64,
        );
        let opened = open_text(file, decoding, false, inspection.format, TextJump::Start)
            .map_err(read_error)?;
        Ok(PreviewContent::Text {
            path,
            opened: Arc::new(opened),
        })
    })
    .await
}

/// 重新打开同一路径（文件被改写、用户改了编码）：已经当文本打开过，不再判类型；
/// 编码按 `choice` 定，窗口读在 `jump` 处。
pub async fn reopen_text_preview(
    path: PathBuf,
    choice: TextEncodingChoice,
    format: TextPreviewFormat,
    jump: TextJump,
) -> Result<TextOpened, String> {
    blocking(move || {
        let file = open_file(&path)?;
        let (sample, sample_is_whole_file) = read_text_sample(&file)?;
        let (encoding, chosen) = match choice {
            TextEncodingChoice::Detect => (detect_encoding(&sample, sample_is_whole_file)?, false),
            // 开头的 BOM 与所选编码一致时跳过它，否则 BOM 也是用户要看的字节。
            TextEncodingChoice::Chosen(encoding) => {
                let bom_length = Encoding::for_bom(&sample)
                    .filter(|(bom_encoding, _)| *bom_encoding == encoding)
                    .map_or(0, |(_, bom_length)| bom_length);
                (
                    SniffedEncoding {
                        encoding,
                        bom_length,
                    },
                    true,
                )
            }
        };
        let decoding = TextDecoding::new(encoding.encoding, encoding.bom_length as u64);
        open_text(file, decoding, chosen, format, jump).map_err(read_error)
    })
    .await
}

/// 分类门禁只看了名字；这里看开头内容定论。类型库给出确定的非文本类型（MPEG-TS 的
/// `.ts`、ELF）就不预览，除非用户把后缀加进了额外文本后缀；类型库认不出（无特征、
/// 无候选、开头不像 ASCII 文字，如无后缀的带 BOM UTF-16）交给编码判定。任何情况下
/// 开头有 NUL 或解出来不像文字都是二进制——额外后缀是想看文字，不是想看乱码。
fn inspect_text_file(
    path: &Path,
    file: &File,
    treat_as_text: bool,
) -> Result<TextFileInspection, String> {
    let (sample, sample_is_whole_file) = read_text_sample(file)?;
    let types = MimeDatabase::system();
    let name_type = path
        .file_name()
        .map_or(NameType::Unknown, |name| types.type_from_name(name));
    let content_type = types.type_from_content(&name_type, &sample);
    let is_typed_as_other = content_type
        .as_deref()
        .is_some_and(|content_type| !types.is_subclass(content_type, TEXT_PLAIN));
    if is_typed_as_other && !treat_as_text {
        return Err(not_text());
    }
    let encoding = detect_encoding(&sample, sample_is_whole_file)?;
    let format = match content_type {
        Some(content_type) if types.is_subclass(&content_type, TEXT_MARKDOWN) => {
            TextPreviewFormat::Markdown
        }
        _ => TextPreviewFormat::Plain,
    };
    Ok(TextFileInspection { encoding, format })
}

fn detect_encoding(sample: &[u8], sample_is_whole_file: bool) -> Result<SniffedEncoding, String> {
    let encoding = sniff_text_encoding(sample, sample_is_whole_file).ok_or_else(not_text)?;
    let decoded = encoding
        .encoding
        .decode_without_bom_handling(&sample[encoding.bom_length..])
        .0;
    if !reads_as_text(&decoded) {
        return Err(not_text());
    }
    Ok(encoding)
}

/// 读开头样本；第二项表示样本就是整个文件。
fn read_text_sample(file: &File) -> Result<(Vec<u8>, bool), String> {
    let mut sample = vec![0; TEXT_PREVIEW_SAMPLE_BYTES as usize + 1];
    let read = crate::text_document::read_full(file as &dyn ByteSource, 0, &mut sample)
        .map_err(read_error)?;
    sample.truncate(read);
    let sample_is_whole_file = sample.len() as u64 <= TEXT_PREVIEW_SAMPLE_BYTES;
    sample.truncate(TEXT_PREVIEW_SAMPLE_BYTES as usize);
    Ok((sample, sample_is_whole_file))
}

fn open_file(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|error| format!("could not open text preview: {error}"))
}

fn read_error(error: std::io::Error) -> String {
    format!("could not read text preview: {error}")
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| format!("could not read text preview: {error}"))?
}

fn not_text() -> String {
    bennu_localization::tr!("This file contains binary data and can't be previewed as text.")
}

#[cfg(test)]
#[path = "text_preview_loading_tests.rs"]
mod tests;
