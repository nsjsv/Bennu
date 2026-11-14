//! 文档里的搜索与跳行：文字上方的条（搜索或跳行）、窗口里的匹配（界面线程按窗口算，窗口
//! 最多 4 MB）、当前匹配与它的序号、后台计数与查找。
//!
//! 搜索条跟着文档走：换文件就没了；同一文件改写重开、改编码时引擎把条和搜索词搬过去。
//! Markdown 渲染模式下搜的是渲染后的文字（`rendered_find`），原文模式搜文件。

use std::collections::VecDeque;

use tokio_util::sync::CancellationToken;

use super::open::{JumpPlace, TextJump};
use super::rendered_find::{RenderedMatch, RenderedSearch};
use super::rows::{TextPosition, TextRow};
use super::search_scan::{
    anchored_position, CountProgress, SearchCheckpoint, SearchDirection, SearchMatcher,
    SearchQuery, TextAnchor, TextMatch,
};
use super::{LineCountStatus, TextPreviewDocument, TextRead, ViewerJump};

/// 文字上方的条：搜索或跳行，同一时间只有一个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindBar {
    Search,
    GotoLine { input: String },
}

/// 搜索条上的计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    /// 当前匹配是第几个；没有当前匹配或还没数到它时为 None。
    pub ordinal: Option<u64>,
    /// 到目前为止数到的匹配数。
    pub total: u64,
    /// 整篇数完了。
    pub finished: bool,
}

/// 窗口里加了哪些行（只重算新行附近的匹配）。
pub(crate) enum WindowChange {
    Replaced,
    Appended { first_new: usize },
    Prepended { added: usize },
}

#[derive(Default)]
pub(crate) struct TextFind {
    bar: Option<FindBar>,
    query: SearchQuery,
    /// 搜索条开着、搜索词非空时的这一轮搜索：原文模式是 `run`（搜文件），渲染模式是
    /// `rendered`（搜渲染后的文字），同一时间至多一个，都在 `stop_search` 里清掉。
    run: Option<SearchRun>,
    rendered: Option<RenderedSearch>,
    /// 每开一轮搜索加一；旧一轮的回复到达时丢弃。
    generation: u64,
    /// 正在后台找这一行（从 0 数）的行首。
    goto_in_flight: Option<u64>,
}

struct SearchRun {
    generation: u64,
    matcher: SearchMatcher,
    window_matches: Vec<TextMatch>,
    current: Option<TextMatch>,
    ordinal: Ordinal,
    count: MatchCount,
    /// 正在后台找下一个/上一个；新的查找取消旧的。
    find_cancel: Option<CancellationToken>,
}

impl Drop for SearchRun {
    fn drop(&mut self) {
        self.count.cancel.cancel();
        if let Some(cancel) = &self.find_cancel {
            cancel.cancel();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ordinal {
    Unknown,
    Requested,
    Known(u64),
}

struct MatchCount {
    checkpoints: Vec<SearchCheckpoint>,
    scanned_until: u64,
    total: u64,
    finished: bool,
    cancel: CancellationToken,
}

impl TextFind {
    /// 改写重开、改编码时搬到新文档：条和搜索词留着，结果重新搜。
    pub(crate) fn carried_over(&self) -> Self {
        Self {
            bar: self.bar.clone(),
            query: self.query.clone(),
            run: None,
            rendered: None,
            generation: self.generation,
            goto_in_flight: None,
        }
    }

    pub(crate) fn retain_window(&mut self, first: u64, last: u64) {
        if let Some(run) = &mut self.run {
            run.window_matches
                .retain(|found| (first..last).contains(&found.start.row));
        }
    }
}

impl TextPreviewDocument {
    pub fn find_bar(&self) -> Option<&FindBar> {
        self.find.bar.as_ref()
    }

    pub fn search_query(&self) -> &SearchQuery {
        &self.find.query
    }

    /// 窗口里的匹配，按位置排好。
    pub fn search_matches(&self) -> &[TextMatch] {
        self.find
            .run
            .as_ref()
            .map_or(&[], |run| run.window_matches.as_slice())
    }

    pub fn current_match(&self) -> Option<TextMatch> {
        self.find.run.as_ref().and_then(|run| run.current)
    }

    /// 渲染模式的匹配与当前匹配的下标。
    pub fn rendered_search(&self) -> Option<(&[RenderedMatch], Option<usize>)> {
        let rendered = self.find.rendered.as_ref()?;
        Some((rendered.matches(), rendered.current()))
    }

    /// 渲染模式当前匹配所在的块（引擎据此把那一块滚进视口）。
    pub(crate) fn rendered_current_block(&self) -> Option<usize> {
        let rendered = self.find.rendered.as_ref()?;
        let found = &rendered.matches()[rendered.current()?];
        self.markdown()?.block_of_unit(found.unit)
    }

    /// 搜索条上的计数；没有在搜时 None。渲染模式一次就数完。
    pub fn search_summary(&self) -> Option<SearchSummary> {
        if let Some(rendered) = &self.find.rendered {
            return Some(SearchSummary {
                ordinal: rendered.current().map(|index| index as u64 + 1),
                total: rendered.matches().len() as u64,
                finished: true,
            });
        }
        let run = self.find.run.as_ref()?;
        Some(SearchSummary {
            ordinal: match (run.current, run.ordinal) {
                (Some(_), Ordinal::Known(ordinal)) => Some(ordinal),
                _ => None,
            },
            total: run.count.total,
            finished: run.count.finished,
        })
    }

    pub fn goto_in_flight(&self) -> bool {
        self.find.goto_in_flight.is_some()
    }

    /// 改写重开、改编码前取出条和搜索词，新文档用 `restore_find` 接上。
    pub(crate) fn carried_find(&self) -> TextFind {
        self.find.carried_over()
    }

    pub(crate) fn restore_find(&mut self, find: TextFind) -> Vec<TextRead> {
        self.find = find;
        self.restart_search()
    }

    /// Ctrl+F：打开搜索条（`seed` 是选中的一小段文字，用来填搜索词）。
    pub(crate) fn open_search(&mut self, seed: Option<String>) -> Vec<TextRead> {
        let reopened = self.find.bar != Some(FindBar::Search);
        self.find.bar = Some(FindBar::Search);
        self.find.goto_in_flight = None;
        match seed {
            Some(seed) if seed != self.find.query.text => {
                self.find.query.text = seed;
                self.restart_search()
            }
            _ if reopened => self.restart_search(),
            _ => Vec::new(),
        }
    }

    /// Ctrl+G：打开跳行条（搜索停下）。跳行只对原文有意义，渲染模式里不开。
    pub(crate) fn open_goto(&mut self) {
        if self.shows_rendered_markdown() {
            return;
        }
        self.stop_search();
        self.find.bar = Some(FindBar::GotoLine {
            input: String::new(),
        });
        self.revision += 1;
    }

    pub(crate) fn close_find(&mut self) {
        self.stop_search();
        self.find.bar = None;
        self.find.goto_in_flight = None;
        self.revision += 1;
    }

    pub(crate) fn set_search_text(&mut self, text: String) -> Vec<TextRead> {
        if text == self.find.query.text {
            return Vec::new();
        }
        self.find.query.text = text;
        self.restart_search()
    }

    pub(crate) fn toggle_search_case(&mut self) -> Vec<TextRead> {
        self.find.query.case_sensitive = !self.find.query.case_sensitive;
        self.restart_search()
    }

    /// 下一个/上一个。条没开时先打开（用上次的搜索词，从视口顶部找起）。
    pub(crate) fn step_search(&mut self, direction: SearchDirection) -> Vec<TextRead> {
        if self.find.bar != Some(FindBar::Search) {
            return self.open_search(None);
        }
        if let Some(rendered) = &mut self.find.rendered {
            rendered.step(direction);
            self.revision += 1;
            return Vec::new();
        }
        let Some(run) = &self.find.run else {
            return Vec::new();
        };
        let Some(current) = run.current else {
            return self.find_from_viewport();
        };
        let from = match direction {
            // 「起点不早于」的下一个要严格在当前匹配之后。
            SearchDirection::Forward => TextPosition {
                column: current.start.column + 1,
                ..current.start
            },
            SearchDirection::Backward => current.start,
        };
        self.find_from(current.line_start, from, direction)
    }

    pub(crate) fn set_goto_input(&mut self, input: String) {
        let digits: String = input.chars().filter(char::is_ascii_digit).collect();
        if let Some(FindBar::GotoLine { input }) = &mut self.find.bar {
            *input = digits;
            self.revision += 1;
        }
    }

    /// 跳行：行号从 1 数，超出范围夹到首/末行；行数还没统计完也能跳。
    pub(crate) fn submit_goto(&mut self) -> Vec<TextRead> {
        let Some(FindBar::GotoLine { input }) = &self.find.bar else {
            return Vec::new();
        };
        let Ok(number) = input.parse::<u64>() else {
            return Vec::new();
        };
        let mut line = number.saturating_sub(1);
        if let LineCountStatus::Done { total } = self.line_count_status() {
            line = line.min(total.saturating_sub(1));
        }
        self.find.goto_in_flight = Some(line);
        self.revision += 1;
        vec![TextRead::LineStart {
            line,
            seek: self.index.seek_line(line),
        }]
    }

    pub(crate) fn accept_line_start(
        &mut self,
        line: u64,
        outcome: Result<u64, String>,
    ) -> Vec<TextRead> {
        if self.find.goto_in_flight != Some(line) {
            return Vec::new();
        }
        self.find.goto_in_flight = None;
        let Ok(line_start) = outcome else {
            self.revision += 1;
            return Vec::new();
        };
        self.find.bar = None;
        self.reveal(TextPosition {
            row: line_start,
            column: 0,
        })
    }

    pub(crate) fn accept_search_counted(
        &mut self,
        generation: u64,
        progress: CountProgress,
    ) -> Vec<TextRead> {
        let len = self.stamp.len;
        let Some(run) = self
            .find
            .run
            .as_mut()
            .filter(|run| run.generation == generation)
        else {
            return Vec::new();
        };
        run.count.checkpoints.extend(progress.checkpoints);
        run.count.scanned_until = progress.scanned_until;
        run.count.total = progress.total;
        self.revision += 1;
        let mut reads: Vec<TextRead> = self.ordinal_read().into_iter().collect();
        if progress.finished {
            let run = self.find.run.as_mut().expect("the run accepted progress");
            run.count.finished = true;
            // 数的途中文件又变长了：从最后一个检查点接着数。
            if len > progress.scanned_until {
                reads.extend(self.continue_count());
            }
        }
        reads
    }

    pub(crate) fn accept_search_found(
        &mut self,
        generation: u64,
        outcome: Result<Option<TextMatch>, String>,
    ) -> Vec<TextRead> {
        let Some(run) = self
            .find
            .run
            .as_mut()
            .filter(|run| run.generation == generation)
        else {
            return Vec::new();
        };
        run.find_cancel = None;
        match outcome {
            Ok(Some(found)) => self.make_current(found),
            // 整篇都没有：计数会数出 0，条上显示「无结果」。
            Ok(None) | Err(_) => Vec::new(),
        }
    }

    pub(crate) fn accept_search_ordinal(
        &mut self,
        generation: u64,
        at: TextMatch,
        outcome: Result<u64, String>,
    ) {
        let Some(run) = self
            .find
            .run
            .as_mut()
            .filter(|run| run.generation == generation && run.current == Some(at))
        else {
            return;
        };
        run.ordinal = match outcome {
            Ok(ordinal) => Ordinal::Known(ordinal),
            Err(_) => Ordinal::Unknown,
        };
        self.revision += 1;
    }

    /// 文件追加了：数完了的计数从最后一个检查点接着数（正在数的数完后自己接上）。
    pub(crate) fn search_after_append(&mut self) -> Vec<TextRead> {
        match &self.find.run {
            Some(run) if run.count.finished => self.continue_count().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// 窗口加了行：只重算新行所在的原始行里的匹配。
    pub(crate) fn refresh_window_matches(&mut self, change: WindowChange) {
        let Some(run) = &mut self.find.run else {
            return;
        };
        let rows = &self.rows;
        match change {
            WindowChange::Replaced => {
                run.window_matches = matches_in_rows(rows, 0..rows.len(), &run.matcher);
            }
            WindowChange::Appended { first_new } => {
                let from = line_run_start(rows, first_new);
                let Some(boundary) = rows.get(from).map(|row| row.start) else {
                    return;
                };
                run.window_matches
                    .retain(|found| found.start.row < boundary);
                run.window_matches
                    .extend(matches_in_rows(rows, from..rows.len(), &run.matcher));
            }
            WindowChange::Prepended { added } => {
                let to = line_run_end(rows, added);
                let boundary = rows.get(to).map_or(u64::MAX, |row| row.start);
                let mut matches = matches_in_rows(rows, 0..to, &run.matcher);
                matches.extend(
                    run.window_matches
                        .drain(..)
                        .filter(|found| found.start.row >= boundary),
                );
                run.window_matches = matches;
            }
        }
        self.revision += 1;
    }

    /// Markdown 换了模式：搜索按新模式重算；跳行只对原文有意义，切到渲染时关掉跳行条。
    pub(crate) fn search_after_mode_change(&mut self) -> Vec<TextRead> {
        if self.shows_rendered_markdown() && matches!(self.find.bar, Some(FindBar::GotoLine { .. }))
        {
            self.find.bar = None;
            self.find.goto_in_flight = None;
        }
        self.restart_search()
    }

    fn stop_search(&mut self) {
        self.find.run = None;
        self.find.rendered = None;
        self.revision += 1;
    }

    fn restart_search(&mut self) -> Vec<TextRead> {
        self.stop_search();
        if self.find.bar != Some(FindBar::Search) {
            return Vec::new();
        }
        let Some(matcher) = SearchMatcher::new(&self.find.query) else {
            return Vec::new();
        };
        if let Some(markdown) = self
            .markdown
            .as_deref()
            .filter(|_| self.shows_rendered_markdown())
        {
            self.find.rendered = Some(RenderedSearch::new(markdown, &matcher));
            return Vec::new();
        }
        self.find.generation += 1;
        let generation = self.find.generation;
        let start = SearchCheckpoint {
            offset: self.decoding.data_start(),
            matches_before: 0,
        };
        let cancel = CancellationToken::new();
        self.find.run = Some(SearchRun {
            generation,
            window_matches: matches_in_rows(&self.rows, 0..self.rows.len(), &matcher),
            matcher: matcher.clone(),
            current: None,
            ordinal: Ordinal::Unknown,
            count: MatchCount {
                checkpoints: vec![start],
                scanned_until: start.offset,
                total: 0,
                finished: false,
                cancel: cancel.clone(),
            },
            find_cancel: None,
        });
        let mut reads = vec![TextRead::SearchCount {
            generation,
            matcher,
            start,
            cancel,
        }];
        // 打字即搜：从视口顶部往后找第一个。
        reads.extend(self.find_from_viewport());
        reads
    }

    fn find_from_viewport(&mut self) -> Vec<TextRead> {
        let top = self
            .viewport
            .and_then(|viewport| self.row_index_containing(viewport.first_row))
            .or((!self.rows.is_empty()).then_some(0));
        let (line_start, from) = match top.map(|index| &self.rows[index]) {
            Some(row) => (
                row.line_start,
                TextPosition {
                    row: row.start,
                    column: 0,
                },
            ),
            None => {
                let start = self.decoding.data_start();
                (
                    start,
                    TextPosition {
                        row: start,
                        column: 0,
                    },
                )
            }
        };
        self.find_from(line_start, from, SearchDirection::Forward)
    }

    /// 往后：起点不早于 `from` 的第一个；往前：起点早于 `from` 的最后一个。`from` 在窗口里
    /// 时先在窗口里找，整篇都在窗口里时在窗口里绕回；否则交给后台。
    fn find_from(
        &mut self,
        line_start: u64,
        from: TextPosition,
        direction: SearchDirection,
    ) -> Vec<TextRead> {
        let in_window = self.row_index_containing(from.row).is_some();
        let whole_file = self.at_start && self.at_end;
        let Some(run) = &mut self.find.run else {
            return Vec::new();
        };
        if in_window {
            let matches = &run.window_matches;
            let after = matches.partition_point(|found| found.start < from);
            let found = match direction {
                SearchDirection::Forward => matches
                    .get(after)
                    .or(whole_file.then(|| matches.first()).flatten()),
                SearchDirection::Backward => after
                    .checked_sub(1)
                    .map(|index| &matches[index])
                    .or(whole_file.then(|| matches.last()).flatten()),
            };
            if let Some(found) = found.copied() {
                return self.make_current(found);
            }
            if whole_file {
                return Vec::new();
            }
        }
        if let Some(cancel) = run.find_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancellationToken::new();
        run.find_cancel = Some(cancel.clone());
        vec![TextRead::SearchFind {
            generation: run.generation,
            matcher: run.matcher.clone(),
            line_start,
            from,
            direction,
            cancel,
        }]
    }

    fn make_current(&mut self, found: TextMatch) -> Vec<TextRead> {
        let Some(run) = &mut self.find.run else {
            return Vec::new();
        };
        run.current = Some(found);
        run.ordinal = Ordinal::Unknown;
        let mut reads = self.reveal(found.start);
        reads.extend(self.ordinal_read());
        reads
    }

    /// 让查看器把 `position` 显示出来并把光标放到那里；不在窗口里就先读那一段。
    fn reveal(&mut self, position: TextPosition) -> Vec<TextRead> {
        self.revision += 1;
        if self.row_index(position.row).is_ok() {
            self.jump = Some(ViewerJump {
                revision: self.jump.map_or(1, |jump| jump.revision + 1),
                row: position.row,
                place: JumpPlace::Reveal,
                cursor: Some(position),
            });
            return Vec::new();
        }
        self.jump_to(TextJump::Reveal(position))
    }

    /// 计数越过了当前匹配所在的行：从它前面最近的检查点数到它，得出序号。
    fn ordinal_read(&mut self) -> Option<TextRead> {
        let run = self.find.run.as_mut()?;
        let current = run.current?;
        if run.ordinal != Ordinal::Unknown || run.count.scanned_until <= current.line_start {
            return None;
        }
        let checkpoints = &run.count.checkpoints;
        let from = checkpoints[checkpoints
            .partition_point(|checkpoint| checkpoint.offset <= current.line_start)
            .saturating_sub(1)];
        run.ordinal = Ordinal::Requested;
        Some(TextRead::SearchOrdinal {
            generation: run.generation,
            matcher: run.matcher.clone(),
            at: current,
            from,
            cancel: run.count.cancel.clone(),
        })
    }

    fn continue_count(&mut self) -> Option<TextRead> {
        let run = self.find.run.as_mut()?;
        let start = *run.count.checkpoints.last().expect("the first checkpoint");
        run.count.cancel = CancellationToken::new();
        run.count.finished = false;
        run.count.total = start.matches_before;
        run.count.scanned_until = start.offset;
        Some(TextRead::SearchCount {
            generation: run.generation,
            matcher: run.matcher.clone(),
            start,
            cancel: run.count.cancel.clone(),
        })
    }
}

/// `index` 所在原始行在窗口里的第一行。
fn line_run_start(rows: &VecDeque<TextRow>, mut index: usize) -> usize {
    while index > 0 && index < rows.len() && !rows[index].is_line_start() {
        index -= 1;
    }
    index
}

/// `index` 所在原始行在窗口里最后一行之后。
fn line_run_end(rows: &VecDeque<TextRow>, mut index: usize) -> usize {
    while index < rows.len() && !rows[index].is_line_start() {
        index += 1;
    }
    index
}

/// `range` 这些窗口行里的匹配：同一原始行的各段拼起来匹配（跨段的也算）。
fn matches_in_rows(
    rows: &VecDeque<TextRow>,
    range: std::ops::Range<usize>,
    matcher: &SearchMatcher,
) -> Vec<TextMatch> {
    let mut matches = Vec::new();
    let mut index = range.start;
    while index < range.end {
        let line_start = rows[index].line_start;
        let run_end = index
            + rows
                .range(index..range.end)
                .take_while(|row| row.line_start == line_start)
                .count();
        let mut text = String::new();
        let mut anchors = Vec::new();
        let single = (run_end == index + 1).then(|| &rows[index]);
        let haystack = match single {
            Some(row) => {
                anchors.push(TextAnchor {
                    text_start: 0,
                    row: row.start,
                    column: 0,
                });
                row.text.as_str()
            }
            None => {
                for row in rows.range(index..run_end) {
                    anchors.push(TextAnchor {
                        text_start: text.len(),
                        row: row.start,
                        column: 0,
                    });
                    text.push_str(&row.text);
                }
                text.as_str()
            }
        };
        matches.extend(matcher.find_iter(haystack).map(|found| TextMatch {
            line_start,
            start: anchored_position(&anchors, found.start),
            end: anchored_position(&anchors, found.end),
        }));
        index = run_end;
    }
    matches
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
