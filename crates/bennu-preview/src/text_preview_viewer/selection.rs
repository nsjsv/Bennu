//! 查看器自管的光标与选择状态。
//!
//! 定位模型：(逻辑行, 行内 byte 偏移)。几何换算复用 retained 段落的
//! cosmic hit test —— `Hit::CharOffset` 即逻辑行内 byte 偏移；
//! 选区高亮按视觉行拆分，列边界 x 由对段落二分 hit test 求得，
//! 不保留第二份文本布局。
use iced::advanced::clipboard::Kind as ClipboardKind;
use iced::advanced::mouse::{click, Click};
use iced::advanced::text::{Hit, Paragraph};
use iced::advanced::Clipboard;
use iced::widget::text_editor;
use iced::{mouse, Point, Rectangle, Size};

use super::{
    base_line_height, line_height_of, visible_logical_lines, TextPreviewViewerState,
    TEXT_PREVIEW_DRAG_THRESHOLD_PX, TEXT_PREVIEW_VIEWER_PADDING,
};
use crate::text_preview::TextPreviewDocument;
/// 光标或选择端点：逻辑行 + 行内 byte 偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextPos {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

fn line_len(document: &TextPreviewDocument, line: usize) -> usize {
    document.line(line).map(str::len).unwrap_or(0)
}

/// 视口内坐标（text_bounds 相对）→ 光标位置。
/// 段落缓存缺失（行高量测回填后可见集合先行变化）时退化为行首，
/// 保证"单击清除选区"的语义不因缓存时序失效。
pub(crate) fn hit_test(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    position: Point,
) -> Option<TextPos> {
    let viewport_height = (state.scroll_height - TEXT_PREVIEW_VIEWER_PADDING * 2.0).max(0.0);
    let mut last_visible: Option<(usize, f32)> = None;
    for entry in visible_logical_lines(state, viewport_height) {
        if position.y < entry.visible_y {
            break;
        }
        if position.y < entry.visible_y + entry.height {
            let column = match state
                .retained_text_lines
                .iter()
                .find(|line| line.line_index == entry.line_index)
            {
                Some(paragraph) => paragraph
                    .paragraph
                    .hit_test(Point::new(position.x, position.y - entry.visible_y))
                    .map(Hit::cursor)
                    .unwrap_or_else(|| line_len(document, entry.line_index)),
                None => 0,
            };
            return Some(TextPos {
                line: entry.line_index,
                column,
            });
        }
        last_visible = Some((entry.line_index, entry.visible_y));
    }
    last_visible.map(|(line, _)| TextPos {
        line,
        column: line_len(document, line),
    })
}

/// 按下：按点击次数定位光标/选区并记录拖动起点，返回本次 Click
///（供调用方存入 last_click 状态）。
pub(super) fn press(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    position: Point,
) -> Click {
    let click = Click::new(position, mouse::Button::Left, state.last_click);
    let kind = click.kind();
    match hit_test(state, document, position) {
        Some(pos) => match kind {
            click::Kind::Single => {
                state.cursor = Some(pos);
                state.anchor = Some(pos);
            }
            click::Kind::Double => {
                let text = document.line(pos.line).unwrap_or_default();
                let (start, end) = word_range(text, pos.column);
                state.anchor = Some(TextPos {
                    line: pos.line,
                    column: start,
                });
                state.cursor = Some(TextPos {
                    line: pos.line,
                    column: end,
                });
            }
            click::Kind::Triple => {
                state.anchor = Some(TextPos {
                    line: pos.line,
                    column: 0,
                });
                state.cursor = Some(TextPos {
                    line: pos.line,
                    column: document.line(pos.line).map(str::len).unwrap_or(0),
                });
            }
        },
        // 定位失败（空文档等）也必须清掉残留选区：
        // 单击清除选择是硬语义，不能因定位失败而跳过。
        None => {
            state.cursor = None;
            state.anchor = None;
        }
    }
    state.is_focused = true;
    state.last_click = Some(click.clone());
    state.drag_click = Some(kind);
    state.drag_press = Some(position);
    state.drag_active = false;
    click
}

/// 按下期间的移动：超过拖动阈值后更新选择焦点，返回光标是否变化。
pub(super) fn drag(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    position: Point,
) -> bool {
    if state.drag_click != Some(click::Kind::Single) {
        return false;
    }
    let Some(press) = state.drag_press else {
        return false;
    };
    if !state.drag_active {
        let dx = position.x - press.x;
        let dy = position.y - press.y;
        if dx * dx + dy * dy < TEXT_PREVIEW_DRAG_THRESHOLD_PX * TEXT_PREVIEW_DRAG_THRESHOLD_PX {
            return false;
        }
        state.drag_active = true;
    }
    match hit_test(state, document, position) {
        Some(pos) => {
            let changed = state.cursor != Some(pos);
            state.cursor = Some(pos);
            changed
        }
        None => false,
    }
}

/// 释放：清拖动状态，返回释放前是否处于按下状态。
pub(super) fn release(state: &mut TextPreviewViewerState) -> bool {
    state.drag_press = None;
    state.drag_active = false;
    state.drag_click.take().is_some()
}

/// 键盘 Motion → 新光标位置；文档边界处原地不动。
/// 上/下按逻辑行移动并保持列（byte 偏移）钳制到目标行长。
pub(crate) fn move_position(
    document: &TextPreviewDocument,
    from: TextPos,
    motion: text_editor::Motion,
    visible_line_count: usize,
) -> TextPos {
    let line_count = document.line_count();
    let last = line_count.saturating_sub(1);
    let clamp_to = |line: usize, column: usize| TextPos {
        line,
        column: column.min(line_len(document, line)),
    };
    match motion {
        text_editor::Motion::Left => {
            if from.column > 0 {
                clamp_to(
                    from.line,
                    prev_boundary(document.line(from.line).unwrap_or_default(), from.column),
                )
            } else if from.line > 0 {
                TextPos {
                    line: from.line - 1,
                    column: line_len(document, from.line - 1),
                }
            } else {
                from
            }
        }
        text_editor::Motion::Right => {
            let len = line_len(document, from.line);
            if from.column < len {
                clamp_to(
                    from.line,
                    next_boundary(document.line(from.line).unwrap_or_default(), from.column),
                )
            } else if from.line < last {
                TextPos {
                    line: from.line + 1,
                    column: 0,
                }
            } else {
                from
            }
        }
        text_editor::Motion::Up => {
            if from.line > 0 {
                clamp_to(from.line - 1, from.column)
            } else {
                TextPos { line: 0, column: 0 }
            }
        }
        text_editor::Motion::Down => {
            if from.line < last {
                clamp_to(from.line + 1, from.column)
            } else {
                TextPos {
                    line: last,
                    column: line_len(document, last),
                }
            }
        }
        text_editor::Motion::Home => TextPos {
            line: from.line,
            column: 0,
        },
        text_editor::Motion::End => clamp_to(from.line, usize::MAX),
        text_editor::Motion::WordLeft => word_left(document, from, last),
        text_editor::Motion::WordRight => word_right(document, from, last),
        text_editor::Motion::PageUp => {
            let line = from
                .line
                .saturating_sub(visible_line_count.saturating_sub(1));
            clamp_to(line, from.column)
        }
        text_editor::Motion::PageDown => {
            let line = (from.line + visible_line_count.saturating_sub(1)).min(last);
            clamp_to(line, from.column)
        }
        text_editor::Motion::DocumentStart => TextPos { line: 0, column: 0 },
        text_editor::Motion::DocumentEnd => clamp_to(last, usize::MAX),
    }
}

fn prev_boundary(text: &str, column: usize) -> usize {
    let mut column = column.min(text.len());
    while column > 0 && !text.is_char_boundary(column) {
        column -= 1;
    }
    text[..column]
        .chars()
        .next_back()
        .map(|c| column - c.len_utf8())
        .unwrap_or(0)
}

fn next_boundary(text: &str, column: usize) -> usize {
    text[column.min(text.len())..]
        .chars()
        .next()
        .map(|c| column + c.len_utf8())
        .unwrap_or(text.len())
}

fn is_word_char(text: &str, index: usize) -> bool {
    text[index..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn word_left(document: &TextPreviewDocument, from: TextPos, last: usize) -> TextPos {
    let text = document.line(from.line).unwrap_or_default();
    let mut column = from.column.min(text.len());
    while column > 0 && !is_word_char(text, prev_boundary(text, column)) {
        column = prev_boundary(text, column);
    }
    while column > 0 && is_word_char(text, prev_boundary(text, column)) {
        column = prev_boundary(text, column);
    }
    if column == from.column && from.line > 0 {
        return TextPos {
            line: from.line - 1,
            column: line_len(document, from.line - 1),
        };
    }
    let _ = last;
    TextPos {
        line: from.line,
        column,
    }
}

fn word_right(document: &TextPreviewDocument, from: TextPos, last: usize) -> TextPos {
    let text = document.line(from.line).unwrap_or_default();
    let mut column = from.column.min(text.len());
    while column < text.len() && !is_word_char(text, column) {
        column = next_boundary(text, column);
    }
    while column < text.len() && is_word_char(text, column) {
        column = next_boundary(text, column);
    }
    if column == from.column && from.line < last {
        return TextPos {
            line: from.line + 1,
            column: 0,
        };
    }
    TextPos {
        line: from.line,
        column,
    }
}

/// 双击选词：与点击处同类（词/空白/标点）字符的连续段。
pub(crate) fn word_range(text: &str, column: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }
    let mut start = column.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let class_at = |index: usize| {
        text[index..]
            .chars()
            .next()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    1u8
                } else if c.is_whitespace() {
                    2
                } else {
                    3
                }
            })
            .unwrap_or(0)
    };
    // 点击落在两类字符的边界上时以右侧字符为准（双击标点选标点、
    // 双击词首选词）；同类内部则沿用左侧字符类。
    let class = if start > 0 && class_at(start) != class_at(prev_boundary(text, start)) {
        class_at(start)
    } else if start > 0 {
        class_at(prev_boundary(text, start))
    } else {
        class_at(0)
    };
    let mut end = start;
    while start > 0 && class_at(prev_boundary(text, start)) == class {
        start = prev_boundary(text, start);
    }
    while end < text.len() && class_at(end) == class {
        end = next_boundary(text, end);
    }
    (start, end)
}

/// 选区文本（有序端点间），跨行以 `\n` 连接。
pub(crate) fn selection_text(
    document: &TextPreviewDocument,
    anchor: TextPos,
    cursor: TextPos,
) -> Option<String> {
    let (start, end) = ordered(anchor, cursor);
    if start.line == end.line {
        let text = document.line(start.line)?;
        return Some(text[start.column..end.column].to_owned());
    }
    let mut out = String::new();
    out.push_str(&document.line(start.line)?[start.column..]);
    for line in start.line + 1..end.line {
        out.push('\n');
        out.push_str(document.line(line)?);
    }
    out.push('\n');
    out.push_str(&document.line(end.line)?[..end.column]);
    Some(out)
}

fn ordered(a: TextPos, b: TextPos) -> (TextPos, TextPos) {
    if (a.line, a.column) <= (b.line, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

/// 选区高亮矩形（text_bounds 相对坐标）：按可见逻辑行的视觉行拆分，
/// 列边界 x 用二分 hit test 求得，每视觉行成本 O(log 列数)。
pub(crate) fn selection_quads(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> Vec<Rectangle> {
    let Some((anchor, cursor)) = state.anchor.zip(state.cursor) else {
        return Vec::new();
    };
    let (start, end) = ordered(anchor, cursor);
    let viewport_height = (state.scroll_height - TEXT_PREVIEW_VIEWER_PADDING * 2.0).max(0.0);
    let base = base_line_height();

    let mut quads = Vec::new();
    for entry in visible_logical_lines(state, viewport_height) {
        if entry.line_index < start.line || entry.line_index > end.line {
            continue;
        }
        let Some(retained) = state
            .retained_text_lines
            .iter()
            .find(|line| line.line_index == entry.line_index)
        else {
            continue;
        };
        let Some(text) = document.line(entry.line_index) else {
            continue;
        };
        let paragraph = &retained.paragraph;
        let row_height = paragraph.min_bounds().height;
        let row_count = ((row_height / base).round() as usize).max(1);
        let width = paragraph.min_bounds().width.max(1.0);
        // 每个视觉行的高亮 = 该视觉行字符范围 ∩ 选区 byte 范围。
        // 以前只对首/尾视觉行应用选区端点，单点光标落在 wrap 续行
        // （中间视觉行）时会被整行画出——表现为"单击选中一段"。
        let sel_lo = if start.line == entry.line_index {
            start.column
        } else {
            0
        };
        let sel_hi = if end.line == entry.line_index {
            end.column
        } else {
            text.len()
        };

        for row in 0..row_count {
            let row_y = entry.visible_y + row as f32 * base;
            let mid_y = row as f32 * base + base * 0.5;
            let (row_start, row_end) = if row_count == 1 {
                (0, text.len())
            } else {
                (
                    paragraph
                        .hit_test(Point::new(0.0, mid_y))
                        .map(Hit::cursor)
                        .unwrap_or(0),
                    paragraph
                        .hit_test(Point::new(width, mid_y))
                        .map(Hit::cursor)
                        .unwrap_or(text.len()),
                )
            };
            let from = row_start.max(sel_lo);
            let to = row_end.min(sel_hi);
            if to <= from {
                continue;
            }
            let x0 = column_x(paragraph, mid_y, width, text, from);
            let x1 = column_x(paragraph, mid_y, width, text, to);
            quads.push(Rectangle::new(
                Point::new(x0, row_y),
                Size::new((x1 - x0).max(1.0), base),
            ));
        }
    }
    quads
}

fn column_x(paragraph: &impl Paragraph, mid_y: f32, width: f32, text: &str, column: usize) -> f32 {
    if column == 0 {
        return 0.0;
    }
    if column >= text.len() {
        return width;
    }
    let (mut lo, mut hi) = (0.0f32, width);
    for _ in 0..14 {
        let mid = (lo + hi) * 0.5;
        let hit_column = paragraph
            .hit_test(Point::new(mid, mid_y))
            .map(Hit::cursor)
            .unwrap_or(text.len());
        if hit_column < column {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

pub(super) fn apply_key_binding<Message>(
    binding: text_editor::Binding<Message>,
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    clipboard: &mut dyn Clipboard,
) {
    match binding {
        text_editor::Binding::Copy | text_editor::Binding::Cut => {
            if let (Some(anchor), Some(cursor)) = (state.anchor, state.cursor) {
                if let Some(text) = selection_text(document, anchor, cursor) {
                    if !text.is_empty() {
                        clipboard.write(ClipboardKind::Standard, text);
                    }
                }
            }
        }
        text_editor::Binding::Move(motion) => apply_motion(state, document, motion, false),
        text_editor::Binding::Select(motion) => apply_motion(state, document, motion, true),
        text_editor::Binding::SelectWord => {
            let cursor = state.cursor.unwrap_or(TextPos { line: 0, column: 0 });
            let text = document.line(cursor.line).unwrap_or_default();
            let (start, end) = word_range(text, cursor.column);
            state.anchor = Some(TextPos {
                line: cursor.line,
                column: start,
            });
            state.cursor = Some(TextPos {
                line: cursor.line,
                column: end,
            });
        }
        text_editor::Binding::SelectLine => {
            let cursor = state.cursor.unwrap_or(TextPos { line: 0, column: 0 });
            state.anchor = Some(TextPos {
                line: cursor.line,
                column: 0,
            });
            state.cursor = Some(TextPos {
                line: cursor.line,
                column: document.line(cursor.line).map(str::len).unwrap_or(0),
            });
        }
        text_editor::Binding::SelectAll => {
            let last = document.line_count().saturating_sub(1);
            state.anchor = Some(TextPos { line: 0, column: 0 });
            state.cursor = Some(TextPos {
                line: last,
                column: document.line(last).map(str::len).unwrap_or(0),
            });
        }
        text_editor::Binding::Sequence(sequence) => {
            for binding in sequence {
                apply_key_binding(binding, state, document, clipboard);
            }
        }
        text_editor::Binding::Unfocus
        | text_editor::Binding::Paste
        | text_editor::Binding::Insert(_)
        | text_editor::Binding::Enter
        | text_editor::Binding::Backspace
        | text_editor::Binding::Delete
        | text_editor::Binding::Custom(_) => {}
    }
}

/// 光标移动：非扩展移动取消选区（anchor 跟随光标），
/// 扩展移动保留 anchor 形成选区；随后滚动跟随使光标行可见。
fn apply_motion(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    motion: text_editor::Motion,
    extend: bool,
) {
    let from = state.cursor.unwrap_or(TextPos { line: 0, column: 0 });
    let viewport_height = (state.scroll_height - TEXT_PREVIEW_VIEWER_PADDING * 2.0).max(0.0);
    let visible_line_count = ((viewport_height / base_line_height()).floor() as usize).max(1);
    let to = move_position(document, from, motion, visible_line_count);
    state.cursor = Some(to);
    if !extend {
        state.anchor = Some(to);
    } else if state.anchor.is_none() {
        state.anchor = Some(from);
    }
    ensure_cursor_visible(state, document, to.line);
}

/// 光标行移出锚定行覆盖的估算可见范围时，整行粒度滚动锚点。
fn ensure_cursor_visible(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    cursor_line: usize,
) {
    let viewport_height = (state.scroll_height - TEXT_PREVIEW_VIEWER_PADDING * 2.0).max(0.0);
    let visible_line_count = ((viewport_height / base_line_height()).floor() as usize).max(1);
    let target = if cursor_line < state.scroll_line {
        Some(cursor_line)
    } else if cursor_line >= state.scroll_line + visible_line_count {
        Some(cursor_line + 1 - visible_line_count)
    } else {
        None
    };
    let Some(target) = target else {
        return;
    };
    let target = target.min(document.line_count().saturating_sub(1));
    while state.scroll_line < target {
        state.anchor_prefix += line_height_of(state, state.scroll_line);
        state.scroll_line += 1;
    }
    while state.scroll_line > target {
        state.scroll_line -= 1;
        state.anchor_prefix -= line_height_of(state, state.scroll_line);
    }
    state.scroll_pixel = 0.0;
}
