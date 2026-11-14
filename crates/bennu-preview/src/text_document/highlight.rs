//! 文档的语法高亮：每个文档一个后台线程，从头解析一遍、每 256 行存一个解析状态；窗口里
//! 要上色的整行发给它，它从最近的状态接着解析，从一次性回复通道把记号送回来。
//!
//! 解析状态留在这个线程里：正则引擎的解析状态带裸指针，不能跨线程，也不能放进消息。
//! 每个请求的回复各自结束，没有常驻的消息流；文档关掉或换掉时指令通道断开，线程随之
//! 退出。只有整行（没被切段的行）上色。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self as std_mpsc, TryRecvError};
use std::sync::Arc;

use iced::futures::channel::oneshot;
use iced::futures::Future;

use super::rows::{read_full, ByteSource, TextRow};
use super::TextDecoding;
use crate::text_highlight::{
    detect_language, HighlightCheckpoint, HighlightLanguage, LineHighlighter, TextToken,
    CHECKPOINT_LINES, HIGHLIGHT_LIMIT_BYTES,
};

const READ_BLOCK_BYTES: usize = 1 << 20;
/// 后台解析每走这么多行就看一眼有没有上色请求，可见区不用等整篇解析完。
const LINES_PER_SLICE: usize = 2_000;
/// 一次给窗口上色最多要这么多行。
const ROWS_PER_REQUEST: usize = 1_500;

/// 高亮的对外状态（状态栏据此注明「文件太大，未高亮」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightStatus {
    /// 还在加载语法包、认语言。
    Detecting,
    /// 认不出语言（纯文本）：不高亮。
    Unavailable,
    /// 超过 10 MB：不高亮。
    TooLarge,
    Ready(HighlightLanguage),
}

/// 上好色的整行：（行首偏移，记号）。
pub(crate) type HighlightedRows = Vec<(u64, Arc<[TextToken]>)>;

/// 发给高亮线程的指令。
pub(crate) enum HighlightCommand {
    /// 给这些整行（行首偏移）上色；新的请求替换还没处理的旧请求（旧回复随之取消）。
    Rows {
        wanted: Vec<u64>,
        reply: oneshot::Sender<HighlightedRows>,
    },
    /// 文件追加到了新长度：从最后一个解析状态接着往后解析。
    Grew { len: u64 },
}

/// 高亮线程的指令接收端（引擎拿去启动线程）。
#[derive(Debug)]
pub(crate) struct HighlightStart(std_mpsc::Receiver<HighlightCommand>);

pub(crate) struct HighlightState {
    status: HighlightStatus,
    /// 高亮线程的指令通道；丢掉即让线程退出。
    commands: Option<std_mpsc::Sender<HighlightCommand>>,
    /// 最近一次请求的行，没回来之前不重复请求。
    requested: Vec<u64>,
    tokens: HashMap<u64, Arc<[TextToken]>>,
}

impl HighlightState {
    /// 新文档：不超过 10 MB 就准备好高亮线程的指令通道，交给引擎启动。
    pub(crate) fn new(len: u64) -> (Self, Option<HighlightStart>) {
        if len > HIGHLIGHT_LIMIT_BYTES {
            return (Self::stopped(HighlightStatus::TooLarge), None);
        }
        let (sender, receiver) = std_mpsc::channel();
        (
            Self {
                commands: Some(sender),
                ..Self::stopped(HighlightStatus::Detecting)
            },
            Some(HighlightStart(receiver)),
        )
    }

    fn stopped(status: HighlightStatus) -> Self {
        Self {
            status,
            commands: None,
            requested: Vec::new(),
            tokens: HashMap::new(),
        }
    }

    pub(crate) fn status(&self) -> &HighlightStatus {
        &self.status
    }

    pub(crate) fn tokens(&self, row_start: u64) -> Option<&Arc<[TextToken]>> {
        self.tokens.get(&row_start)
    }

    /// 高亮线程认出了语言（None：认不出，线程已退出）。
    pub(crate) fn accept_language(&mut self, language: Option<HighlightLanguage>) {
        match language {
            Some(language) => self.status = HighlightStatus::Ready(language),
            None => *self = Self::stopped(HighlightStatus::Unavailable),
        }
    }

    pub(crate) fn accept_rows(&mut self, rows: HighlightedRows) {
        self.requested
            .retain(|start| !rows.iter().any(|(row, _)| row == start));
        self.tokens.extend(rows);
    }

    /// 窗口里还没上色的整行：发给高亮线程（已经在等的就不再发），返回回复的接收端。
    pub(crate) fn request_rows(
        &mut self,
        rows: &std::collections::VecDeque<TextRow>,
        len: u64,
    ) -> Option<oneshot::Receiver<HighlightedRows>> {
        let commands = self.commands.as_ref()?;
        if !matches!(self.status, HighlightStatus::Ready(_)) {
            return None;
        }
        let wanted: Vec<u64> = rows
            .iter()
            .filter(|row| is_whole_line(row, len) && !self.tokens.contains_key(&row.start))
            .map(|row| row.start)
            .take(ROWS_PER_REQUEST)
            .collect();
        if wanted.is_empty() || wanted == self.requested {
            return None;
        }
        self.requested.clone_from(&wanted);
        let (reply, receiver) = oneshot::channel();
        commands
            .send(HighlightCommand::Rows { wanted, reply })
            .ok()?;
        Some(receiver)
    }

    /// 文件追加：超过上限就停止高亮；否则让高亮线程接着解析新的部分。末尾被重读的行
    /// 记号作废。
    pub(crate) fn accept_append(&mut self, len: u64, stale_from: u64) {
        self.tokens.retain(|start, _| *start < stale_from);
        self.requested.clear();
        if len > HIGHLIGHT_LIMIT_BYTES {
            *self = Self::stopped(HighlightStatus::TooLarge);
            return;
        }
        if let Some(commands) = &self.commands {
            let _ = commands.send(HighlightCommand::Grew { len });
        }
    }

    /// 窗口裁掉的行，记号一起丢。
    pub(crate) fn retain_window(&mut self, first: u64, last: u64) {
        self.tokens.retain(|start, _| (first..last).contains(start));
    }
}

fn is_whole_line(row: &TextRow, len: u64) -> bool {
    row.is_line_start() && (row.ends_line || row.end >= len)
}

/// 启动高亮线程：认语言，从头解析存检查点，穿插着回应上色请求。返回的 future 在认出
/// 语言（或认不出）时完成，线程继续活到文档关掉。
pub(crate) fn highlight_worker(
    source: Arc<dyn ByteSource>,
    decoding: TextDecoding,
    path: PathBuf,
    len: u64,
    start: HighlightStart,
) -> impl Future<Output = Option<HighlightLanguage>> {
    let (detected, receiver) = oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let HighlightStart(commands) = start;
        let language = detect(&path, source.as_ref(), decoding, len);
        let _ = detected.send(language.clone());
        let Some(language) = language else {
            return;
        };
        let mut worker = Worker {
            source,
            decoding,
            len,
            checkpoints: vec![HighlightCheckpoint::at_start(
                &language,
                decoding.data_start(),
            )],
            pass: None,
            parsed_until: decoding.data_start(),
            lines_since_checkpoint: 0,
        };
        worker.restart_pass_from_last_checkpoint();
        let mut pending = None;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => worker.accept(command, &mut pending),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            if let Some((wanted, reply)) = pending.take_if(|(wanted, _)| worker.can_serve(wanted)) {
                let _ = reply.send(worker.highlight_rows(&wanted));
            } else if worker.pass.is_some() {
                worker.parse_slice();
            } else {
                // 解析完了、也没有请求：睡到下一条指令。
                let Ok(command) = commands.recv() else {
                    return;
                };
                worker.accept(command, &mut pending);
            }
        }
    });
    async move { receiver.await.ok().flatten() }
}

struct Worker {
    source: Arc<dyn ByteSource>,
    decoding: TextDecoding,
    len: u64,
    checkpoints: Vec<HighlightCheckpoint>,
    /// 正在进行的从头解析：游标与解析器。
    pass: Option<(LineCursor, LineHighlighter)>,
    parsed_until: u64,
    lines_since_checkpoint: usize,
}

impl Worker {
    fn accept(
        &mut self,
        command: HighlightCommand,
        pending: &mut Option<(Vec<u64>, oneshot::Sender<HighlightedRows>)>,
    ) {
        match command {
            HighlightCommand::Rows { wanted, reply } => *pending = Some((wanted, reply)),
            HighlightCommand::Grew { len } => {
                self.len = len;
                // 最后一个检查点之后的状态可能受旧文件尾影响（没换行的最后一行），从那里重来。
                self.restart_pass_from_last_checkpoint();
            }
        }
    }

    fn restart_pass_from_last_checkpoint(&mut self) {
        let checkpoint = self.checkpoints.last().expect("the start checkpoint");
        self.parsed_until = checkpoint.line_start;
        self.lines_since_checkpoint = 0;
        self.pass = Some((
            LineCursor::new(checkpoint.line_start),
            LineHighlighter::resume(checkpoint),
        ));
    }

    /// 解析一段，期间存检查点。
    fn parse_slice(&mut self) {
        let Some((cursor, highlighter)) = &mut self.pass else {
            return;
        };
        for _ in 0..LINES_PER_SLICE {
            let Some((line_start, text)) =
                cursor.next_line(self.source.as_ref(), self.decoding, self.len)
            else {
                self.parsed_until = self.len;
                self.pass = None;
                return;
            };
            if self.lines_since_checkpoint == CHECKPOINT_LINES {
                self.checkpoints.push(highlighter.checkpoint(line_start));
                self.lines_since_checkpoint = 0;
            }
            highlighter.advance(&text);
            self.lines_since_checkpoint += 1;
            self.parsed_until = line_start;
        }
    }

    /// 请求的第一行之前的状态已经确定（解析走过了它）才能上色。
    fn can_serve(&self, wanted: &[u64]) -> bool {
        wanted
            .first()
            .is_some_and(|first| self.pass.is_none() || self.parsed_until >= *first)
    }

    fn highlight_rows(&self, wanted: &[u64]) -> HighlightedRows {
        let (Some(&first), Some(&last)) = (wanted.first(), wanted.last()) else {
            return Vec::new();
        };
        let Some(checkpoint) = self
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.line_start <= first)
        else {
            return Vec::new();
        };
        let wanted_set: HashSet<u64> = wanted.iter().copied().collect();
        let mut highlighter = LineHighlighter::resume(checkpoint);
        let mut cursor = LineCursor::new(checkpoint.line_start);
        let mut colored = Vec::new();
        while let Some((line_start, text)) =
            cursor.next_line(self.source.as_ref(), self.decoding, self.len)
        {
            let tokens = highlighter.line(&text);
            if wanted_set.contains(&line_start) {
                colored.push((line_start, Arc::from(tokens)));
            }
            if line_start >= last {
                break;
            }
        }
        colored
    }
}

fn detect(
    path: &Path,
    source: &dyn ByteSource,
    decoding: TextDecoding,
    len: u64,
) -> Option<HighlightLanguage> {
    let data_start = decoding.data_start();
    let mut head = vec![0; (len.saturating_sub(data_start)).min(1024) as usize];
    let read = read_full(source, data_start, &mut head).ok()?;
    head.truncate(read);
    let breaks = decoding.line_breaks();
    let first_line_end = breaks
        .next_line_end(&head)
        .unwrap_or(breaks.complete_len(head.len()));
    let first_line = decoding.decode(&head[..first_line_end]);
    detect_language(path, first_line.trim_end())
}

/// 逐行读文件并解码（行首偏移、不含换行的文字）。
struct LineCursor {
    buffer: Vec<u8>,
    buffer_start: u64,
    position: u64,
}

impl LineCursor {
    fn new(from: u64) -> Self {
        Self {
            buffer: Vec::new(),
            buffer_start: from,
            position: from,
        }
    }

    fn next_line(
        &mut self,
        source: &dyn ByteSource,
        decoding: TextDecoding,
        len: u64,
    ) -> Option<(u64, String)> {
        let breaks = decoding.line_breaks();
        loop {
            let at_end = self.position >= len;
            let line_len = match breaks.next_line_end(&self.buffer) {
                Some(end) => Some(end),
                None if at_end && !self.buffer.is_empty() => {
                    Some(breaks.complete_len(self.buffer.len())).filter(|len| *len > 0)
                }
                None if at_end => return None,
                None => None,
            };
            if let Some(line_len) = line_len {
                let line_start = self.buffer_start;
                let line = &self.buffer[..line_len];
                let text = decoding.decode(&line[..line.len() - breaks.terminator_len(line)]);
                self.buffer.drain(..line_len);
                self.buffer_start += line_len as u64;
                return Some((line_start, text));
            }
            if at_end {
                return None;
            }
            let wanted = READ_BLOCK_BYTES.min((len - self.position) as usize);
            let old_len = self.buffer.len();
            self.buffer.resize(old_len + wanted, 0);
            let read = read_full(source, self.position, &mut self.buffer[old_len..]).ok()?;
            self.buffer.truncate(old_len + read);
            if read == 0 {
                // 文件比预期短（被截断）：当作到了末尾。
                self.position = len;
                continue;
            }
            self.position += read as u64;
        }
    }
}
