//! 搜索的后台读取：按块读完整的原始行，整块解码后一次匹配（几千万个短行不必逐行分配）。
//! 一块里没有换行的超长行按显示段分批读，批之间留一小段重叠，跨批的匹配不丢。
//!
//! 页缓存不主动丢：换个词再搜同一个大文件时，从缓存读比从盘读快一个数量级。

use std::io;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc;
use iced::futures::Stream;
use regex::{Regex, RegexBuilder};
use tokio_util::sync::CancellationToken;

use super::decoding::SEGMENT_BYTES;
use super::line_breaks::LineBreaks;
use super::rows::{read_full, ByteSource, RowBudget, RowReader, RowStart, TextPosition};
use super::TextDecoding;

/// 每次读的块，也是计数检查点的大致间隔。
const SCAN_BLOCK_BYTES: usize = 1 << 20;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

/// 搜索词：字面量；默认不分大小写。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
}

/// 编译好的匹配器。
#[derive(Clone)]
pub(crate) struct SearchMatcher {
    regex: Arc<Regex>,
    /// 一处匹配最多占多少字节：不分大小写时一个字符可能匹配到 4 字节的字符。
    max_match_bytes: usize,
}

impl std::fmt::Debug for SearchMatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("SearchMatcher")
            .field(&self.regex.as_str())
            .finish()
    }
}

impl SearchMatcher {
    /// 只在一个原始行里匹配：搜索词换行之后的部分不要；空词没有匹配器。
    pub(crate) fn new(query: &SearchQuery) -> Option<Self> {
        let text = query.text.split(['\n', '\r']).next().unwrap_or_default();
        if text.is_empty() {
            return None;
        }
        let regex = RegexBuilder::new(&regex::escape(text))
            .case_insensitive(!query.case_sensitive)
            .build()
            .ok()?;
        Some(Self {
            regex: Arc::new(regex),
            max_match_bytes: text.chars().count() * 4,
        })
    }

    pub(crate) fn find_iter<'a>(
        &'a self,
        text: &'a str,
    ) -> impl Iterator<Item = Range<usize>> + 'a {
        self.regex.find_iter(text).map(|found| found.range())
    }
}

/// 一处匹配：所在原始行的行首与起止位置（终点可以在同一原始行的下一段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextMatch {
    pub line_start: u64,
    pub start: TextPosition,
    pub end: TextPosition,
}

/// 拼起来的一段文字里，第 `text_start` 字节起对应显示位置 `(row, column)`。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextAnchor {
    pub(crate) text_start: usize,
    pub(crate) row: u64,
    pub(crate) column: usize,
}

/// 拼接文字里的下标 → 显示位置；`anchors` 按 `text_start` 升序，第一个从 0 开始。
pub(crate) fn anchored_position(anchors: &[TextAnchor], index: usize) -> TextPosition {
    let anchor = anchors[anchors
        .partition_point(|anchor| anchor.text_start <= index)
        .saturating_sub(1)];
    TextPosition {
        row: anchor.row,
        column: anchor.column + (index - anchor.text_start),
    }
}

/// 扫描时找到的一处匹配。没切段的行位置直接可知；可能切了段的行要按段换算（`locate`）。
pub(crate) struct FoundMatch {
    pub(crate) line_start: u64,
    place: FoundPlace,
}

enum FoundPlace {
    /// 整行解码后的字节范围。
    InLine {
        columns: Range<usize>,
        may_be_segmented: bool,
    },
    Located(TextMatch),
}

pub(crate) enum ScanEvent {
    Found(FoundMatch),
    /// `next_line` 之前的行都扫完了（计数在这里记检查点）。
    LinesScanned {
        next_line: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScanEnd {
    /// 扫到了 `to`（或文件比预期短，扫到了实际的末尾）。
    Completed,
    Stopped,
    Cancelled,
}

pub(crate) struct MatchScanner<'a> {
    pub(crate) reader: RowReader<'a>,
    pub(crate) matcher: &'a SearchMatcher,
}

impl MatchScanner<'_> {
    fn breaks(&self) -> LineBreaks {
        self.reader.decoding.line_breaks()
    }

    /// 按文件顺序报告行首在 `[from, to)` 里的那些行中的匹配。`from` 是行首，`to` 是行首或
    /// 文件长度。
    pub(crate) fn scan(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
        visit: &mut dyn FnMut(ScanEvent) -> ControlFlow<()>,
    ) -> io::Result<ScanEnd> {
        let breaks = self.breaks();
        let to = to.min(self.reader.len);
        let mut position = from;
        let mut block = vec![0; SCAN_BLOCK_BYTES];
        while position < to {
            if cancel.is_cancelled() {
                return Ok(ScanEnd::Cancelled);
            }
            let wanted = (self.reader.len - position).min(SCAN_BLOCK_BYTES as u64) as usize;
            let read = read_full(self.reader.source, position, &mut block[..wanted])?;
            let complete = breaks.complete_len(read);
            if complete == 0 {
                break;
            }
            // `to` 是行首或文件尾：它之前的行在块里都完整。
            let chunk_end = if position + complete as u64 >= to {
                Some((to - position) as usize)
            } else {
                breaks.last_line_end(&block[..complete])
            };
            let (next_line, ends_with_break) = match chunk_end {
                Some(chunk_end) => {
                    let chunk = &block[..chunk_end];
                    if self.scan_chunk(position, chunk, visit).is_break() {
                        return Ok(ScanEnd::Stopped);
                    }
                    (
                        position + chunk_end as u64,
                        breaks.last_line_end(chunk) == Some(chunk_end),
                    )
                }
                None => match self.scan_long_line(position, cancel, visit)? {
                    LongLineEnd::NextLine {
                        next_line,
                        ends_with_break,
                    } => (next_line, ends_with_break),
                    LongLineEnd::Stopped => return Ok(ScanEnd::Stopped),
                    LongLineEnd::Cancelled => return Ok(ScanEnd::Cancelled),
                },
            };
            position = next_line;
            // 文件尾没有换行的最后一行之后不是行首（追加后它还会变长），不报。
            if ends_with_break && visit(ScanEvent::LinesScanned { next_line }).is_break() {
                return Ok(ScanEnd::Stopped);
            }
        }
        Ok(ScanEnd::Completed)
    }

    /// 一块完整的行：整块解码、一次匹配，匹配按换行对回各行。
    fn scan_chunk(
        &self,
        chunk_start: u64,
        raw: &[u8],
        visit: &mut dyn FnMut(ScanEvent) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        let text = self.reader.decoding.decode(raw);
        let mut lines = LineWalker::new(self.breaks());
        for range in self.matcher.find_iter(&text) {
            lines.advance_to(&text, raw, range.start);
            visit(ScanEvent::Found(FoundMatch {
                line_start: chunk_start + lines.raw_start as u64,
                place: FoundPlace::InLine {
                    columns: range.start - lines.decoded_start..range.end - lines.decoded_start,
                    may_be_segmented: lines.content_len(raw) as u64 > SEGMENT_BYTES,
                },
            }))?;
        }
        ControlFlow::Continue(())
    }

    /// 超过一块还没有换行的行：按显示段每批约一块读，上一批末尾留 `max_match_bytes`
    /// 字节和这一批拼起来再匹配；整段落在重叠里的匹配上一批已经报过。
    fn scan_long_line(
        &self,
        line_start: u64,
        cancel: &CancellationToken,
        visit: &mut dyn FnMut(ScanEvent) -> ControlFlow<()>,
    ) -> io::Result<LongLineEnd> {
        let mut next = RowStart {
            start: line_start,
            line_start,
        };
        let mut piece = String::new();
        let mut anchors: Vec<TextAnchor> = Vec::new();
        loop {
            if cancel.is_cancelled() {
                return Ok(LongLineEnd::Cancelled);
            }
            let carried = piece.len();
            let rows = self.reader.rows_after(
                next,
                None,
                RowBudget {
                    rows: usize::MAX,
                    bytes: SCAN_BLOCK_BYTES as u64,
                },
            )?;
            let mut line_end = None;
            for row in rows.iter().take_while(|row| row.line_start == line_start) {
                anchors.push(TextAnchor {
                    text_start: piece.len(),
                    row: row.start,
                    column: 0,
                });
                piece.push_str(&row.text);
                next = row.next_row_start();
                if row.ends_line || row.end >= self.reader.len {
                    line_end = Some((row.end, row.ends_line));
                    break;
                }
            }
            // 读不到更多（文件被截短）：这一行到此为止。
            let line_end = line_end.or_else(|| rows.is_empty().then_some((next.start, false)));
            for range in self.matcher.find_iter(&piece) {
                if range.end <= carried {
                    continue;
                }
                let found = TextMatch {
                    line_start,
                    start: anchored_position(&anchors, range.start),
                    end: anchored_position(&anchors, range.end),
                };
                if visit(ScanEvent::Found(FoundMatch {
                    line_start,
                    place: FoundPlace::Located(found),
                }))
                .is_break()
                {
                    return Ok(LongLineEnd::Stopped);
                }
            }
            if let Some((next_line, ends_with_break)) = line_end {
                return Ok(LongLineEnd::NextLine {
                    next_line,
                    ends_with_break,
                });
            }
            let cut =
                piece.floor_char_boundary(piece.len().saturating_sub(self.matcher.max_match_bytes));
            let first = anchors.partition_point(|anchor| anchor.text_start <= cut) - 1;
            let head = anchors[first];
            anchors.drain(..first);
            anchors[0] = TextAnchor {
                text_start: cut,
                column: head.column + (cut - head.text_start),
                ..head
            };
            for anchor in &mut anchors {
                anchor.text_start -= cut;
            }
            piece.drain(..cut);
        }
    }

    /// 匹配的显示位置：没切段的行直接用行首；可能切了段的行从行首往后数各段的文字。
    pub(crate) fn locate(&self, found: &FoundMatch) -> io::Result<TextMatch> {
        let (columns, may_be_segmented) = match &found.place {
            FoundPlace::Located(located) => return Ok(*located),
            FoundPlace::InLine {
                columns,
                may_be_segmented,
            } => (columns.clone(), *may_be_segmented),
        };
        let line_start = found.line_start;
        if !may_be_segmented {
            let at = |column| TextPosition {
                row: line_start,
                column,
            };
            return Ok(TextMatch {
                line_start,
                start: at(columns.start),
                end: at(columns.end),
            });
        }
        let rows = self.reader.rows_after(
            RowStart {
                start: line_start,
                line_start,
            },
            None,
            RowBudget {
                rows: usize::MAX,
                bytes: SCAN_BLOCK_BYTES as u64,
            },
        )?;
        let mut anchors = Vec::new();
        let mut text_start = 0;
        for row in rows.iter().take_while(|row| row.line_start == line_start) {
            anchors.push(TextAnchor {
                text_start,
                row: row.start,
                column: 0,
            });
            text_start += row.text.len();
        }
        if anchors.is_empty() {
            anchors.push(TextAnchor {
                text_start: 0,
                row: line_start,
                column: 0,
            });
        }
        Ok(TextMatch {
            line_start,
            start: anchored_position(&anchors, columns.start),
            end: anchored_position(&anchors, columns.end),
        })
    }
}

enum LongLineEnd {
    NextLine {
        next_line: u64,
        ends_with_break: bool,
    },
    Stopped,
    Cancelled,
}

/// 在整块解码的文字里跟着匹配往后走：解码文字里第 k 个 `\n` 对应原始字节里第 k 个换行。
struct LineWalker {
    breaks: LineBreaks,
    decoded_start: usize,
    raw_start: usize,
    /// 当前行之后那一行在解码文字里的起点（算一次留着，一行里有很多匹配时不重复找）。
    decoded_next: Option<usize>,
    content_len: Option<usize>,
}

impl LineWalker {
    fn new(breaks: LineBreaks) -> Self {
        Self {
            breaks,
            decoded_start: 0,
            raw_start: 0,
            decoded_next: None,
            content_len: None,
        }
    }

    fn advance_to(&mut self, text: &str, raw: &[u8], index: usize) {
        loop {
            let decoded_start = self.decoded_start;
            let next = *self.decoded_next.get_or_insert_with(|| {
                memchr::memchr(b'\n', &text.as_bytes()[decoded_start..])
                    .map_or(usize::MAX, |newline| decoded_start + newline + 1)
            });
            if next > index {
                return;
            }
            let Some(line_len) = self.breaks.next_line_end(&raw[self.raw_start..]) else {
                // 解码出的换行比原始字节里的多（坏数据）：停在最后一行。
                self.decoded_next = Some(usize::MAX);
                return;
            };
            self.decoded_start = next;
            self.raw_start += line_len;
            self.decoded_next = None;
            self.content_len = None;
        }
    }

    /// 当前行（原始字节）不含换行的长度。
    fn content_len(&mut self, raw: &[u8]) -> usize {
        let breaks = self.breaks;
        let raw_start = self.raw_start;
        *self.content_len.get_or_insert_with(|| {
            let rest = &raw[raw_start..];
            let line_len = breaks.next_line_end(rest).unwrap_or(rest.len());
            line_len - breaks.terminator_len(&rest[..line_len])
        })
    }
}

/// 计数的检查点：`offset`（一个行首）之前有 `matches_before` 处匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchCheckpoint {
    pub(crate) offset: u64,
    pub(crate) matches_before: u64,
}

/// 计数进度：新增的检查点、数到哪、到那为止的总数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CountProgress {
    pub(crate) checkpoints: Vec<SearchCheckpoint>,
    pub(crate) scanned_until: u64,
    pub(crate) total: u64,
    pub(crate) finished: bool,
}

/// 从检查点 `start` 往后数到 `len`，在线程池里跑，进度经流送回。
pub(crate) fn count_matches(
    source: Arc<dyn ByteSource>,
    decoding: TextDecoding,
    len: u64,
    matcher: SearchMatcher,
    start: SearchCheckpoint,
    cancel: CancellationToken,
) -> impl Stream<Item = CountProgress> {
    let (sender, receiver) = mpsc::unbounded();
    tokio::task::spawn_blocking(move || {
        let scanner = MatchScanner {
            reader: RowReader {
                source: source.as_ref(),
                decoding,
                len,
            },
            matcher: &matcher,
        };
        count_from(
            &scanner,
            start,
            &cancel,
            &mut |progress| match sender.unbounded_send(progress) {
                Ok(()) => ControlFlow::Continue(()),
                Err(_) => ControlFlow::Break(()),
            },
        );
    });
    receiver
}

/// 从检查点 `start` 往后数，按节流报进度、每个行首块边界记检查点，扫完（或读出错）再报
/// 一次 `finished`；`cancel` 触发或 `report` 叫停就不再报。
pub(crate) fn count_from(
    scanner: &MatchScanner<'_>,
    start: SearchCheckpoint,
    cancel: &CancellationToken,
    report: &mut dyn FnMut(CountProgress) -> ControlFlow<()>,
) {
    let len = scanner.reader.len;
    let mut total = start.matches_before;
    let mut pending = Vec::new();
    let mut reported_at = Instant::now();
    let outcome = scanner.scan(start.offset, len, cancel, &mut |event| {
        match event {
            ScanEvent::Found(_) => total += 1,
            ScanEvent::LinesScanned { next_line } => {
                pending.push(SearchCheckpoint {
                    offset: next_line,
                    matches_before: total,
                });
                if reported_at.elapsed() >= PROGRESS_INTERVAL {
                    reported_at = Instant::now();
                    report(CountProgress {
                        checkpoints: std::mem::take(&mut pending),
                        scanned_until: next_line,
                        total,
                        finished: false,
                    })?;
                }
            }
        }
        ControlFlow::Continue(())
    });
    if matches!(outcome, Ok(ScanEnd::Completed) | Err(_)) {
        let _ = report(CountProgress {
            checkpoints: pending,
            scanned_until: len,
            total,
            finished: true,
        });
    }
}

/// 往哪个方向找。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// 从 `from`（在行首为 `line_start` 的那一行里）找：往后是起点不早于它的第一个匹配，往前是
/// 起点早于它的最后一个；到头绕回另一端。整篇都没有时 None。
pub(crate) fn find_match(
    scanner: &MatchScanner<'_>,
    line_start: u64,
    from: TextPosition,
    direction: SearchDirection,
    cancel: &CancellationToken,
) -> io::Result<Option<TextMatch>> {
    match direction {
        SearchDirection::Forward => find_forward(scanner, line_start, from, cancel),
        SearchDirection::Backward => find_backward(scanner, line_start, from, cancel),
    }
}

fn find_forward(
    scanner: &MatchScanner<'_>,
    line_start: u64,
    from: TextPosition,
    cancel: &CancellationToken,
) -> io::Result<Option<TextMatch>> {
    let mut found = None;
    let mut failure = None;
    scanner.scan(line_start, scanner.reader.len, cancel, &mut |event| {
        let ScanEvent::Found(candidate) = event else {
            return ControlFlow::Continue(());
        };
        match scanner.locate(&candidate) {
            Ok(located) if located.line_start > line_start || located.start >= from => {
                found = Some(located);
                ControlFlow::Break(())
            }
            Ok(_) => ControlFlow::Continue(()),
            Err(error) => {
                failure = Some(error);
                ControlFlow::Break(())
            }
        }
    })?;
    if let Some(error) = failure {
        return Err(error);
    }
    if found.is_some() || cancel.is_cancelled() {
        return Ok(found);
    }
    // 绕回开头：第一个匹配就是答案（在 `from` 之后的都已经找过了）。
    let data_start = scanner.reader.decoding.data_start();
    scanner.scan(
        data_start,
        scanner.reader.len,
        cancel,
        &mut |event| match event {
            ScanEvent::Found(candidate) => {
                match scanner.locate(&candidate) {
                    Ok(located) => found = Some(located),
                    Err(error) => failure = Some(error),
                }
                ControlFlow::Break(())
            }
            ScanEvent::LinesScanned { next_line } if next_line > line_start => {
                ControlFlow::Break(())
            }
            ScanEvent::LinesScanned { .. } => ControlFlow::Continue(()),
        },
    )?;
    failure.map_or(Ok(found), Err)
}

fn find_backward(
    scanner: &MatchScanner<'_>,
    line_start: u64,
    from: TextPosition,
    cancel: &CancellationToken,
) -> io::Result<Option<TextMatch>> {
    // 先看 `from` 所在这一行里它前面的。
    let mut found = None;
    let mut failure = None;
    scanner.scan(
        line_start,
        scanner.reader.len,
        cancel,
        &mut |event| match event {
            ScanEvent::Found(candidate) => match scanner.locate(&candidate) {
                Ok(located) if located.line_start == line_start && located.start < from => {
                    found = Some(located);
                    ControlFlow::Continue(())
                }
                Ok(_) => ControlFlow::Break(()),
                Err(error) => {
                    failure = Some(error);
                    ControlFlow::Break(())
                }
            },
            ScanEvent::LinesScanned { .. } => ControlFlow::Break(()),
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if found.is_some() {
        return Ok(found);
    }
    let data_start = scanner.reader.decoding.data_start();
    if let Some(last) = last_match_before(scanner, line_start, data_start, cancel)? {
        return Ok(Some(last));
    }
    // 绕回末尾：整篇最后一个匹配（`from` 之前的都已经找过了）。
    last_match_before(scanner, scanner.reader.len, line_start, cancel)
}

/// 行首在 `[floor, high)` 里的行中的最后一个匹配：从 `high` 往前一块一块地找。
fn last_match_before(
    scanner: &MatchScanner<'_>,
    mut high: u64,
    floor: u64,
    cancel: &CancellationToken,
) -> io::Result<Option<TextMatch>> {
    while high > floor && !cancel.is_cancelled() {
        let probe = high.saturating_sub(SCAN_BLOCK_BYTES as u64).max(floor);
        let known = RowStart {
            start: floor,
            line_start: floor,
        };
        let low = scanner.reader.line_start_of(probe, Some(known))?;
        let mut last = None;
        scanner.scan(low, high, cancel, &mut |event| {
            if let ScanEvent::Found(candidate) = event {
                last = Some(candidate);
            }
            ControlFlow::Continue(())
        })?;
        if let Some(last) = last {
            return scanner.locate(&last).map(Some);
        }
        high = low;
    }
    Ok(None)
}

/// 匹配 `at` 是第几个（从 1 数）：从它前面的检查点 `from` 往后数到它。
pub(crate) fn match_ordinal(
    scanner: &MatchScanner<'_>,
    at: TextMatch,
    from: SearchCheckpoint,
    cancel: &CancellationToken,
) -> io::Result<u64> {
    let mut before = from.matches_before;
    let mut failure = None;
    scanner.scan(
        from.offset,
        scanner.reader.len,
        cancel,
        &mut |event| match event {
            ScanEvent::Found(candidate) if candidate.line_start < at.line_start => {
                before += 1;
                ControlFlow::Continue(())
            }
            ScanEvent::Found(candidate) => match scanner.locate(&candidate) {
                Ok(located) if located.line_start == at.line_start && located.start < at.start => {
                    before += 1;
                    ControlFlow::Continue(())
                }
                Ok(_) => ControlFlow::Break(()),
                Err(error) => {
                    failure = Some(error);
                    ControlFlow::Break(())
                }
            },
            ScanEvent::LinesScanned { next_line } if next_line > at.line_start => {
                ControlFlow::Break(())
            }
            ScanEvent::LinesScanned { .. } => ControlFlow::Continue(()),
        },
    )?;
    failure.map_or(Ok(before + 1), Err)
}

#[cfg(test)]
#[path = "search_scan_tests.rs"]
mod tests;
