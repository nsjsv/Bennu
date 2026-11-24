//! 一帧要画什么：把屏幕内容按格子算好（纯函数，测得到）。画到屏幕上的是 [`super::draw_paint`]。

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};
use iced::{Color, Theme};

use super::super::palette::Palette;

/// 暗淡（faint）的字的不透明度。
const DIM_ALPHA: f32 = 0.66;

/// 一帧要画的东西，坐标都是格子（行、列从 0 数）。
#[derive(Debug, Default, PartialEq)]
pub(super) struct GridPaint {
    /// 和默认背景不同的背景段（含选区、实心光标）。
    pub(super) backgrounds: Vec<CellSpan>,
    pub(super) texts: Vec<TextRun>,
    /// 下划线、删除线。
    pub(super) strokes: Vec<Stroke>,
    /// 没被并进格子颜色的光标：空心框、下划线、竖线。
    pub(super) cursor: Option<CursorMark>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CellSpan {
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) width: usize,
    pub(super) color: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TextRun {
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) content: String,
    pub(super) color: Color,
    pub(super) bold: bool,
    pub(super) italic: bool,
    /// 只有 ASCII：等宽字体一定有，宽度就是格宽，可以不做复杂排版。
    pub(super) ascii: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Stroke {
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) width: usize,
    pub(super) kind: StrokeKind,
    pub(super) color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StrokeKind {
    Underline,
    DoubleUnderline,
    Strikeout,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CursorMark {
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) width: usize,
    pub(super) shape: CursorShape,
    pub(super) color: Color,
}

/// 选区和光标用的颜色（从主题来）。
#[derive(Debug, Clone, Copy)]
pub(super) struct Highlight {
    pub(super) selection_background: Color,
    pub(super) selection_text: Color,
}

impl Highlight {
    pub(super) fn for_theme(theme: &Theme) -> Self {
        let weak = theme.extended_palette().primary.weak;
        Self {
            selection_background: weak.color,
            selection_text: weak.text,
        }
    }
}

/// 把屏幕内容按格子算成要画的东西。字按列号放，不按顺序数：宽字符后面的 spacer 不画，但也
/// 不让后面的字往前挪。同一行里连续的 ASCII、同色同字形的格子并成一段；别的字符一格一放，
/// 字体回退后宽度不定，成段排会把后面挤歪。
pub(super) fn paint(
    content: RenderableContent<'_>,
    palette: &Palette,
    highlight: Highlight,
    focused: bool,
) -> GridPaint {
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let selection = content.selection;
    let cursor = content.cursor;
    let cursor_line = cursor.point.line.0 + offset;
    let cursor_column = cursor.point.column.0;
    let block_cursor = focused && cursor.shape == CursorShape::Block;
    let default_background = palette.background();
    let mut painted = PaintBuilder::default();
    let mut cursor_width = 1;

    for indexed in content.display_iter {
        let cell = indexed.cell;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let line = (indexed.point.line.0 + offset) as usize;
        let column = indexed.point.column.0;
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let at_cursor = line as i32 == cursor_line && column == cursor_column;
        if at_cursor {
            cursor_width = width;
        }

        let (mut foreground, mut background) = (cell.fg, cell.bg);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        let mut text_color = palette.color(foreground, colors);
        let mut background_color = match background {
            TermColor::Named(NamedColor::Background) => None,
            other => Some(palette.color(other, colors)),
        };
        if cell.flags.contains(Flags::DIM) {
            text_color.a *= DIM_ALPHA;
        }
        if selection.is_some_and(|range| range.contains(indexed.point)) {
            text_color = highlight.selection_text;
            background_color = Some(highlight.selection_background);
        }
        if at_cursor && block_cursor {
            // 实心光标：格子底色换成光标色，字用原来的底色，看得清压在下面的字。
            let cursor_color = palette.color(TermColor::Named(NamedColor::Cursor), colors);
            text_color = background_color.unwrap_or(default_background);
            background_color = Some(cursor_color);
        }

        if let Some(color) = background_color {
            painted.background(CellSpan {
                line,
                column,
                width,
                color,
            });
        }
        let stroke_color = cell
            .underline_color()
            .map(|color| palette.color(color, colors))
            .unwrap_or(text_color);
        if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
            painted.stroke(
                line,
                column,
                width,
                StrokeKind::DoubleUnderline,
                stroke_color,
            );
        } else if cell.flags.intersects(Flags::ALL_UNDERLINES) {
            painted.stroke(line, column, width, StrokeKind::Underline, stroke_color);
        }
        if cell.flags.contains(Flags::STRIKEOUT) {
            painted.stroke(line, column, width, StrokeKind::Strikeout, text_color);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            painted.end_text_run();
            continue;
        }
        let style = TextStyle {
            color: text_color,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
        };
        match cell.zerowidth() {
            None if cell.c == ' ' => painted.space(line, column),
            None if cell.c.is_ascii_graphic() && width == 1 => {
                painted.ascii(line, column, cell.c, style);
            }
            zerowidth => {
                let mut content = String::from(cell.c);
                content.extend(zerowidth.into_iter().flatten());
                painted.single(line, column, content, style);
            }
        }
    }
    painted.end_text_run();

    let mut painted = painted.finish();
    if cursor.shape != CursorShape::Hidden && !block_cursor && cursor_line >= 0 {
        painted.cursor = Some(CursorMark {
            line: cursor_line as usize,
            column: cursor_column,
            width: cursor_width,
            // 没焦点时一律画空心框（跟别的终端一样），看得出光标在哪又不抢眼。
            shape: if focused {
                cursor.shape
            } else {
                CursorShape::HollowBlock
            },
            color: palette.color(TermColor::Named(NamedColor::Cursor), colors),
        });
    }
    painted
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TextStyle {
    color: Color,
    bold: bool,
    italic: bool,
}

#[derive(Default)]
struct PaintBuilder {
    paint: GridPaint,
    /// 正在拼的 ASCII 段和它的字形；空格先跟着，收尾时去掉段尾的空格。
    run: Option<(TextRun, TextStyle)>,
}

impl PaintBuilder {
    fn background(&mut self, span: CellSpan) {
        if let Some(last) = self.paint.backgrounds.last_mut() {
            if last.line == span.line
                && last.column + last.width == span.column
                && last.color == span.color
            {
                last.width += span.width;
                return;
            }
        }
        self.paint.backgrounds.push(span);
    }

    fn stroke(&mut self, line: usize, column: usize, width: usize, kind: StrokeKind, color: Color) {
        if let Some(last) = self.paint.strokes.last_mut() {
            if last.line == line
                && last.column + last.width == column
                && last.kind == kind
                && last.color == color
            {
                last.width += width;
                return;
            }
        }
        self.paint.strokes.push(Stroke {
            line,
            column,
            width,
            kind,
            color,
        });
    }

    fn ascii(&mut self, line: usize, column: usize, character: char, style: TextStyle) {
        if let Some((run, run_style)) = &mut self.run {
            if run.line == line && run.column + run.content.len() == column && *run_style == style {
                run.content.push(character);
                return;
            }
        }
        self.end_text_run();
        self.run = Some((
            TextRun {
                line,
                column,
                content: String::from(character),
                color: style.color,
                bold: style.bold,
                italic: style.italic,
                ascii: true,
            },
            style,
        ));
    }

    /// 空格看不见，颜色字形不影响它：接在正在拼的段后面，免得一行字被空格切成很多段。
    fn space(&mut self, line: usize, column: usize) {
        match &mut self.run {
            Some((run, _)) if run.line == line && run.column + run.content.len() == column => {
                run.content.push(' ');
            }
            _ => self.end_text_run(),
        }
    }

    fn single(&mut self, line: usize, column: usize, content: String, style: TextStyle) {
        self.end_text_run();
        self.paint.texts.push(TextRun {
            line,
            column,
            content,
            color: style.color,
            bold: style.bold,
            italic: style.italic,
            ascii: false,
        });
    }

    fn end_text_run(&mut self) {
        if let Some((mut run, _)) = self.run.take() {
            run.content
                .truncate(run.content.trim_end_matches(' ').len());
            self.paint.texts.push(run);
        }
    }

    fn finish(self) -> GridPaint {
        self.paint
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::Side;
    use alacritty_terminal::selection::{Selection, SelectionType};

    use super::*;
    use crate::terminal_panel::emulator::grid_point;
    use crate::terminal_panel::emulator::tests::{emulator, feed};

    fn painted(bytes: &[u8], focused: bool) -> GridPaint {
        let emulator = emulator(20, 3);
        feed(&emulator, bytes);
        let term = emulator.lock();
        paint(
            term.renderable_content(),
            &Palette::for_theme(&Theme::Dark),
            Highlight::for_theme(&Theme::Dark),
            focused,
        )
    }

    fn texts(paint: &GridPaint) -> Vec<(usize, usize, &str)> {
        paint
            .texts
            .iter()
            .map(|run| (run.line, run.column, run.content.as_str()))
            .collect()
    }

    #[test]
    fn text_after_wide_characters_stays_in_its_columns() {
        let paint = painted("中文ab 你c".as_bytes(), false);
        // 中、文各占两格，ab 从第 4 列开始；空格后的「你」在第 7 列，c 在第 9 列。
        assert_eq!(
            texts(&paint),
            vec![
                (0, 0, "中"),
                (0, 2, "文"),
                (0, 4, "ab"),
                (0, 7, "你"),
                (0, 9, "c")
            ]
        );
    }

    #[test]
    fn ascii_in_one_style_is_one_run_and_spaces_do_not_split_it() {
        let paint = painted(b"ls -la  x\x1b[31mred\x1b[0m", false);
        assert_eq!(texts(&paint), vec![(0, 0, "ls -la  x"), (0, 9, "red")]);
    }

    #[test]
    fn background_colors_are_painted_and_merged_per_line() {
        let paint = painted(b"\x1b[44mab\x1b[0m \x1b[7mcd", false);
        let spans: Vec<(usize, usize, usize)> = paint
            .backgrounds
            .iter()
            .map(|span| (span.line, span.column, span.width))
            .collect();
        // 蓝底两格一段；反色的两格用前景色当底，又是一段。
        assert_eq!(spans, vec![(0, 0, 2), (0, 3, 2)]);
        let palette = Palette::for_theme(&Theme::Dark);
        assert_eq!(
            paint.backgrounds[1].color,
            palette.color(
                TermColor::Named(NamedColor::Foreground),
                &Default::default()
            )
        );
    }

    #[test]
    fn a_selection_gets_a_solid_background_even_over_empty_cells() {
        let emulator = emulator(20, 3);
        feed(&emulator, b"hi");
        let mut term = emulator.lock();
        let mut selection =
            Selection::new(SelectionType::Simple, grid_point(&term, 0, 0), Side::Left);
        selection.update(grid_point(&term, 0, 5), Side::Right);
        term.selection = Some(selection);
        let highlight = Highlight::for_theme(&Theme::Dark);
        let paint = paint(
            term.renderable_content(),
            &Palette::for_theme(&Theme::Dark),
            highlight,
            false,
        );

        assert_eq!(
            paint.backgrounds,
            vec![CellSpan {
                line: 0,
                column: 0,
                width: 6,
                color: highlight.selection_background
            }]
        );
        assert_eq!(paint.texts[0].color, highlight.selection_text);
    }

    #[test]
    fn a_focused_block_cursor_inverts_its_cell_and_an_unfocused_one_is_hollow() {
        let focused = painted(b"ab", true);
        assert_eq!(focused.cursor, None);
        assert_eq!(focused.backgrounds.len(), 1);
        assert_eq!(
            (focused.backgrounds[0].line, focused.backgrounds[0].column),
            (0, 2)
        );

        let unfocused = painted(b"ab", false);
        assert!(unfocused.backgrounds.is_empty());
        let cursor = unfocused
            .cursor
            .expect("an unfocused cursor is still shown");
        assert_eq!(
            (cursor.line, cursor.column, cursor.shape),
            (0, 2, CursorShape::HollowBlock)
        );
    }

    #[test]
    fn underlines_and_strikeouts_are_strokes() {
        let paint = painted(b"\x1b[4mab\x1b[0m\x1b[9mc", false);
        let strokes: Vec<(usize, usize, StrokeKind)> = paint
            .strokes
            .iter()
            .map(|stroke| (stroke.column, stroke.width, stroke.kind))
            .collect();
        assert_eq!(
            strokes,
            vec![(0, 2, StrokeKind::Underline), (2, 1, StrokeKind::Strikeout)]
        );
    }
}
