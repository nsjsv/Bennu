//! 打开与重新打开：读第一个窗口、Markdown 全文和用来判断文件变化的戳。全部阻塞 I/O。

use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use super::rows::{
    read_full, ByteSource, LineCountAt, RowBudget, RowReader, RowStart, TextPosition, TextRow,
};
use super::TextDecoding;
use super::{RowsRequest, EXTEND_BUDGET};
use crate::text_preview::TextPreviewFormat;

/// Markdown 渲染模式要整篇读进内存再排版，超过这个大小只显示原文（第五轮决定）。
pub const MARKDOWN_RENDER_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
/// 判断「末尾追加」时比对的原末尾字节数。
const TAIL_BYTES: u64 = 64;
/// 跳转与打开时，目标行之前、之后各读多少。
pub(crate) const ROWS_BEFORE_FOCUS: RowBudget = RowBudget {
    rows: 300,
    bytes: 128 * 1024,
};
pub(crate) const ROWS_AFTER_FOCUS: RowBudget = RowBudget {
    rows: 600,
    bytes: 256 * 1024,
};

/// 读取的落点：文件开头、某个字节所在的行、文件末尾、要显示出来的一个位置（搜索、跳行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextJump {
    Start,
    Offset(u64),
    End,
    Reveal(TextPosition),
}

/// 落点行在视口里的位置：顶到视口顶部；（文件末尾）贴住视口底部；（搜索、跳行）已经看得见
/// 就不动，看不见就放到视口约三分之一高处。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpPlace {
    Top,
    Bottom,
    Reveal,
}

/// 一次读取的结果：连续的显示行、是否到了文件头/尾、落点行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowBatch {
    pub(crate) rows: Vec<TextRow>,
    pub(crate) at_start: bool,
    pub(crate) at_end: bool,
    pub(crate) focus: Option<(u64, JumpPlace)>,
}

/// 文件此刻的样子：换文件（改名替换）、截断、改写、追加都能从它看出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub(crate) identity: (u64, u64),
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    /// 文件末尾最多 64 字节：追加只会在它后面加东西，改写会动到它。
    pub(crate) tail: Vec<u8>,
}

/// 文件变化的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Unchanged,
    /// 只在末尾追加了内容。
    Appended {
        len: u64,
    },
    /// 截断、改写或换成了另一个文件：整个重新打开。
    Rewritten,
    /// 路径上已经没有文件：继续显示已打开的句柄里的内容。
    Deleted,
}

/// 打开后的文本：句柄、解码方式、第一个窗口与 Markdown 全文。
pub struct TextOpened {
    pub(crate) source: Arc<dyn ByteSource>,
    pub(crate) decoding: TextDecoding,
    /// 编码是用户手选的（重新打开时保留，不重新识别）。
    pub(crate) encoding_chosen: bool,
    pub(crate) stamp: FileStamp,
    pub(crate) batch: RowBatch,
    pub(crate) markdown: Option<Arc<str>>,
    pub(crate) format: TextPreviewFormat,
}

impl std::fmt::Debug for TextOpened {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TextOpened")
            .field("encoding", &self.decoding.encoding().name())
            .field("len", &self.stamp.len)
            .field("rows", &self.batch.rows.len())
            .finish_non_exhaustive()
    }
}

impl TextOpened {
    pub fn format(&self) -> TextPreviewFormat {
        self.format
    }
}

/// 用已经判定好的编码打开 `file`，读落点附近的窗口。
pub(crate) fn open_text(
    file: File,
    decoding: TextDecoding,
    encoding_chosen: bool,
    format: TextPreviewFormat,
    jump: TextJump,
) -> io::Result<TextOpened> {
    let stamp = file_stamp(&file)?;
    let source: Arc<dyn ByteSource> = Arc::new(file);
    let reader = RowReader {
        source: source.as_ref(),
        decoding,
        len: stamp.len,
    };
    let known = (jump == TextJump::Start).then_some(LineCountAt {
        offset: decoding.data_start(),
        lines_before: 0,
    });
    let batch = rows_around(&reader, jump, None, known)?;
    let markdown = (format == TextPreviewFormat::Markdown
        && stamp.len - decoding.data_start() <= MARKDOWN_RENDER_LIMIT_BYTES)
        .then(|| read_whole_text(&reader))
        .transpose()?
        .map(Arc::from);
    Ok(TextOpened {
        source,
        decoding,
        encoding_chosen,
        stamp,
        batch,
        markdown,
        format,
    })
}

/// 读落点前后的显示行。`floor` 是落点之前已知的行首；`known` 是能算出落点行号的已知行数。
pub(crate) fn rows_around(
    reader: &RowReader<'_>,
    jump: TextJump,
    floor: Option<RowStart>,
    known: Option<LineCountAt>,
) -> io::Result<RowBatch> {
    let data_start = reader.decoding.data_start();
    let (focus_row, place) = match jump {
        TextJump::Start => (
            RowStart {
                start: data_start,
                line_start: data_start,
            },
            JumpPlace::Top,
        ),
        TextJump::Offset(offset) => (reader.row_at(offset, floor)?, JumpPlace::Top),
        TextJump::Reveal(position) => (reader.row_at(position.row, floor)?, JumpPlace::Reveal),
        TextJump::End => {
            let line_start = reader.line_start_of(reader.len, floor)?;
            (
                RowStart {
                    start: reader.len,
                    line_start,
                },
                JumpPlace::Bottom,
            )
        }
    };
    let line_number = match known {
        Some(known) => Some(reader.lines_before(focus_row.line_start, known)?),
        None => None,
    };
    let before = reader.rows_before(focus_row, line_number, ROWS_BEFORE_FOCUS)?;
    let after = reader.rows_after(focus_row, line_number, ROWS_AFTER_FOCUS)?;
    let focus = match place {
        JumpPlace::Top | JumpPlace::Reveal => after
            .first()
            .or(before.last())
            .map(|row| (row.start, place)),
        JumpPlace::Bottom => before.last().map(|row| (row.start, place)),
    };
    let mut rows = before;
    rows.extend(after);
    Ok(RowBatch {
        at_start: rows.first().is_none_or(|row| row.start <= data_start),
        at_end: rows.last().is_none_or(|row| row.end >= reader.len),
        rows,
        focus,
    })
}

/// 整篇解码（只给 ≤ 2 MB 的 Markdown 渲染用）。
fn read_whole_text(reader: &RowReader<'_>) -> io::Result<String> {
    let data_start = reader.decoding.data_start();
    let mut bytes = vec![0; (reader.len - data_start) as usize];
    let read = read_full(reader.source, data_start, &mut bytes)?;
    bytes.truncate(read);
    Ok(reader.decoding.decode(&bytes))
}

pub(crate) fn file_stamp(file: &File) -> io::Result<FileStamp> {
    let metadata = file.metadata()?;
    let len = metadata.len();
    let tail_start = len.saturating_sub(TAIL_BYTES);
    let mut tail = vec![0; (len - tail_start) as usize];
    let read = read_full(file, tail_start, &mut tail)?;
    tail.truncate(read);
    Ok(FileStamp {
        identity: (metadata.dev(), metadata.ino()),
        len,
        modified: metadata.modified().ok(),
        tail,
    })
}

/// 比对路径上现在的文件与上次的戳。
pub(crate) fn check_file_change(
    path: &Path,
    previous: &FileStamp,
) -> io::Result<(FileChange, Option<FileStamp>)> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((FileChange::Deleted, None));
        }
        Err(error) => return Err(error),
    };
    let current = file_stamp(&file)?;
    let change = if current.identity != previous.identity || current.len < previous.len {
        FileChange::Rewritten
    } else if current.len == previous.len {
        if current.modified == previous.modified && current.tail == previous.tail {
            FileChange::Unchanged
        } else {
            FileChange::Rewritten
        }
    } else {
        // 变长了：原来末尾那几个字节还在原位才是追加。
        let tail_start = previous.len - previous.tail.len() as u64;
        let mut old_tail = vec![0; previous.tail.len()];
        let read = read_full(&file, tail_start, &mut old_tail)?;
        if read == old_tail.len() && old_tail == previous.tail {
            FileChange::Appended { len: current.len }
        } else {
            FileChange::Rewritten
        }
    };
    Ok((change, Some(current)))
}

/// 执行一次显示行读取请求。
pub(crate) fn read_rows(
    source: &dyn ByteSource,
    decoding: TextDecoding,
    len: u64,
    request: RowsRequest,
) -> std::io::Result<RowBatch> {
    let reader = RowReader {
        source,
        decoding,
        len,
    };
    match request {
        RowsRequest::Forward { from, line_number } => {
            let rows = reader.rows_after(from, line_number, EXTEND_BUDGET)?;
            Ok(RowBatch {
                at_start: false,
                at_end: rows.last().map_or(from.start, |row| row.end) >= len,
                rows,
                focus: None,
            })
        }
        RowsRequest::Backward {
            before,
            line_number,
        } => {
            let rows = reader.rows_before(before, line_number, EXTEND_BUDGET)?;
            Ok(RowBatch {
                at_start: rows.first().map_or(before.start, |row| row.start)
                    <= decoding.data_start(),
                at_end: false,
                rows,
                focus: None,
            })
        }
        RowsRequest::Around { jump, floor, known } => rows_around(&reader, jump, floor, known),
    }
}
