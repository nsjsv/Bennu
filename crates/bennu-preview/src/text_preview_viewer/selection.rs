//! 查看器自管的光标与选择状态。
//!
//! 位置 = （显示行起点, 行内文字的字节下标）。文件末尾是（文件长度, 0），全选因此不用把
//! 整篇读进来。几何换算复用 retained 段落的 hit test；选区高亮按视觉行拆分，列边界 x 由
//! 对段落二分 hit test 求得。选区超出窗口时复制交给引擎在后台读。

use iced::advanced::clipboard::Kind as ClipboardKind;
use iced::advanced::mouse::{click, Click};
use iced::advanced::text::{Hit, Paragraph};
use iced::advanced::Clipboard;
use iced::widget::text_editor;
use iced::{mouse, Point, Rectangle, Size};

use super::{
    base_line_height, row_height, scroll_viewer_by, visible_rows, TextPreviewViewerState,
    VisibleRow, TEXT_PREVIEW_DRAG_THRESHOLD_PX,
};
use crate::text_document::{TextJump, TextPosition, TextPreviewDocument, TextViewerEvent};

/// 视口内坐标（text_bounds 相对）→ 光标位置。段落缓存缺失时退化为行首，
/// 保证「单击清除选区」的语义不因缓存时序失效。
pub(crate) fn hit_test(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    position: Point,
) -> Option<TextPosition> {
    let rows = document.rows();
    let mut last_visible = None;
    for entry in visible_rows(state, document) {
        if position.y < entry.visible_y {
            break;
        }
        let row = &rows[entry.index];
        if position.y < entry.visible_y + entry.height {
            let column = match state
                .retained_rows
                .iter()
                .find(|retained| retained.start == row.start)
            {
                Some(retained) => retained
                    .paragraph
                    .hit_test(Point::new(position.x, position.y - entry.visible_y))
                    .map(Hit::cursor)
                    .unwrap_or(row.text.len()),
                None => 0,
            };
            return Some(TextPosition {
                row: row.start,
                column,
            });
        }
        last_visible = Some(TextPosition {
            row: row.start,
            column: row.text.len(),
        });
    }
    last_visible
}

/// 按下：按点击次数定位光标/选区并记录拖动起点。
pub(super) fn press(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    position: Point,
) -> Click {
    let click = Click::new(position, mouse::Button::Left, state.last_click);
    let kind = click.kind();
    match hit_test(state, document, position) {
        Some(pos) => {
            let text = row_text(document, pos.row);
            let (anchor, cursor) = match kind {
                click::Kind::Single => (pos.column, pos.column),
                click::Kind::Double => word_range(text, pos.column),
                // 三击选点中的那一段（超长行切开的段各自独立，第六轮决定）。
                click::Kind::Triple => (0, text.len()),
            };
            state.selection_anchor = Some(TextPosition {
                row: pos.row,
                column: anchor,
            });
            state.cursor = Some(TextPosition {
                row: pos.row,
                column: cursor,
            });
        }
        // 定位失败（空文档等）也必须清掉残留选区。
        None => {
            state.cursor = None;
            state.selection_anchor = None;
        }
    }
    state.is_focused = true;
    state.last_click = Some(click);
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

fn row_text(document: &TextPreviewDocument, row: u64) -> &str {
    document
        .row_index(row)
        .ok()
        .map_or("", |index| document.rows()[index].text.as_str())
}

/// 键盘 Motion → 新光标位置，只在窗口里移动；文件头/尾直接给出位置，由滚动跟随
/// 去请求那里的内容。
pub(crate) fn move_position(
    document: &TextPreviewDocument,
    from: TextPosition,
    motion: text_editor::Motion,
    visible_row_count: usize,
) -> TextPosition {
    let rows = document.rows();
    let document_start = TextPosition {
        row: document.data_start(),
        column: 0,
    };
    let document_end = TextPosition {
        row: document.len(),
        column: 0,
    };
    let Ok(index) = document.row_index(from.row) else {
        return match motion {
            text_editor::Motion::DocumentStart => document_start,
            text_editor::Motion::DocumentEnd => document_end,
            _ => from,
        };
    };
    let text = rows[index].text.as_str();
    let at = |index: usize, column: usize| TextPosition {
        row: rows[index].start,
        column: rows[index].text.floor_char_boundary(column),
    };
    let row_end = |index: usize| at(index, usize::MAX);
    let last = rows.len() - 1;
    match motion {
        text_editor::Motion::Left if from.column > 0 => at(index, prev_boundary(text, from.column)),
        text_editor::Motion::Left if index > 0 => row_end(index - 1),
        text_editor::Motion::Right if from.column < text.len() => {
            at(index, next_boundary(text, from.column))
        }
        text_editor::Motion::Right if index < last => at(index + 1, 0),
        text_editor::Motion::Left | text_editor::Motion::Right => from,
        text_editor::Motion::Up if index > 0 => at(index - 1, from.column),
        text_editor::Motion::Up => at(0, 0),
        text_editor::Motion::Down if index < last => at(index + 1, from.column),
        text_editor::Motion::Down => row_end(last),
        text_editor::Motion::Home => at(index, 0),
        text_editor::Motion::End => row_end(index),
        text_editor::Motion::WordLeft => word_left(document, index, from.column),
        text_editor::Motion::WordRight => word_right(document, index, from.column),
        text_editor::Motion::PageUp => at(
            index.saturating_sub(visible_row_count.saturating_sub(1)),
            from.column,
        ),
        text_editor::Motion::PageDown => at(
            (index + visible_row_count.saturating_sub(1)).min(last),
            from.column,
        ),
        text_editor::Motion::DocumentStart => document_start,
        text_editor::Motion::DocumentEnd => document_end,
    }
}

fn prev_boundary(text: &str, column: usize) -> usize {
    let column = text.floor_char_boundary(column);
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

fn word_left(document: &TextPreviewDocument, index: usize, from: usize) -> TextPosition {
    let rows = document.rows();
    let text = rows[index].text.as_str();
    let mut column = from.min(text.len());
    while column > 0 && !is_word_char(text, prev_boundary(text, column)) {
        column = prev_boundary(text, column);
    }
    while column > 0 && is_word_char(text, prev_boundary(text, column)) {
        column = prev_boundary(text, column);
    }
    if column == from && index > 0 {
        return TextPosition {
            row: rows[index - 1].start,
            column: rows[index - 1].text.len(),
        };
    }
    TextPosition {
        row: rows[index].start,
        column,
    }
}

fn word_right(document: &TextPreviewDocument, index: usize, from: usize) -> TextPosition {
    let rows = document.rows();
    let text = rows[index].text.as_str();
    let mut column = from.min(text.len());
    while column < text.len() && !is_word_char(text, column) {
        column = next_boundary(text, column);
    }
    while column < text.len() && is_word_char(text, column) {
        column = next_boundary(text, column);
    }
    if column == from && index + 1 < rows.len() {
        return TextPosition {
            row: rows[index + 1].start,
            column: 0,
        };
    }
    TextPosition {
        row: rows[index].start,
        column,
    }
}

/// 双击选词：与点击处同类（词/空白/标点）字符的连续段。
pub(crate) fn word_range(text: &str, column: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }
    let mut start = text.floor_char_boundary(column);
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

/// 选区全在窗口里时直接拼出文字：同一原始行的各段直接相连，原始行之间补 `\n`。
/// 有一端在窗口外时 None（交给引擎后台读）。
pub(crate) fn selection_text(
    document: &TextPreviewDocument,
    anchor: TextPosition,
    cursor: TextPosition,
) -> Option<String> {
    let (start, end) = ordered(anchor, cursor);
    let rows = document.rows();
    let first = document.row_index(start.row).ok()?;
    let last = match document.row_index(end.row) {
        Ok(index) => index,
        // 选到文件尾：窗口到达文件尾时就是最后一行之后。
        Err(index) if end.row == document.len() && document.at_end() => index,
        Err(_) => return None,
    };
    let mut out = String::new();
    let last = last.min(rows.len().saturating_sub(1));
    for (index, row) in rows.iter().enumerate().take(last + 1).skip(first) {
        if row.start == end.row && end.column == 0 && index != first {
            if row.is_line_start() {
                out.push('\n');
            }
            break;
        }
        if index != first && row.is_line_start() {
            out.push('\n');
        }
        let low = if index == first {
            row.text.floor_char_boundary(start.column)
        } else {
            0
        };
        let high = if row.start == end.row {
            row.text.floor_char_boundary(end.column)
        } else {
            row.text.len()
        };
        if low < high {
            out.push_str(&row.text[low..high]);
        }
        if row.start == end.row {
            break;
        }
    }
    Some(out)
}

fn ordered(a: TextPosition, b: TextPosition) -> (TextPosition, TextPosition) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// 选区高亮矩形（text_bounds 相对坐标）：按可见行的视觉行拆分，列边界 x 用二分
/// hit test 求得，每视觉行成本 O(log 列数)。
pub(crate) fn selection_quads(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    visible: &[VisibleRow],
) -> Vec<Rectangle> {
    let Some((anchor, cursor)) = state.selection_anchor.zip(state.cursor) else {
        return Vec::new();
    };
    let (start, end) = ordered(anchor, cursor);
    if start == end {
        return Vec::new();
    }
    let base = base_line_height();
    let rows = document.rows();

    let mut quads = Vec::new();
    for entry in visible {
        let row = &rows[entry.index];
        if row.start < start.row && row.end <= start.row
            || row.start > end.row
            || row.start == end.row && end.column == 0 && start.row != end.row
        {
            continue;
        }
        let Some(retained) = state
            .retained_rows
            .iter()
            .find(|retained| retained.start == row.start)
        else {
            continue;
        };
        let text = row.text.as_str();
        let paragraph = &retained.paragraph;
        let row_height_px = paragraph.min_bounds().height;
        let visual_rows = ((row_height_px / base).round() as usize).max(1);
        let width = paragraph.min_bounds().width.max(1.0);
        // 每个视觉行的高亮 = 该视觉行字符范围 ∩ 选区 byte 范围。
        let sel_lo = if start.row == row.start {
            start.column
        } else {
            0
        };
        let sel_hi = if end.row == row.start {
            end.column
        } else {
            text.len()
        };

        for visual in 0..visual_rows {
            let row_y = entry.visible_y + visual as f32 * base;
            let mid_y = visual as f32 * base + base * 0.5;
            let (row_start, row_end) = if visual_rows == 1 {
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
            copy_selection(state, document, clipboard)
        }
        text_editor::Binding::Move(motion) => apply_motion(state, document, motion, false),
        text_editor::Binding::Select(motion) => apply_motion(state, document, motion, true),
        text_editor::Binding::SelectWord => {
            let cursor = state.cursor.unwrap_or(default_position(state));
            let (start, end) = word_range(row_text(document, cursor.row), cursor.column);
            state.selection_anchor = Some(TextPosition {
                row: cursor.row,
                column: start,
            });
            state.cursor = Some(TextPosition {
                row: cursor.row,
                column: end,
            });
        }
        text_editor::Binding::SelectLine => {
            let cursor = state.cursor.unwrap_or(default_position(state));
            state.selection_anchor = Some(TextPosition {
                row: cursor.row,
                column: 0,
            });
            state.cursor = Some(TextPosition {
                row: cursor.row,
                column: row_text(document, cursor.row).len(),
            });
        }
        text_editor::Binding::SelectAll => {
            state.selection_anchor = Some(TextPosition {
                row: document.data_start(),
                column: 0,
            });
            state.cursor = Some(TextPosition {
                row: document.len(),
                column: 0,
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

/// 复制：选区在窗口里直接写剪贴板；超出窗口交给引擎后台读（超过 15 MB 引擎不复制）。
fn copy_selection(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    clipboard: &mut dyn Clipboard,
) {
    let Some((anchor, cursor)) = state.selection_anchor.zip(state.cursor) else {
        return;
    };
    if anchor == cursor {
        return;
    }
    match selection_text(document, anchor, cursor) {
        Some(text) if !text.is_empty() => clipboard.write(ClipboardKind::Standard, text),
        Some(_) => {}
        None => {
            let (from, to) = ordered(anchor, cursor);
            state
                .pending_events
                .push(TextViewerEvent::CopyRequested { from, to });
        }
    }
}

fn default_position(state: &TextPreviewViewerState) -> TextPosition {
    TextPosition {
        row: state.anchor,
        column: 0,
    }
}

/// 光标移动：非扩展移动取消选区（anchor 跟随光标），扩展移动保留 anchor 形成选区；
/// 随后滚动跟随使光标行可见。
fn apply_motion(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    motion: text_editor::Motion,
    extend: bool,
) {
    let from = state.cursor.unwrap_or(default_position(state));
    let visible_row_count = ((state.viewport_height / base_line_height()).floor() as usize).max(1);
    let to = move_position(document, from, motion, visible_row_count);
    state.cursor = Some(to);
    if !extend {
        state.selection_anchor = Some(to);
    } else if state.selection_anchor.is_none() {
        state.selection_anchor = Some(from);
    }
    ensure_cursor_visible(state, document, to);
}

/// 光标行在窗口里就滚到看得见；不在窗口里（文件头/尾）就请求跳过去。
fn ensure_cursor_visible(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    cursor: TextPosition,
) {
    let rows = document.rows();
    let Ok(cursor_index) = document.row_index(cursor.row) else {
        let jump = if cursor.row >= document.len() {
            TextJump::End
        } else if cursor.row <= document.data_start() {
            TextJump::Start
        } else {
            TextJump::Offset(cursor.row)
        };
        state
            .pending_events
            .push(TextViewerEvent::JumpRequested(jump));
        return;
    };
    let Some(anchor_index) = document.row_index(state.anchor).ok() else {
        return;
    };
    if cursor_index < anchor_index || cursor_index == anchor_index && state.pixel > 0.0 {
        state.anchor = rows[cursor_index].start;
        state.pixel = 0.0;
        scroll_viewer_by(state, document, 0.0);
        return;
    }
    // 光标行底边超出视口：往下滚到它底边贴住视口底边。
    let mut bottom = -state.pixel;
    for row in rows.range(anchor_index..=cursor_index) {
        bottom += row_height(state, row.start);
    }
    if bottom > state.viewport_height {
        scroll_viewer_by(state, document, bottom - state.viewport_height);
    }
}
