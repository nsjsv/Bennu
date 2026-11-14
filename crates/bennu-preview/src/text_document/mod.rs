//! 文本预览文档：按需从磁盘读取的窗口模型。
//!
//! 文档只留视口附近的一段显示行（窗口），往前往后按需补、离视口远的丢掉，内存上限固定；
//! 位置一律用字节偏移表示（行号要等后台统计，字节偏移随时都知道）。读取都在后台：文档
//! 只产出读取请求（`TextRead`），引擎执行后把结果交回来。

mod decoding;
mod highlight;
mod index;
mod line_breaks;
mod open;
mod rows;
mod watch;
mod window;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

pub use decoding::TextDecoding;
use highlight::HighlightState;
pub use highlight::HighlightStatus;
pub(crate) use highlight::{highlight_worker, HighlightStart, HighlightedRows};
pub use index::{IndexProgress, LineCountStatus};
pub use open::{
    FileChange, JumpPlace, RowBatch, TextJump, TextOpened, MARKDOWN_RENDER_LIMIT_BYTES,
};
pub use rows::{TextPosition, TextRow};
pub use watch::text_file_watch_subscription;

pub(crate) use index::{count_lines, IndexCursor};
pub(crate) use open::{check_file_change, open_text, read_rows, FileStamp};
pub(crate) use rows::{read_full, ByteSource, LineCountAt, RowBudget, RowReader, RowStart};

use index::LineIndex;

use crate::text_preview::{MarkdownPreviewMode, TextPreviewFormat};

/// 往前/往后补一次读多少。
pub(crate) const EXTEND_BUDGET: RowBudget = RowBudget {
    rows: 600,
    bytes: 256 * 1024,
};
/// 视口外剩下不到这么多行就补。
const PREFETCH_ROWS: usize = 200;
/// 窗口上限：超出时从离视口远的一端丢。
const WINDOW_MAX_ROWS: usize = 4_000;
const WINDOW_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// 复制超过这么多字节不执行（用户定的上限）。
pub const COPY_LIMIT_BYTES: u64 = 15 * 1024 * 1024;

/// 查看器报告的可见范围（显示行起点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextViewport {
    pub first_row: u64,
    pub last_row: u64,
    /// 视口贴着文件末尾（追加内容时要跟到新末尾）。
    pub at_bottom: bool,
    /// 光标位置（状态栏显示所在行）。
    pub cursor: Option<TextPosition>,
}

/// 给查看器的跳转指令：revision 变了就把锚点放到 `row`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerJump {
    pub revision: u64,
    pub row: u64,
    pub place: JumpPlace,
}

/// 查看器发给引擎的事件（滚轮另走宿主的平滑滚动管线）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextViewerEvent {
    ViewportChanged(TextViewport),
    JumpRequested(TextJump),
    /// 选区超出窗口，要后台读出来再复制。
    CopyRequested {
        from: TextPosition,
        to: TextPosition,
        target: ClipboardTarget,
    },
    /// 调整字号或换行；设置归宿主（主程序记住，对话框只当次有效），引擎原样还给宿主。
    ChangeView(crate::text_view::TextViewChange),
}

/// 复制写到哪：普通剪贴板（Ctrl+C）或 Linux 主选区（选中即写，中键粘贴）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTarget {
    Standard,
    Primary,
}

impl ClipboardTarget {
    pub(crate) fn kind(self) -> iced::advanced::clipboard::Kind {
        match self {
            Self::Standard => iced::advanced::clipboard::Kind::Standard,
            Self::Primary => iced::advanced::clipboard::Kind::Primary,
        }
    }
}

/// 后台读取的回复；内容只有引擎和文档看得懂。
#[derive(Debug, Clone)]
pub struct TextReadReply(pub(crate) TextReadReplyKind);

#[derive(Debug, Clone)]
pub(crate) enum TextReadReplyKind {
    Rows {
        generation: u64,
        request: RowsRequest,
        outcome: Result<RowBatch, String>,
    },
    Number {
        generation: u64,
        line_start: u64,
        outcome: Result<u64, String>,
    },
    Index(IndexProgress),
    FileChecked(Result<(FileChange, Option<FileStamp>), String>),
    Selection {
        target: ClipboardTarget,
        outcome: Result<String, String>,
    },
    HighlightLanguage(Option<crate::text_highlight::HighlightLanguage>),
    HighlightRows(HighlightedRows),
    Reopened(Result<Arc<TextOpened>, String>),
}

/// 文档要的后台读取。
#[derive(Debug)]
pub(crate) enum TextRead {
    Rows {
        generation: u64,
        request: RowsRequest,
    },
    /// 窗口第一行（一个原始行的行首）的行号。
    Number {
        generation: u64,
        line_start: u64,
        known: LineCountAt,
    },
    Count {
        start: IndexCursor,
        first_checkpoint: usize,
        len: u64,
        cancel: CancellationToken,
    },
    /// 启动这个文档的高亮线程。
    Highlight(HighlightStart),
    /// 等高亮线程给这些行上色的回复（被更新的请求替换时取消）。
    HighlightRows(iced::futures::channel::oneshot::Receiver<HighlightedRows>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowsRequest {
    Forward {
        from: RowStart,
        line_number: Option<u64>,
    },
    Backward {
        before: RowStart,
        line_number: Option<u64>,
    },
    Around {
        jump: TextJump,
        floor: Option<RowStart>,
        known: Option<LineCountAt>,
    },
}

pub struct TextPreviewDocument {
    id: u64,
    path: PathBuf,
    format: TextPreviewFormat,
    source: Arc<dyn ByteSource>,
    decoding: TextDecoding,
    encoding_chosen: bool,
    stamp: FileStamp,
    markdown: Option<Arc<str>>,
    markdown_preview_mode: MarkdownPreviewMode,
    rows: VecDeque<TextRow>,
    /// 窗口被整个替换（跳转）时加一；旧窗口的补读结果到达时丢弃。
    generation: u64,
    at_start: bool,
    at_end: bool,
    index: LineIndex,
    /// 正在跑的行号统计；文档被替换或关掉时取消。
    index_run: Option<IndexRun>,
    forward_in_flight: bool,
    backward_in_flight: bool,
    jump_in_flight: bool,
    queued_jump: Option<TextJump>,
    numbering_in_flight: bool,
    viewport: Option<TextViewport>,
    /// 追加后要跟到新末尾（追加前视口贴着末尾）。
    follow_end: bool,
    jump: Option<ViewerJump>,
    revision: u64,
    read_error: Option<String>,
    highlight: HighlightState,
}

struct IndexRun {
    cancel: CancellationToken,
    target_len: u64,
}

impl Drop for TextPreviewDocument {
    fn drop(&mut self) {
        if let Some(run) = &self.index_run {
            run.cancel.cancel();
        }
    }
}

impl TextPreviewDocument {
    /// 用打开结果建文档，并给出要开始的后台读取（行号统计）。
    pub(crate) fn open(id: u64, path: PathBuf, opened: &TextOpened) -> (Self, Vec<TextRead>) {
        let (highlight, highlight_start) = HighlightState::new(opened.stamp.len);
        let mut document = Self {
            id,
            path,
            format: opened.format,
            source: Arc::clone(&opened.source),
            decoding: opened.decoding,
            encoding_chosen: opened.encoding_chosen,
            stamp: opened.stamp.clone(),
            markdown: opened.markdown.clone(),
            markdown_preview_mode: match (opened.format, &opened.markdown) {
                (TextPreviewFormat::Markdown, Some(_)) => MarkdownPreviewMode::Rendered,
                _ => MarkdownPreviewMode::Raw,
            },
            rows: VecDeque::new(),
            generation: 0,
            at_start: true,
            at_end: true,
            index: LineIndex::new(opened.decoding.data_start()),
            index_run: None,
            forward_in_flight: false,
            backward_in_flight: false,
            jump_in_flight: false,
            queued_jump: None,
            numbering_in_flight: false,
            viewport: None,
            follow_end: false,
            jump: None,
            revision: 0,
            read_error: None,
            highlight,
        };
        document.replace_window(opened.batch.clone());
        let mut reads: Vec<TextRead> = document.start_counting().into_iter().collect();
        reads.extend(highlight_start.map(TextRead::Highlight));
        (document, reads)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn format(&self) -> TextPreviewFormat {
        self.format
    }

    pub fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.decoding.encoding()
    }

    pub fn encoding_chosen(&self) -> bool {
        self.encoding_chosen
    }

    pub(crate) fn decoding(&self) -> TextDecoding {
        self.decoding
    }

    pub(crate) fn source(&self) -> Arc<dyn ByteSource> {
        Arc::clone(&self.source)
    }

    pub(crate) fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    pub fn len(&self) -> u64 {
        self.stamp.len
    }

    pub fn is_empty(&self) -> bool {
        self.stamp.len <= self.decoding.data_start()
    }

    pub fn data_start(&self) -> u64 {
        self.decoding.data_start()
    }

    pub fn rows(&self) -> &VecDeque<TextRow> {
        &self.rows
    }

    /// 起点为 `start` 的行在窗口里的下标；不在窗口里时是它该插入的位置。
    pub fn row_index(&self, start: u64) -> Result<usize, usize> {
        self.rows.binary_search_by_key(&start, |row| row.start)
    }

    /// 包含 `offset` 的窗口行下标。
    pub fn row_index_containing(&self, offset: u64) -> Option<usize> {
        let index = match self.row_index(offset) {
            Ok(index) => index,
            Err(0) => return None,
            Err(index) => index - 1,
        };
        (offset < self.rows[index].end || index + 1 == self.rows.len()).then_some(index)
    }

    pub fn at_start(&self) -> bool {
        self.at_start
    }

    pub fn at_end(&self) -> bool {
        self.at_end
    }

    /// 窗口内容（行、行号）变过就加一，查看器据此刷新。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn jump(&self) -> Option<ViewerJump> {
        self.jump
    }

    /// 查看器最近报告的可见范围（改编码、文件改写重新打开时据此保位置）。
    pub fn viewport(&self) -> Option<TextViewport> {
        self.viewport
    }

    pub fn read_error(&self) -> Option<&str> {
        self.read_error.as_deref()
    }

    pub fn line_count_status(&self) -> LineCountStatus {
        self.index.status(self.stamp.len)
    }

    /// 估算的总显示行数：统计完用总行数，否则按窗口里平均每行字节数推算。
    pub fn estimated_total_rows(&self) -> u64 {
        let span = self.stamp.len.saturating_sub(self.decoding.data_start());
        let window_bytes: u64 = self.rows.iter().map(|row| row.end - row.start).sum();
        let estimate = (span * self.rows.len() as u64)
            .checked_div(window_bytes)
            .unwrap_or(0);
        match self.line_count_status() {
            LineCountStatus::Done { total } => total.max(estimate.min(total * 2)).max(1),
            LineCountStatus::Counting { lines_so_far } => estimate.max(lines_so_far).max(1),
        }
    }

    /// 行号栏要容纳的最大行号（总行数或估算值）。
    pub fn largest_line_number(&self) -> u64 {
        match self.line_count_status() {
            LineCountStatus::Done { total } => total.max(1),
            LineCountStatus::Counting { .. } => self.estimated_total_rows(),
        }
    }

    pub fn markdown(&self) -> Option<&Arc<str>> {
        self.markdown.as_ref()
    }

    pub fn markdown_preview_mode(&self) -> MarkdownPreviewMode {
        self.markdown_preview_mode
    }

    /// 只有拿到了渲染全文（≤ 2 MB）才能切到渲染模式。
    pub fn select_markdown_preview_mode(&mut self, mode: MarkdownPreviewMode) {
        if mode == MarkdownPreviewMode::Raw || self.markdown.is_some() {
            self.markdown_preview_mode = mode;
        }
    }

    /// 查看器可见范围变了：记下来，视口外剩得不多就补读。
    pub(crate) fn accept_viewport(&mut self, viewport: TextViewport) -> Vec<TextRead> {
        self.viewport = Some(viewport);
        if self.read_error.take().is_some() {
            self.revision += 1;
        }
        self.trim_window();
        let mut reads = self.extension_reads();
        reads.extend(self.highlight_read());
        reads
    }

    /// 跳到文件开头/某处/末尾：读落点附近的窗口整个替换当前窗口。
    pub(crate) fn jump_to(&mut self, jump: TextJump) -> Vec<TextRead> {
        if self.jump_in_flight {
            self.queued_jump = Some(jump);
            return Vec::new();
        }
        self.generation += 1;
        self.jump_in_flight = true;
        self.forward_in_flight = false;
        self.backward_in_flight = false;
        self.numbering_in_flight = false;
        let offset = match jump {
            TextJump::Start => self.decoding.data_start(),
            TextJump::Offset(offset) => offset,
            TextJump::End => self.stamp.len,
        };
        vec![TextRead::Rows {
            generation: self.generation,
            request: RowsRequest::Around {
                jump,
                floor: self.index.line_start_floor(offset),
                known: self.index.line_count_before(offset),
            },
        }]
    }

    /// 后台读到的显示行。
    pub(crate) fn accept_rows(
        &mut self,
        generation: u64,
        request: RowsRequest,
        outcome: Result<RowBatch, String>,
    ) -> Vec<TextRead> {
        if generation != self.generation {
            return Vec::new();
        }
        match request {
            RowsRequest::Forward { .. } => self.forward_in_flight = false,
            RowsRequest::Backward { .. } => self.backward_in_flight = false,
            RowsRequest::Around { .. } => self.jump_in_flight = false,
        }
        let batch = match outcome {
            Ok(batch) => batch,
            Err(error) => {
                self.read_error = Some(error);
                self.revision += 1;
                return self
                    .queued_jump
                    .take()
                    .map(|jump| self.jump_to(jump))
                    .unwrap_or_default();
            }
        };
        match request {
            RowsRequest::Forward { .. } => self.append_rows(batch),
            RowsRequest::Backward { .. } => self.prepend_rows(batch),
            RowsRequest::Around { .. } => {
                if let Some(jump) = self.queued_jump.take() {
                    // 拖动滑块时只有最新的落点有意义。
                    return self.jump_to(jump);
                }
                self.replace_window(batch);
            }
        }
        self.trim_window();
        let mut reads = self.extension_reads();
        reads.extend(self.numbering_read());
        reads.extend(self.highlight_read());
        reads
    }

    /// 窗口第一行行号算出来了：按顺序给整个窗口编号。
    pub(crate) fn accept_numbering(
        &mut self,
        generation: u64,
        line_start: u64,
        outcome: Result<u64, String>,
    ) -> Vec<TextRead> {
        if generation != self.generation {
            return Vec::new();
        }
        self.numbering_in_flight = false;
        let Ok(mut line_number) = outcome else {
            return Vec::new();
        };
        let Some(first) = self
            .rows
            .iter()
            .position(|row| row.line_start == line_start)
        else {
            return self.numbering_read().into_iter().collect();
        };
        for (index, row) in self.rows.iter_mut().enumerate().skip(first) {
            if row.is_line_start() && index != first {
                line_number += 1;
            }
            row.line_number = Some(line_number);
        }
        // 第一行之前（同一原始行的前几段或更早的行）往回编。
        let mut line_number = self.rows[first].line_number.unwrap_or_default();
        for index in (0..first).rev() {
            if self.rows[index + 1].is_line_start() {
                line_number = line_number.saturating_sub(1);
            }
            self.rows[index].line_number = Some(line_number);
        }
        self.revision += 1;
        Vec::new()
    }

    pub(crate) fn accept_index_progress(&mut self, progress: IndexProgress) -> Vec<TextRead> {
        let finished = progress.finished;
        self.index.merge(progress);
        self.revision += 1;
        let mut reads: Vec<TextRead> = self.numbering_read().into_iter().collect();
        if finished {
            let grew = self
                .index_run
                .as_ref()
                .is_some_and(|run| self.stamp.len > run.target_len);
            self.index_run = None;
            if grew {
                reads.extend(self.start_counting());
            }
        }
        reads
    }

    /// 文件末尾追加了内容。
    pub(crate) fn accept_append(&mut self, stamp: FileStamp) -> Vec<TextRead> {
        let old_len = self.stamp.len;
        self.stamp = stamp;
        self.follow_end = self.viewport.is_some_and(|viewport| viewport.at_bottom);
        if self.at_end {
            // 末尾几行的边界可能受旧文件尾影响（没换行的最后一行、切点附近的字形簇）：重读。
            while self
                .rows
                .back()
                .is_some_and(|row| row.end + decoding_lookahead() > old_len)
            {
                self.rows.pop_back();
            }
            self.at_end = false;
            self.revision += 1;
        }
        let mut reads = self.extension_reads();
        if self.index_run.is_none() {
            reads.extend(self.start_counting());
        }
        let stale_from = self.rows.back().map_or(0, |row| row.end);
        self.highlight.accept_append(self.stamp.len, stale_from);
        reads
    }

    pub fn highlight_status(&self) -> &HighlightStatus {
        self.highlight.status()
    }

    /// 这一行（整行）的高亮记号；还没解析到或不高亮时 None。
    pub fn row_tokens(&self, row_start: u64) -> Option<&Arc<[crate::text_highlight::TextToken]>> {
        self.highlight.tokens(row_start)
    }

    pub(crate) fn accept_highlight_language(
        &mut self,
        language: Option<crate::text_highlight::HighlightLanguage>,
    ) -> Vec<TextRead> {
        self.highlight.accept_language(language);
        self.revision += 1;
        self.highlight_read().into_iter().collect()
    }

    pub(crate) fn accept_highlight_rows(&mut self, rows: HighlightedRows) -> Vec<TextRead> {
        self.highlight.accept_rows(rows);
        // 回复到达前窗口可能已经被跳转替换：只留窗口里的。
        if let (Some(first), Some(last)) = (self.rows.front(), self.rows.back()) {
            self.highlight.retain_window(first.start, last.end);
        }
        self.revision += 1;
        // 窗口比一次请求的行数多：接着要还没上色的行。
        self.highlight_read().into_iter().collect()
    }

    fn highlight_read(&mut self) -> Option<TextRead> {
        self.highlight
            .request_rows(&self.rows, self.stamp.len)
            .map(TextRead::HighlightRows)
    }

    /// 选区 `[from, to)` 能不能复制（不超过 15 MB）。
    pub fn copy_fits(from: TextPosition, to: TextPosition) -> bool {
        to.row.saturating_sub(from.row) <= COPY_LIMIT_BYTES
    }

    fn start_counting(&mut self) -> Option<TextRead> {
        if self.index.is_complete(self.stamp.len) {
            return None;
        }
        let cancel = CancellationToken::new();
        self.index_run = Some(IndexRun {
            cancel: cancel.clone(),
            target_len: self.stamp.len,
        });
        Some(TextRead::Count {
            start: self.index.cursor(),
            first_checkpoint: self.index.checkpoint_count(),
            len: self.stamp.len,
            cancel,
        })
    }
}

#[cfg(test)]
impl TextPreviewDocument {
    /// 测试用：换一个文档 id（模拟打开了另一个文件）。
    pub(crate) fn reidentified_for_tests(mut document: Self, id: u64) -> Self {
        document.id = id;
        document
    }
}

/// 测试用：把内存里的 UTF-8 字节当成文件打开，窗口读在 `jump` 处。
#[cfg(test)]
pub(crate) fn document_from_bytes(bytes: Vec<u8>, jump: TextJump) -> TextPreviewDocument {
    let len = bytes.len() as u64;
    let source: Arc<dyn ByteSource> = Arc::new(bytes);
    let decoding = TextDecoding::new(encoding_rs::UTF_8, 0);
    let reader = RowReader {
        source: source.as_ref(),
        decoding,
        len,
    };
    let known = Some(LineCountAt {
        offset: 0,
        lines_before: 0,
    });
    let batch = open::rows_around(&reader, jump, None, known).expect("rows");
    let opened = TextOpened {
        source,
        decoding,
        encoding_chosen: false,
        stamp: FileStamp {
            identity: (0, 0),
            len,
            modified: None,
            tail: Vec::new(),
        },
        batch,
        markdown: None,
        format: TextPreviewFormat::Plain,
    };
    TextPreviewDocument::open(1, PathBuf::from("/tmp/text-preview-test.txt"), &opened).0
}

fn decoding_lookahead() -> u64 {
    decoding::CUT_LOOKAHEAD_BYTES
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
