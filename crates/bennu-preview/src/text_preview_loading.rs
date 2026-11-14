use std::path::{Path, PathBuf};
use std::sync::Arc;

use encoding_rs::Encoding;
use file_core::mime::{MimeDatabase, NameType, TEXT_PLAIN};
use file_core::text_encoding::{reads_as_text, sniff_text_encoding, SniffedEncoding};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::preview::PreviewContent;
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeKind};
use crate::text_line_breaks::LineBreaks;
use crate::text_preview::{
    text_preview_loaded_line_count, TextPreviewChunk, TextPreviewChunkRequest, TextPreviewFormat,
    TextPreviewLineLimitNotice, TEXT_PREVIEW_INITIAL_LINE_LIMIT, TEXT_PREVIEW_LINE_LIMIT,
};

pub const PREVIEW_TEXT_LIMIT: usize = 4 * 1024 * 1024;

const TEXT_PREVIEW_READ_BLOCK_SIZE: usize = 8 * 1024;
/// 判类型和编码读的开头样本：盖住类型库最长的特征范围，也给编码检测足够的字节。
const TEXT_PREVIEW_SAMPLE_BYTES: u64 = 64 * 1024;
const TEXT_MARKDOWN: &str = "text/markdown";

/// 打开时判定的结论：按什么编码、从哪个字节开始解码、按什么格式显示。
struct TextFileInspection {
    encoding: SniffedEncoding,
    format: TextPreviewFormat,
}

pub async fn load_initial_text_preview(
    path: PathBuf,
    rules: &PreviewExtensionRules,
) -> Result<PreviewContent, String> {
    let inspection = inspect_text_file(&path, rules).await?;
    load_text_preview_from(path, inspection).await
}

/// 用户指定编码重读：已经当文本打开过，不再做类型与二进制判定。开头的 BOM 与所选编码
/// 一致时跳过它，否则从第 0 字节按所选编码解码（BOM 也是用户要看的字节）。
pub async fn load_reencoded_text_preview(
    path: PathBuf,
    encoding: &'static Encoding,
    format: TextPreviewFormat,
) -> Result<PreviewContent, String> {
    let (head, _) = read_text_sample(&path).await?;
    let bom_length = Encoding::for_bom(&head)
        .filter(|(bom_encoding, _)| *bom_encoding == encoding)
        .map_or(0, |(_, bom_length)| bom_length);
    let inspection = TextFileInspection {
        encoding: SniffedEncoding {
            encoding,
            bom_length,
        },
        format,
    };
    load_text_preview_from(path, inspection).await
}

async fn load_text_preview_from(
    path: PathBuf,
    inspection: TextFileInspection,
) -> Result<PreviewContent, String> {
    let encoding = inspection.encoding.encoding;
    let text_preview = read_text_preview_chunk(
        path.as_path(),
        inspection.encoding.bom_length as u64,
        TEXT_PREVIEW_INITIAL_LINE_LIMIT,
        0,
        encoding,
    )
    .await?;

    Ok(PreviewContent::Text {
        path,
        rendered: Arc::from(text_preview.content),
        format: inspection.format,
        encoding,
        next_offset: text_preview.next_offset,
        loaded_line_count: text_preview.line_count,
        line_limit_notice: text_preview.line_limit_notice,
    })
}

/// 分类门禁只看了名字；这里看开头内容定论。类型库给出确定的非文本类型（MPEG-TS 的
/// `.ts`、ELF）就不预览，除非用户把后缀加进了额外文本后缀；类型库认不出（无特征、
/// 无候选、开头不像 ASCII 文字，如无后缀的带 BOM UTF-16）交给编码判定。任何情况下
/// 开头有 NUL 或解出来不像文字都是二进制——额外后缀是想看文字，不是想看乱码。
async fn inspect_text_file(
    path: &Path,
    rules: &PreviewExtensionRules,
) -> Result<TextFileInspection, String> {
    let (sample, sample_is_whole_file) = read_text_sample(path).await?;
    let types = MimeDatabase::system();
    let name_type = path
        .file_name()
        .map_or(NameType::Unknown, |name| types.type_from_name(name));
    let content_type = types.type_from_content(&name_type, &sample);
    let is_typed_as_other = content_type
        .as_deref()
        .is_some_and(|content_type| !types.is_subclass(content_type, TEXT_PLAIN));
    if is_typed_as_other && !rules.matches(PreviewFileSizeKind::Text, path) {
        return Err(not_text());
    }
    let encoding = sniff_text_encoding(&sample, sample_is_whole_file).ok_or_else(not_text)?;
    let decoded = encoding
        .encoding
        .decode_without_bom_handling(&sample[encoding.bom_length..])
        .0;
    if !reads_as_text(&decoded) {
        return Err(not_text());
    }
    let format = match content_type {
        Some(content_type) if types.is_subclass(&content_type, TEXT_MARKDOWN) => {
            TextPreviewFormat::Markdown
        }
        _ => TextPreviewFormat::Plain,
    };
    Ok(TextFileInspection { encoding, format })
}

/// 读开头样本；第二项表示样本就是整个文件。
async fn read_text_sample(path: &Path) -> Result<(Vec<u8>, bool), String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("could not open text preview: {error}"))?;
    let mut sample = Vec::new();
    file.take(TEXT_PREVIEW_SAMPLE_BYTES + 1)
        .read_to_end(&mut sample)
        .await
        .map_err(|error| format!("could not read text preview: {error}"))?;
    let sample_is_whole_file = sample.len() as u64 <= TEXT_PREVIEW_SAMPLE_BYTES;
    sample.truncate(TEXT_PREVIEW_SAMPLE_BYTES as usize);
    Ok((sample, sample_is_whole_file))
}

pub async fn load_text_preview_chunk(
    request: TextPreviewChunkRequest,
) -> Result<TextPreviewChunk, String> {
    let text_preview = read_text_preview_chunk(
        request.path.as_path(),
        request.start_offset,
        request.line_limit,
        request.loaded_line_count,
        request.encoding,
    )
    .await?;

    Ok(TextPreviewChunk {
        start_offset: request.start_offset,
        content: text_preview.content,
        line_count: text_preview.line_count,
        next_offset: text_preview.next_offset,
        line_limit_notice: text_preview.line_limit_notice,
    })
}

struct TextPreviewReadChunk {
    content: String,
    line_count: usize,
    next_offset: Option<u64>,
    line_limit_notice: Option<TextPreviewLineLimitNotice>,
}

/// `start_offset` 总落在换行单元之后（或 BOM 之后），是字符边界，块与块之间不会切开字。
async fn read_text_preview_chunk(
    path: &Path,
    start_offset: u64,
    line_limit: usize,
    loaded_line_count: usize,
    encoding: &'static Encoding,
) -> Result<TextPreviewReadChunk, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("could not open text preview: {error}"))?;
    let file_len = file
        .metadata()
        .await
        .map_err(|error| format!("could not inspect text preview: {error}"))?
        .len();
    file.seek(std::io::SeekFrom::Start(start_offset))
        .await
        .map_err(|error| format!("could not seek text preview: {error}"))?;

    let breaks = LineBreaks::for_encoding(encoding);
    let scan_after_line_limit =
        loaded_line_count.saturating_add(line_limit) >= TEXT_PREVIEW_LINE_LIMIT;
    let mut buffer = Vec::new();
    let mut block = vec![0; TEXT_PREVIEW_READ_BLOCK_SIZE];
    let mut line_tally = LineTally::new(breaks);
    // buffer 里已经找过换行的长度与找到的换行数；第 line_limit 个换行之后就是本块结尾。
    let mut scanned = 0;
    let mut line_end_count = 0;
    let mut cut = None;
    let mut reached_eof = false;

    loop {
        let read_count = file
            .read(&mut block)
            .await
            .map_err(|error| format!("could not read text preview: {error}"))?;
        if read_count == 0 {
            reached_eof = true;
            break;
        }

        let bytes = &block[..read_count];
        line_tally.observe(bytes);

        if buffer.len() >= PREVIEW_TEXT_LIMIT || cut.is_some() {
            if scan_after_line_limit {
                continue;
            }
            break;
        }

        let stored_len = bytes.len().min(PREVIEW_TEXT_LIMIT - buffer.len());
        buffer.extend_from_slice(&bytes[..stored_len]);
        while cut.is_none() {
            let Some(line_end) = breaks.next_line_end(&buffer[scanned..]) else {
                scanned = breaks.complete_len(buffer.len());
                break;
            };
            scanned += line_end;
            line_end_count += 1;
            if line_end_count == line_limit {
                cut = Some(scanned);
            }
        }

        if cut.is_some() && !scan_after_line_limit {
            break;
        }
    }

    let next_offset = cut
        .map(|cut| start_offset.saturating_add(cut as u64))
        .filter(|next_offset| *next_offset != file_len);
    let content = match cut {
        Some(cut) => {
            let content = decode_text_preview_bytes(&buffer[..cut], encoding, false);
            let content = content.strip_suffix('\n').unwrap_or(&content);
            content.strip_suffix('\r').unwrap_or(content).to_owned()
        }
        // 4 MB 上限可能落在一个字中间：非结尾解码丢掉被切开的半个字。
        None => decode_text_preview_bytes(&buffer, encoding, reached_eof),
    };
    let line_count = text_preview_loaded_line_count(&content);
    let line_limit_notice = if scan_after_line_limit && next_offset.is_some() {
        TextPreviewLineLimitNotice::for_total_line_count(
            loaded_line_count.saturating_add(line_tally.total_lines()),
        )
    } else {
        None
    };
    Ok(TextPreviewReadChunk {
        content,
        line_count,
        next_offset,
        line_limit_notice,
    })
}

/// `is_last` 为假时结尾不完整的字留在解码器里不输出（而不是变成替换符）。
fn decode_text_preview_bytes(bytes: &[u8], encoding: &'static Encoding, is_last: bool) -> String {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = decoder
        .max_utf8_buffer_length(bytes.len())
        .unwrap_or(bytes.len());
    let mut text = String::with_capacity(capacity);
    let _ = decoder.decode_to_string(bytes, &mut text, is_last);
    text
}

/// 从 `start_offset` 到文件末尾的总行数（按编码认换行）；被块边界切开的半个 UTF-16
/// 单元留到下一块补齐。
struct LineTally {
    breaks: LineBreaks,
    line_ends: usize,
    pending: Vec<u8>,
    saw_bytes: bool,
    ends_with_line_break: bool,
}

impl LineTally {
    fn new(breaks: LineBreaks) -> Self {
        Self {
            breaks,
            line_ends: 0,
            pending: Vec::new(),
            saw_bytes: false,
            ends_with_line_break: false,
        }
    }

    fn observe(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.saw_bytes = true;
        let mut joined = std::mem::take(&mut self.pending);
        joined.extend_from_slice(bytes);
        let complete = self.breaks.complete_len(joined.len());
        self.line_ends += self.breaks.count(&joined[..complete]);
        if complete > 0 {
            self.ends_with_line_break = self.breaks.ends_with_line_break(&joined[..complete]);
        }
        self.pending = joined[complete..].to_vec();
    }

    fn total_lines(&self) -> usize {
        match (self.saw_bytes, self.ends_with_line_break) {
            (false, _) => 0,
            (true, true) => self.line_ends,
            (true, false) => self.line_ends + 1,
        }
    }
}

fn not_text() -> String {
    bennu_localization::tr!("This file contains binary data and can't be previewed as text.")
}

#[cfg(test)]
mod tests {
    use std::io::{BufWriter, Write};

    use tempfile::tempdir;

    use super::*;
    use crate::text_preview::{TextPreviewFormat, TEXT_PREVIEW_CHUNK_LINE_LIMIT};

    #[tokio::test]
    async fn initial_plain_text_preview_bounds_large_no_extension_file() {
        const POST_SCRIPT_LINE_PADDING: &str =
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

        let temp_dir = tempdir().expect("temp dir");
        let text_path = temp_dir.path().join("--help");
        let file = std::fs::File::create(&text_path).expect("create text file");
        let mut writer = BufWriter::new(file);
        writeln!(writer, "%!PS-Adobe-3.0").expect("write header");
        for index in 1..290_000 {
            writeln!(writer, "%%Page: {index:06} {POST_SCRIPT_LINE_PADDING}").expect("write line");
        }
        writer.flush().expect("flush text file");
        assert!(std::fs::metadata(&text_path).expect("metadata").len() > 20 * 1024 * 1024);

        let PreviewContent::Text {
            rendered,
            format,
            next_offset,
            loaded_line_count,
            ..
        } = load_initial_text_preview(text_path.clone(), &PreviewExtensionRules::default_rules())
            .await
            .expect("text preview")
        else {
            panic!("expected text preview");
        };

        assert_eq!(format, TextPreviewFormat::Plain);
        assert_eq!(loaded_line_count, TEXT_PREVIEW_INITIAL_LINE_LIMIT);
        assert!(rendered.len() < 8 * 1024);
        assert!(!rendered.contains("%%Page: 289999"));

        let request = TextPreviewChunkRequest {
            path: text_path,
            generation: 1,
            encoding: encoding_rs::UTF_8,
            start_offset: next_offset.expect("next offset"),
            loaded_line_count,
            line_limit: TEXT_PREVIEW_CHUNK_LINE_LIMIT,
        };
        let chunk = load_text_preview_chunk(request).await.expect("next chunk");

        assert!(chunk.content.starts_with("%%Page: 000050"));
        assert_eq!(chunk.line_count, TEXT_PREVIEW_CHUNK_LINE_LIMIT);
        assert!(chunk.next_offset.is_some());
    }

    #[tokio::test]
    async fn initial_markdown_preview_clears_next_offset_at_exact_eof_boundary() {
        let temp_dir = tempdir().expect("temp dir");
        let text_path = temp_dir.path().join("fifty-lines.md");
        std::fs::write(&text_path, numbered_line_range(0, 50)).expect("write text file");

        let PreviewContent::Text {
            rendered,
            next_offset,
            loaded_line_count,
            ..
        } = load_initial_text_preview(text_path, &PreviewExtensionRules::default_rules())
            .await
            .expect("text preview")
        else {
            panic!("expected text preview");
        };

        assert!(rendered.contains("line 49"));
        assert_eq!(loaded_line_count, TEXT_PREVIEW_INITIAL_LINE_LIMIT);
        assert_eq!(next_offset, None);
    }

    #[tokio::test]
    async fn chunk_text_preview_continues_from_previous_offset() {
        let temp_dir = tempdir().expect("temp dir");
        let text_path = temp_dir.path().join("large.md");
        std::fs::write(&text_path, numbered_line_range(0, 700)).expect("write text file");

        let PreviewContent::Text { next_offset, .. } =
            load_initial_text_preview(text_path.clone(), &PreviewExtensionRules::default_rules())
                .await
                .expect("text preview")
        else {
            panic!("expected text preview");
        };
        let request = TextPreviewChunkRequest {
            path: text_path,
            generation: 1,
            encoding: encoding_rs::UTF_8,
            start_offset: next_offset.expect("next offset"),
            loaded_line_count: TEXT_PREVIEW_INITIAL_LINE_LIMIT,
            line_limit: TEXT_PREVIEW_CHUNK_LINE_LIMIT,
        };

        let chunk = load_text_preview_chunk(request).await.expect("chunk");

        assert!(chunk.content.starts_with("line 50"));
        assert!(chunk.content.contains("line 549"));
        assert!(!chunk.content.contains("line 550"));
        assert_eq!(chunk.line_count, TEXT_PREVIEW_CHUNK_LINE_LIMIT);
        assert!(chunk.next_offset.is_some());
    }

    #[tokio::test]
    async fn last_chunk_scans_total_line_count_after_display_limit() {
        let temp_dir = tempdir().expect("temp dir");
        let text_path = temp_dir.path().join("huge.txt");
        std::fs::write(&text_path, numbered_line_range(0, 10_005)).expect("write text file");

        let start_offset = byte_offset_after_lines(&numbered_line_range(0, 9_500), 9_500);
        let request = TextPreviewChunkRequest {
            path: text_path,
            generation: 1,
            encoding: encoding_rs::UTF_8,
            start_offset,
            loaded_line_count: 9_500,
            line_limit: 500,
        };

        let chunk = load_text_preview_chunk(request).await.expect("chunk");

        assert!(chunk.content.starts_with("line 9500"));
        assert!(chunk.content.contains("line 9999"));
        assert!(!chunk.content.contains("line 10000"));
        assert_eq!(
            chunk.line_limit_notice,
            TextPreviewLineLimitNotice::for_total_line_count(10_005)
        );
    }

    #[tokio::test]
    async fn chunk_truncates_crlf_without_trailing_carriage_return() {
        let temp_dir = tempdir().expect("temp dir");
        let text_path = temp_dir.path().join("crlf.md");
        let content = numbered_crlf_line_range(0, 52);
        std::fs::write(&text_path, content).expect("write text file");

        let PreviewContent::Text { rendered, .. } =
            load_initial_text_preview(text_path, &PreviewExtensionRules::default_rules())
                .await
                .expect("text preview")
        else {
            panic!("expected text preview");
        };

        assert!(rendered.ends_with("line 49"));
        assert!(!rendered.ends_with('\r'));
    }

    async fn loaded_text(
        path: PathBuf,
        rules: &PreviewExtensionRules,
    ) -> Result<(String, TextPreviewFormat, &'static Encoding), String> {
        match load_initial_text_preview(path, rules).await? {
            PreviewContent::Text {
                rendered,
                format,
                encoding,
                ..
            } => Ok((rendered.to_string(), format, encoding)),
            _ => panic!("expected text preview"),
        }
    }

    fn utf16le_with_bom(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[tokio::test]
    async fn legacy_encodings_and_utf16_are_decoded() {
        let temp_dir = tempdir().expect("temp dir");
        let rules = PreviewExtensionRules::default_rules();
        let cases = [
            (
                encoding_rs::GBK,
                "这是一个用简体中文写的季度工作报告，内容包括销售数据和市场分析。",
            ),
            (
                encoding_rs::BIG5,
                "這是一份用繁體中文撰寫的年度報告，包含財務資料與市場分析。",
            ),
            (
                encoding_rs::SHIFT_JIS,
                "これは日本語で書かれた四半期の業務報告書です。売上データと市場分析を含みます。",
            ),
        ];
        for (encoding, text) in cases {
            let path = temp_dir.path().join(format!("{}.txt", encoding.name()));
            std::fs::write(&path, encoding.encode(text).0).expect("write text file");
            let (rendered, _, detected) = loaded_text(path, &rules).await.expect("text preview");
            assert_eq!(detected, encoding);
            assert_eq!(rendered, text);
        }

        // 带 BOM 的 UTF-16，有后缀和没后缀（类型库认不出）都当文本。
        for name in ["notepad.txt", "notepad"] {
            let path = temp_dir.path().join(name);
            std::fs::write(&path, utf16le_with_bom("记事本\r\n第二行")).expect("write text file");
            let (rendered, _, detected) = loaded_text(path, &rules).await.expect(name);
            assert_eq!(detected, encoding_rs::UTF_16LE);
            assert_eq!(rendered, "记事本\r\n第二行");
        }
    }

    #[tokio::test]
    async fn binary_content_is_rejected_whatever_the_name() {
        let temp_dir = tempdir().expect("temp dir");
        let rules = PreviewExtensionRules::default_rules();
        let mut binary = b"\x7fELF\x02\x01\x01".to_vec();
        binary.extend((0..4096_u32).map(|value| (value % 251) as u8));
        for name in ["looks-like.txt", "no-extension"] {
            let path = temp_dir.path().join(name);
            std::fs::write(&path, &binary).expect("write binary file");
            assert_eq!(
                loaded_text(path, &rules).await.err(),
                Some(not_text()),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn typescript_is_text_but_mpeg_ts_is_not() {
        let temp_dir = tempdir().expect("temp dir");
        let rules = PreviewExtensionRules::default_rules();
        let source = temp_dir.path().join("index.ts");
        std::fs::write(&source, "const answer: number = 42;\nexport { answer };\n")
            .expect("write typescript");
        assert!(loaded_text(source, &rules).await.is_ok());

        let mut transport_stream = Vec::new();
        for _ in 0..5 {
            let mut packet = vec![0xff; 188];
            packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
            transport_stream.extend(packet);
        }
        let video = temp_dir.path().join("clip.ts");
        std::fs::write(&video, transport_stream).expect("write transport stream");
        assert_eq!(loaded_text(video, &rules).await.err(), Some(not_text()));
    }

    #[tokio::test]
    async fn markdown_is_decided_by_type() {
        let temp_dir = tempdir().expect("temp dir");
        let rules = PreviewExtensionRules::default_rules();
        let markdown = temp_dir.path().join("README.md");
        std::fs::write(&markdown, "# Title\n").expect("write markdown");
        let plain = temp_dir.path().join("README");
        std::fs::write(&plain, "# Title\n").expect("write readme");

        assert_eq!(
            loaded_text(markdown, &rules).await.expect("md").1,
            TextPreviewFormat::Markdown
        );
        assert_eq!(
            loaded_text(plain, &rules).await.expect("readme").1,
            TextPreviewFormat::Plain
        );
    }

    #[tokio::test]
    async fn extra_text_extension_overrides_a_non_text_name() {
        let temp_dir = tempdir().expect("temp dir");
        let path = temp_dir.path().join("notes.so");
        std::fs::write(&path, "plain words\n").expect("write text file");

        let rules = PreviewExtensionRules::default_rules();
        assert_eq!(
            loaded_text(path.clone(), &rules).await.err(),
            Some(not_text())
        );
        let mut rules = PreviewExtensionRules::default_rules();
        rules.text.push("so".to_owned());
        assert!(loaded_text(path, &rules).await.is_ok());
    }

    #[tokio::test]
    async fn utf16_chunks_continue_on_whole_line_units() {
        let temp_dir = tempdir().expect("temp dir");
        let path = temp_dir.path().join("lines.txt");
        // 「上」的小端编码含 0x0A，不能被当成换行。
        let text = (0..120)
            .map(|index| format!("上{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, utf16le_with_bom(&text)).expect("write text file");

        let PreviewContent::Text {
            rendered,
            encoding,
            next_offset,
            loaded_line_count,
            ..
        } = load_initial_text_preview(path.clone(), &PreviewExtensionRules::default_rules())
            .await
            .expect("text preview")
        else {
            panic!("expected text preview");
        };
        assert_eq!(encoding, encoding_rs::UTF_16LE);
        assert_eq!(loaded_line_count, TEXT_PREVIEW_INITIAL_LINE_LIMIT);
        assert!(rendered.ends_with("上49"));

        let chunk = load_text_preview_chunk(TextPreviewChunkRequest {
            path,
            generation: 1,
            encoding,
            start_offset: next_offset.expect("next offset"),
            loaded_line_count,
            line_limit: TEXT_PREVIEW_CHUNK_LINE_LIMIT,
        })
        .await
        .expect("chunk");
        assert!(chunk.content.starts_with("上50\n上51"));
        assert!(chunk.content.ends_with("上119"));
        assert_eq!(chunk.line_count, 70);
    }

    #[tokio::test]
    async fn reencoding_decodes_the_same_bytes_with_the_chosen_encoding() {
        let temp_dir = tempdir().expect("temp dir");
        let path = temp_dir.path().join("report.txt");
        let text = "这是一个用简体中文写的季度工作报告。";
        std::fs::write(&path, encoding_rs::GBK.encode(text).0).expect("write text file");

        let PreviewContent::Text { rendered, .. } =
            load_reencoded_text_preview(path.clone(), encoding_rs::UTF_8, TextPreviewFormat::Plain)
                .await
                .expect("reencoded")
        else {
            panic!("expected text preview");
        };
        assert!(rendered.contains('\u{fffd}'));

        let PreviewContent::Text {
            rendered, encoding, ..
        } = load_reencoded_text_preview(path, encoding_rs::GBK, TextPreviewFormat::Plain)
            .await
            .expect("reencoded")
        else {
            panic!("expected text preview");
        };
        assert_eq!(encoding, encoding_rs::GBK);
        assert_eq!(&*rendered, text);
    }

    #[tokio::test]
    async fn reencoding_skips_only_a_matching_byte_order_mark() {
        let temp_dir = tempdir().expect("temp dir");
        let path = temp_dir.path().join("bom.txt");
        std::fs::write(&path, b"\xef\xbb\xbfabc").expect("write text file");

        let rendered = |content: PreviewContent| match content {
            PreviewContent::Text { rendered, .. } => rendered.to_string(),
            _ => panic!("expected text preview"),
        };
        let same =
            load_reencoded_text_preview(path.clone(), encoding_rs::UTF_8, TextPreviewFormat::Plain)
                .await
                .expect("utf-8");
        assert_eq!(rendered(same), "abc");
        // 换成别的编码时 BOM 也是要看的字节。
        let other =
            load_reencoded_text_preview(path, encoding_rs::WINDOWS_1252, TextPreviewFormat::Plain)
                .await
                .expect("windows-1252");
        assert_eq!(rendered(other), "ï»¿abc");
    }

    fn numbered_line_range(start: usize, end: usize) -> String {
        let mut content = String::new();
        for index in start..end {
            content.push_str(&format!("line {index}\n"));
        }
        content
    }

    fn numbered_crlf_line_range(start: usize, end: usize) -> String {
        let mut content = String::new();
        for index in start..end {
            content.push_str(&format!("line {index}\r\n"));
        }
        content
    }

    fn byte_offset_after_lines(content: &str, line_count: usize) -> u64 {
        content
            .match_indices('\n')
            .nth(line_count.saturating_sub(1))
            .map(|(index, _)| index as u64 + 1)
            .unwrap_or(content.len() as u64)
    }
}
