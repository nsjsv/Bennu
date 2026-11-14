//! 行号索引：后台从头到尾数一遍换行，每 1 MiB 记一个检查点。任意偏移的行号 = 最近的
//! 检查点 + 检查点之后最多 1 MiB 里的换行数；2 GB 文件只有 2048 个检查点（约 32 KB）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc;
use iced::futures::Stream;
use tokio_util::sync::CancellationToken;

use super::rows::{read_full, ByteSource, LineCountAt, RowReader, RowStart};
use super::TextDecoding;

/// 检查点间隔，也是统计时每次读的块大小。
pub(crate) const CHECKPOINT_BYTES: u64 = 1 << 20;
/// 进度至少每数这么多字节、或者每隔这么久报一次。
const PROGRESS_BYTES: u64 = 32 * CHECKPOINT_BYTES;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// 第 i 个检查点对应偏移 `data_start + i × CHECKPOINT_BYTES`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Checkpoint {
    /// 检查点之前有几个换行（= 检查点所在原始行的行号）。
    pub(crate) lines_before: u64,
    /// 检查点所在原始行的行首：往回找行首最多找到这里。
    pub(crate) line_start: u64,
}

/// 数到哪了：续数（文件追加）从这里接着。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexCursor {
    pub(crate) counted_until: u64,
    pub(crate) lines_before: u64,
    pub(crate) last_line_start: u64,
}

/// 一次进度：新增的检查点与最新的游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexProgress {
    pub(crate) first_checkpoint: usize,
    pub(crate) checkpoints: Vec<Checkpoint>,
    pub(crate) cursor: IndexCursor,
    /// 这一轮统计结束了（数到了预定长度、读出错或文件变短）。
    pub(crate) finished: bool,
}

/// 行数统计的对外状态（状态栏在子任务 3 显示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCountStatus {
    /// 还在数，已经数到这么多行。
    Counting {
        lines_so_far: u64,
    },
    Done {
        total: u64,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct LineIndex {
    data_start: u64,
    checkpoints: Vec<Checkpoint>,
    cursor: IndexCursor,
}

impl LineIndex {
    pub(crate) fn new(data_start: u64) -> Self {
        Self {
            data_start,
            checkpoints: Vec::new(),
            cursor: IndexCursor {
                counted_until: data_start,
                lines_before: 0,
                last_line_start: data_start,
            },
        }
    }

    pub(crate) fn cursor(&self) -> IndexCursor {
        self.cursor
    }

    pub(crate) fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    pub(crate) fn status(&self, len: u64) -> LineCountStatus {
        if self.cursor.counted_until < len {
            return LineCountStatus::Counting {
                lines_so_far: self.cursor.lines_before,
            };
        }
        // 最后一行没有换行结尾时也算一行；以换行结尾的文件没有空的末行。
        let unfinished_line = u64::from(self.cursor.last_line_start < len);
        LineCountStatus::Done {
            total: self.cursor.lines_before + unfinished_line,
        }
    }

    pub(crate) fn is_complete(&self, len: u64) -> bool {
        self.cursor.counted_until >= len
    }

    pub(crate) fn merge(&mut self, progress: IndexProgress) {
        self.checkpoints.truncate(progress.first_checkpoint);
        self.checkpoints.extend(progress.checkpoints);
        self.cursor = progress.cursor;
    }

    /// `offset` 已经数到时，离它最近的已知行数位置（用来算 `offset` 的行号）。
    pub(crate) fn line_count_before(&self, offset: u64) -> Option<LineCountAt> {
        if offset > self.cursor.counted_until {
            return None;
        }
        let checkpoint = self.checkpoint_at_or_before(offset)?;
        Some(LineCountAt {
            offset: self.checkpoint_offset(checkpoint),
            lines_before: self.checkpoints[checkpoint].lines_before,
        })
    }

    /// 不晚于 `offset` 的一个已知行首，往回找行首最多找到这里。
    pub(crate) fn line_start_floor(&self, offset: u64) -> Option<RowStart> {
        let checkpoint = self.checkpoint_at_or_before(offset)?;
        let line_start = self.checkpoints[checkpoint].line_start;
        Some(RowStart {
            start: line_start,
            line_start,
        })
    }

    /// 第 `line` 行（从 0 数）之前最近的已知位置：从这里往后数换行就到它的行首。统计
    /// 还没数到那一行时从数到的地方接着数，不必等统计。
    pub(crate) fn seek_line(&self, line: u64) -> LineSeek {
        if self.cursor.lines_before <= line {
            return LineSeek {
                offset: self.cursor.counted_until,
                lines_before: self.cursor.lines_before,
                line_start: self.cursor.last_line_start,
            };
        }
        let after = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.lines_before <= line);
        match after.checked_sub(1) {
            Some(index) => LineSeek {
                offset: self.checkpoint_offset(index),
                lines_before: self.checkpoints[index].lines_before,
                line_start: self.checkpoints[index].line_start,
            },
            None => LineSeek {
                offset: self.data_start,
                lines_before: 0,
                line_start: self.data_start,
            },
        }
    }

    fn checkpoint_at_or_before(&self, offset: u64) -> Option<usize> {
        let index = (offset.checked_sub(self.data_start)? / CHECKPOINT_BYTES) as usize;
        Some(index.min(self.checkpoints.len().checked_sub(1)?))
    }

    fn checkpoint_offset(&self, index: usize) -> u64 {
        self.data_start + index as u64 * CHECKPOINT_BYTES
    }
}

/// 已知行号的位置：`offset` 之前有 `lines_before` 个换行，`offset` 所在行的行首是
/// `line_start`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineSeek {
    pub(crate) offset: u64,
    pub(crate) lines_before: u64,
    pub(crate) line_start: u64,
}

/// 第 `line` 行（从 0 数）的行首：从 `seek` 往后数换行。超出文件时夹到最后一行。
pub(crate) fn line_start_for_number(
    reader: &RowReader<'_>,
    line: u64,
    seek: LineSeek,
) -> std::io::Result<u64> {
    let breaks = reader.decoding.line_breaks();
    let mut remaining = line.saturating_sub(seek.lines_before);
    let mut position = seek.offset;
    let mut line_start = seek.line_start;
    let mut block = vec![0; CHECKPOINT_BYTES as usize];
    while remaining > 0 && position < reader.len {
        let wanted = (reader.len - position).min(CHECKPOINT_BYTES) as usize;
        let read = breaks.complete_len(read_full(reader.source, position, &mut block[..wanted])?);
        if read == 0 {
            break;
        }
        let bytes = &block[..read];
        let count = breaks.count(bytes);
        if count < remaining {
            if let Some(last) = breaks.last_line_end(bytes) {
                line_start = position + last as u64;
            }
            remaining -= count;
            position += read as u64;
            continue;
        }
        let mut consumed = 0;
        while remaining > 0 {
            consumed += breaks
                .next_line_end(&bytes[consumed..])
                .expect("the block holds at least `remaining` line breaks");
            remaining -= 1;
        }
        line_start = position + consumed as u64;
    }
    // 以换行结尾的文件没有空的末行：文件尾不是行首，夹到最后一行。
    if line_start >= reader.len && reader.len > reader.decoding.data_start() {
        let unit = breaks.unit_len() as u64;
        return reader.line_start_of(reader.len - unit, None);
    }
    Ok(line_start)
}

/// 从 `start` 数到 `len`，按节流报进度，结束时再报一次（文件比预期短、读出错时停在
/// 实际数到的位置）。`cancel` 触发就停，不再报。
pub(crate) fn count_lines(
    source: Arc<dyn ByteSource>,
    decoding: TextDecoding,
    len: u64,
    start: IndexCursor,
    first_checkpoint: usize,
    cancel: CancellationToken,
) -> impl Stream<Item = IndexProgress> {
    let (sender, receiver) = mpsc::unbounded();
    tokio::task::spawn_blocking(move || {
        let breaks = decoding.line_breaks();
        let data_start = decoding.data_start();
        let mut cursor = start;
        let mut pending = Vec::new();
        let mut pending_first = first_checkpoint;
        let mut block = vec![0; CHECKPOINT_BYTES as usize];
        let mut last_report = (Instant::now(), cursor.counted_until);
        while cursor.counted_until < len && !cancel.is_cancelled() {
            let position = cursor.counted_until;
            let grid_offset = position - data_start;
            if grid_offset.is_multiple_of(CHECKPOINT_BYTES) {
                pending.push(Checkpoint {
                    lines_before: cursor.lines_before,
                    line_start: cursor.last_line_start,
                });
            }
            let block_end =
                (data_start + (grid_offset / CHECKPOINT_BYTES + 1) * CHECKPOINT_BYTES).min(len);
            let wanted = (block_end - position) as usize;
            let read = match read_full(source.as_ref(), position, &mut block[..wanted]) {
                Ok(read) => breaks.complete_len(read),
                Err(_) => 0,
            };
            if read == 0 {
                break;
            }
            let bytes = &block[..read];
            cursor.lines_before += breaks.count(bytes);
            if let Some(line_end) = breaks.last_line_end(bytes) {
                cursor.last_line_start = position + line_end as u64;
            }
            cursor.counted_until += read as u64;
            source.release(position, read as u64);
            let (reported_at, reported_until) = last_report;
            if cursor.counted_until - reported_until >= PROGRESS_BYTES
                || reported_at.elapsed() >= PROGRESS_INTERVAL
            {
                let progress = IndexProgress {
                    first_checkpoint: pending_first,
                    checkpoints: std::mem::take(&mut pending),
                    cursor,
                    finished: false,
                };
                pending_first += progress.checkpoints.len();
                if sender.unbounded_send(progress).is_err() {
                    return;
                }
                last_report = (Instant::now(), cursor.counted_until);
            }
        }
        if !cancel.is_cancelled() {
            let _ = sender.unbounded_send(IndexProgress {
                first_checkpoint: pending_first,
                checkpoints: pending,
                cursor,
                finished: true,
            });
        }
    });
    receiver
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
