use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::model::PreviewContent;
use crate::text_preview::{
    render_text_preview, text_preview_loaded_line_count, TextPreviewChunk, TextPreviewChunkRequest,
    TextPreviewLineLimitNotice, TEXT_PREVIEW_INITIAL_LINE_LIMIT, TEXT_PREVIEW_LINE_LIMIT,
};

pub(crate) const PREVIEW_TEXT_LIMIT: usize = 4 * 1024 * 1024;

const TEXT_PREVIEW_READ_BLOCK_SIZE: usize = 8 * 1024;

pub(crate) async fn load_initial_text_preview(path: PathBuf) -> Result<PreviewContent, String> {
    let text_preview =
        read_text_preview_chunk(path.as_path(), 0, TEXT_PREVIEW_INITIAL_LINE_LIMIT, 0).await?;
    let (rendered, format) = render_text_preview(path.as_path(), &text_preview.content);

    Ok(PreviewContent::Text {
        path,
        rendered: Arc::from(rendered),
        format,
        next_offset: text_preview.next_offset,
        loaded_line_count: text_preview.line_count,
        line_limit_notice: text_preview.line_limit_notice,
    })
}

pub(crate) async fn load_text_preview_chunk(
    request: TextPreviewChunkRequest,
) -> Result<TextPreviewChunk, String> {
    let text_preview = read_text_preview_chunk(
        request.path.as_path(),
        request.start_offset,
        request.line_limit,
        request.loaded_line_count,
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

async fn read_text_preview_chunk(
    path: &Path,
    start_offset: u64,
    line_limit: usize,
    loaded_line_count: usize,
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

    let scan_after_line_limit =
        loaded_line_count.saturating_add(line_limit) >= TEXT_PREVIEW_LINE_LIMIT;
    let mut buffer = Vec::new();
    let mut block = vec![0; TEXT_PREVIEW_READ_BLOCK_SIZE];
    let mut line_scan = TextPreviewLineScan::default();
    let mut preview_newline_count = 0;
    let mut line_limit_truncation_len = None;
    let mut next_offset = None;
    let mut read_position = start_offset;
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
        line_scan.observe(bytes);

        if buffer.len() >= PREVIEW_TEXT_LIMIT || line_limit_truncation_len.is_some() {
            read_position = read_position.saturating_add(read_count as u64);
            if scan_after_line_limit {
                continue;
            }
            break;
        }

        let stored_len = bytes.len().min(PREVIEW_TEXT_LIMIT - buffer.len());
        let stored_bytes = &bytes[..stored_len];

        for (offset, byte) in stored_bytes.iter().enumerate() {
            if *byte == b'\n' {
                preview_newline_count += 1;
                if preview_newline_count == line_limit {
                    line_limit_truncation_len = Some(text_preview_line_limit_truncation_len(
                        &buffer,
                        stored_bytes,
                        offset,
                    ));
                    next_offset = Some(read_position.saturating_add(offset as u64 + 1));
                    break;
                }
            }
        }

        buffer.extend_from_slice(stored_bytes);
        read_position = read_position.saturating_add(read_count as u64);

        if line_limit_truncation_len.is_some() && !scan_after_line_limit {
            break;
        }
    }

    if let Some(truncation_len) = line_limit_truncation_len {
        buffer.truncate(truncation_len);
    }
    if next_offset == Some(file_len) {
        next_offset = None;
    }

    let reached_preview_end =
        reached_eof && next_offset.is_none() && start_offset + buffer.len() as u64 >= file_len;
    let valid_len = valid_utf8_prefix_len(&buffer, reached_preview_end)?;
    buffer.truncate(valid_len);
    let content = String::from_utf8(buffer)
        .map_err(|_| "Preview is only available for UTF-8 text files".to_owned())?;
    let line_count = text_preview_loaded_line_count(&content);
    let line_limit_notice = if scan_after_line_limit && next_offset.is_some() {
        TextPreviewLineLimitNotice::for_total_line_count(
            loaded_line_count.saturating_add(line_scan.total_line_count()),
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

fn text_preview_line_limit_truncation_len(
    buffered_bytes: &[u8],
    stored_bytes: &[u8],
    newline_offset: usize,
) -> usize {
    let newline_index = buffered_bytes.len() + newline_offset;
    let preceding_byte = if newline_offset > 0 {
        stored_bytes.get(newline_offset - 1).copied()
    } else {
        buffered_bytes.last().copied()
    };

    if preceding_byte == Some(b'\r') {
        newline_index - 1
    } else {
        newline_index
    }
}

#[derive(Default)]
struct TextPreviewLineScan {
    newline_count: usize,
    saw_byte: bool,
    last_byte: Option<u8>,
}

impl TextPreviewLineScan {
    fn observe(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        self.saw_byte = true;
        self.newline_count += bytes.iter().filter(|byte| **byte == b'\n').count();
        self.last_byte = bytes.last().copied();
    }

    fn total_line_count(&self) -> usize {
        if !self.saw_byte {
            return 0;
        }

        if self.last_byte == Some(b'\n') {
            self.newline_count
        } else {
            self.newline_count + 1
        }
    }
}

fn valid_utf8_prefix_len(buffer: &[u8], reached_eof: bool) -> Result<usize, String> {
    match std::str::from_utf8(buffer) {
        Ok(_) => Ok(buffer.len()),
        Err(error) if error.error_len().is_none() && !reached_eof && error.valid_up_to() > 0 => {
            Ok(error.valid_up_to())
        }
        Err(_) => Err("Preview is only available for UTF-8 text files".to_owned()),
    }
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
        } = load_initial_text_preview(text_path.clone())
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
        } = load_initial_text_preview(text_path)
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

        let PreviewContent::Text { next_offset, .. } = load_initial_text_preview(text_path.clone())
            .await
            .expect("text preview")
        else {
            panic!("expected text preview");
        };
        let request = TextPreviewChunkRequest {
            path: text_path,
            generation: 1,
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

        let PreviewContent::Text { rendered, .. } = load_initial_text_preview(text_path)
            .await
            .expect("text preview")
        else {
            panic!("expected text preview");
        };

        assert!(rendered.ends_with("line 49"));
        assert!(!rendered.ends_with('\r'));
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
