//! 终端屏幕：alacritty 的 [`Term`]。读线程解析输出时写它，画面画的时候读它，中间隔一把公平锁
//! （读线程不会把画面饿着）。

use std::ops::DerefMut;
use std::sync::Arc;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};

use super::session::EventProxy;

/// 终端回看缓冲行数。
pub(crate) const SCROLLBACK_LINES: usize = 10_000;

/// 读线程和画面共用的屏幕。
pub(crate) type SharedTerm = Arc<FairMutex<Term<EventProxy>>>;

/// alacritty [`Term`] 只认 `Dimensions` 实现;`total_lines` 由 scrollback 常量决定。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalDimensions {
    pub(crate) columns: usize,
    pub(crate) screen_lines: usize,
}

impl Dimensions for TerminalDimensions {
    fn columns(&self) -> usize {
        self.columns
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn total_lines(&self) -> usize {
        self.screen_lines + SCROLLBACK_LINES
    }
}

pub(crate) struct TerminalEmulator {
    term: SharedTerm,
}

impl TerminalEmulator {
    pub(crate) fn new(dimensions: TerminalDimensions, events: EventProxy) -> Self {
        let config = TermConfig {
            scrolling_history: SCROLLBACK_LINES,
            ..TermConfig::default()
        };
        Self {
            term: Arc::new(FairMutex::new(Term::new(config, &dimensions, events))),
        }
    }

    pub(crate) fn shared(&self) -> SharedTerm {
        Arc::clone(&self.term)
    }

    pub(crate) fn lock(&self) -> impl DerefMut<Target = Term<EventProxy>> + '_ {
        self.term.lock()
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.lock().mode()
    }

    pub(crate) fn dimensions(&self) -> TerminalDimensions {
        let term = self.lock();
        TerminalDimensions {
            columns: term.columns(),
            screen_lines: term.screen_lines(),
        }
    }

    pub(crate) fn resize(&self, dimensions: TerminalDimensions) {
        self.lock().resize(dimensions);
    }

    pub(crate) fn scroll(&self, scroll: Scroll) {
        self.lock().scroll_display(scroll);
    }

    pub(crate) fn scroll_to_bottom(&self) {
        self.scroll(Scroll::Bottom);
    }

    /// 全选：从回看最上一行到最下一行按行选，返回选中的字（进 PRIMARY）。
    pub(crate) fn select_all(&self) -> Option<String> {
        let mut term = self.lock();
        let first = Point::new(term.topmost_line(), Column(0));
        let last = Point::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Lines, first, Side::Left);
        selection.update(last, Side::Right);
        term.selection = Some(selection);
        term.selection_to_string()
    }

    pub(crate) fn clear_history(&self) {
        let mut term = self.lock();
        term.grid_mut().clear_history();
        term.selection = None;
    }

    pub(crate) fn clear_selection(&self) {
        self.lock().selection = None;
    }

    pub(crate) fn selection_string(&self) -> Option<String> {
        self.lock().selection_to_string()
    }

    /// 程序打开了全屏界面，要用户来操作：切到了备用屏幕，或者打开了方向键的程序模式（DECCKM）。
    /// top 不切备用屏幕，只开方向键模式；普通命令两样都不碰。
    pub(crate) fn is_full_screen(&self) -> bool {
        self.mode()
            .intersects(TermMode::ALT_SCREEN | TermMode::APP_CURSOR)
    }

    /// 最后 `count` 行字（连同回看），行尾空白去掉，后面空着的行不算。
    pub(crate) fn last_lines(&self, count: usize) -> String {
        let term = self.lock();
        let bottom = term.bottommost_line();
        // 屏幕下半截可能还空着：多取一屏，去掉空行后再数。
        let reach = (count + term.screen_lines()) as i32;
        let top = Line((bottom.0 - reach + 1).max(term.topmost_line().0));
        let text = term.bounds_to_string(
            Point::new(top, Column(0)),
            Point::new(bottom, term.last_column()),
        );
        let lines: Vec<&str> = text.trim_end().lines().map(str::trim_end).collect();
        lines[lines.len().saturating_sub(count)..].join("\n")
    }
}

/// 屏幕上第 `row` 行第 `column` 列在整个网格（连同回看）里的位置；回看滚上去时也对。
pub(crate) fn grid_point(term: &Term<EventProxy>, row: usize, column: usize) -> Point {
    Point::new(
        Line(row as i32 - term.grid().display_offset() as i32),
        Column(column.min(term.columns().saturating_sub(1))),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use alacritty_terminal::index::Side;
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    /// 不接伪终端的屏幕：测试直接喂字节。
    pub(crate) fn emulator(columns: usize, rows: usize) -> TerminalEmulator {
        TerminalEmulator::new(
            TerminalDimensions {
                columns,
                screen_lines: rows,
            },
            EventProxy::detached(),
        )
    }

    pub(crate) fn feed(emulator: &TerminalEmulator, bytes: &[u8]) {
        let mut processor: Processor = Processor::new();
        processor.advance(&mut *emulator.lock(), bytes);
    }

    #[test]
    fn resize_keeps_visible_content() {
        let emulator = emulator(10, 3);
        feed(&emulator, b"abc");
        emulator.resize(TerminalDimensions {
            columns: 20,
            screen_lines: 6,
        });
        assert_eq!(emulator.last_lines(1), "abc");
    }

    #[test]
    fn selection_over_rendered_text_extracts_it() {
        let emulator = emulator(20, 3);
        feed(&emulator, b"hello world");
        {
            let mut term = emulator.lock();
            let start = grid_point(&term, 0, 0);
            let end = grid_point(&term, 0, 4);
            let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
            selection.update(end, Side::Right);
            term.selection = Some(selection);
        }
        assert_eq!(emulator.selection_string().as_deref(), Some("hello"));
    }

    #[test]
    fn the_last_lines_come_from_the_history_too_and_skip_the_empty_screen_below() {
        let emulator = emulator(6, 3);
        // 换行时折到下一行的不算两行；进度条用回车改写的只留最后的样子。
        feed(&emulator, b"one\r\ntwo\r\n10%\r100%  \r\nlong line\r\n");
        assert_eq!(emulator.last_lines(3), "two\n100%\nlong line");
        assert_eq!(emulator.last_lines(10), "one\ntwo\n100%\nlong line");
        assert_eq!(emulator.last_lines(1), "long line");
    }

    #[test]
    fn a_program_that_takes_over_the_screen_or_the_arrow_keys_is_full_screen() {
        let emulator = emulator(10, 4);
        feed(&emulator, b"plain output\r\n");
        assert!(!emulator.is_full_screen());
        feed(&emulator, b"\x1b[?1049h");
        assert!(emulator.is_full_screen());
        feed(&emulator, b"\x1b[?1049l");
        assert!(!emulator.is_full_screen());
        // top 只发这个。
        feed(&emulator, b"\x1b[?1h");
        assert!(emulator.is_full_screen());
    }
}
