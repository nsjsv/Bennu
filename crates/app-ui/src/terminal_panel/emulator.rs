use std::io::Write;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, GridIterator, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, RenderableContent, Term};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};

/// 终端回看缓冲行数。
pub(crate) const SCROLLBACK_LINES: usize = 10_000;

/// PTY 写端;终端应答(DA/光标查询等)与用户输入共用。
pub(crate) type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

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

/// Term 事件沉淀点:PTY 回写直接写入写端;其余事件(标题/铃响)第一版忽略。
struct EventProxy {
    writer: PtyWriter,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            if let Ok(mut writer) = self.writer.lock() {
                let _ = writer.write_all(text.as_bytes());
                let _ = writer.flush();
            }
        }
    }
}

/// 单个终端字符格的渲染快照。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalCell {
    pub(crate) character: char,
    pub(crate) fg: TermColor,
    pub(crate) bg: TermColor,
    pub(crate) flags: Flags,
    pub(crate) selected: bool,
}

/// 一帧可渲染的终端内容:可视区按行存放。
pub(crate) struct TerminalFrame {
    pub(crate) lines: Vec<Vec<TerminalCell>>,
    pub(crate) cursor: Option<(usize, usize)>,
    pub(crate) cursor_shape: CursorShape,
}

pub(crate) struct TerminalEmulator {
    term: Term<EventProxy>,
    processor: Processor,
}

impl TerminalEmulator {
    pub(crate) fn new(columns: usize, rows: usize, writer: PtyWriter) -> Self {
        let dimensions = TerminalDimensions {
            columns,
            screen_lines: rows,
        };
        let mut config = TermConfig::default();
        config.scrolling_history = SCROLLBACK_LINES;
        let term = Term::new(config, &dimensions, EventProxy { writer });
        Self {
            term,
            processor: Processor::new(),
        }
    }

    pub(crate) fn dimensions(&self) -> TerminalDimensions {
        TerminalDimensions {
            columns: self.term.columns(),
            screen_lines: self.term.screen_lines(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        if columns == 0 || rows == 0 {
            return;
        }
        let dimensions = TerminalDimensions {
            columns,
            screen_lines: rows,
        };
        self.term.resize(dimensions);
    }

    pub(crate) fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub(crate) fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub(crate) fn begin_selection(&mut self, screen_row: usize, column: usize) {
        self.term.selection = Some(Selection::new(
            SelectionType::Simple,
            self.absolute_point(screen_row, column),
            Side::Left,
        ));
    }

    pub(crate) fn extend_selection(&mut self, screen_row: usize, column: usize) {
        // Side::Right 让 end 单元格包含在选择内(拖到第 4 列 = 选到第 4 列的字符)。
        let point = self.absolute_point(screen_row, column);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, Side::Right);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub(crate) fn selection_string(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    fn absolute_point(&self, screen_row: usize, column: usize) -> Point<Line, Column> {
        Point::new(
            Line(screen_row as i32 - self.display_offset() as i32),
            Column(column.min(self.term.columns().saturating_sub(1))),
        )
    }

    /// 截取当前可视帧。调用方在绘制路径上频繁使用,不做写操作。
    pub(crate) fn renderable_frame(&self) -> TerminalFrame {
        let columns = self.term.columns();
        let rows = self.term.screen_lines();
        let offset = self.display_offset();
        let content: RenderableContent<'_> = self.term.renderable_content();
        let cursor = content.cursor.point;
        let selection_range = content.selection;
        let mut lines: Vec<Vec<TerminalCell>> = vec![Vec::with_capacity(columns); rows];

        // renderable_content 的迭代器从可见区顶部开始;abs_line = point.line + offset
        // 映射回屏幕行,历史行(负)在 offset>0 时同样落在可视范围内。
        let mut iterator: GridIterator<'_, alacritty_terminal::term::cell::Cell> =
            content.display_iter;
        while let Some(indexed) = iterator.next() {
            let screen_row = (indexed.point.line.0 + offset as i32) as usize;
            if screen_row >= rows {
                continue;
            }
            let column = indexed.point.column.0;
            if column >= columns {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            lines[screen_row].push(TerminalCell {
                character: cell.c,
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
                selected: self.cell_is_selected(cell, indexed.point, selection_range),
            });
        }

        // 迭代器跳过空 cell;补齐每行,保证列号与屏幕坐标一一对应。
        for line in &mut lines {
            line.resize(columns, TerminalCell::empty());
        }

        let cursor_screen_row = (cursor.line.0 + offset as i32) as usize;
        let cursor = if cursor_screen_row < rows {
            Some((cursor_screen_row, cursor.column.0))
        } else {
            None
        };

        TerminalFrame {
            lines,
            cursor,
            cursor_shape: content.cursor.shape,
        }
    }

    fn cell_is_selected(
        &self,
        cell: &alacritty_terminal::term::cell::Cell,
        point: Point<Line, Column>,
        selection_range: Option<alacritty_terminal::selection::SelectionRange>,
    ) -> bool {
        let _ = cell;
        let Some(range) = selection_range else {
            return false;
        };
        if range.is_block {
            return point.line >= range.start.line
                && point.line <= range.end.line
                && point.column >= range.start.column
                && point.column <= range.end.column;
        }
        if point.line < range.start.line || point.line > range.end.line {
            return false;
        }
        if point.line == range.start.line && point.column < range.start.column {
            return false;
        }
        if point.line == range.end.line && point.column > range.end.column {
            return false;
        }
        true
    }
}

impl TerminalCell {
    fn empty() -> Self {
        Self {
            character: ' ',
            fg: TermColor::Named(NamedColor::Foreground),
            bg: TermColor::Named(NamedColor::Background),
            flags: Flags::empty(),
            selected: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emulator(columns: usize, rows: usize) -> TerminalEmulator {
        TerminalEmulator::new(
            columns,
            rows,
            Arc::new(Mutex::new(
                Box::new(std::io::sink()) as Box<dyn Write + Send>
            )),
        )
    }

    #[test]
    fn printable_input_renders_in_grid() {
        let mut emulator = emulator(10, 3);
        emulator.feed(b"hello");
        let frame = emulator.renderable_frame();
        let rendered: String = frame.lines[0].iter().map(|cell| cell.character).collect();
        assert!(rendered.starts_with("hello"));
        assert_eq!(frame.cursor, Some((0, 5)));
    }

    #[test]
    fn resize_keeps_visible_content() {
        let mut emulator = emulator(10, 3);
        emulator.feed(b"abc");
        emulator.resize(20, 6);
        let frame = emulator.renderable_frame();
        let rendered: String = frame.lines[0].iter().map(|cell| cell.character).collect();
        assert!(rendered.starts_with("abc"));
    }

    #[test]
    fn selection_over_rendered_text_extracts_it() {
        let mut emulator = emulator(20, 3);
        emulator.feed(b"hello world");
        emulator.begin_selection(0, 0);
        emulator.extend_selection(0, 4);
        assert_eq!(emulator.selection_string().as_deref(), Some("hello"));
    }

    #[test]
    fn newline_moves_cursor_to_next_line() {
        let mut emulator = emulator(10, 4);
        emulator.feed(b"one\r\ntwo");
        let frame = emulator.renderable_frame();
        let second: String = frame.lines[1].iter().map(|cell| cell.character).collect();
        assert!(second.starts_with("two"));
    }
}
