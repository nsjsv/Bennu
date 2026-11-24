//! 纵向位置的几何：可见行、按像素逐行滚动、底部钳制、横向滚动。

use bennu_theme::smooth_scroll::{Orientation, ScrollTarget};

use super::{refresh_retained_content, TextPreviewViewerState, VisibleRow};
use crate::text_document::TextPreviewDocument;

pub(super) fn window_span(document: &TextPreviewDocument) -> (u64, u64) {
    let rows = document.rows();
    match (rows.front(), rows.back()) {
        (Some(first), Some(last)) => (first.start, last.end),
        _ => (0, 0),
    }
}

/// 锚点行不在窗口里了（窗口被整个替换、两端被裁）：落到最近的一行。
pub(super) fn normalize_anchor(state: &mut TextPreviewViewerState, document: &TextPreviewDocument) {
    let rows = document.rows();
    if rows.is_empty() {
        state.anchor = 0;
        state.pixel = 0.0;
        return;
    }
    if let Some(index) = document.row_index_containing(state.anchor) {
        if rows[index].start != state.anchor {
            state.anchor = rows[index].start;
            state.pixel = 0.0;
        }
        return;
    }
    let index = match document.row_index(state.anchor) {
        Ok(index) | Err(index) => index.min(rows.len() - 1),
    };
    state.anchor = rows[index].start;
    state.pixel = 0.0;
}

pub(super) fn anchor_index(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> Option<usize> {
    document.row_index(state.anchor).ok()
}

pub(super) fn row_height(state: &TextPreviewViewerState, start: u64) -> f32 {
    state
        .heights
        .get(&start)
        .copied()
        .unwrap_or(state.line_height.max(1.0))
}

/// 量到的行高只增不减：排版器会裁掉视口外行的布局，观测值缩小时保留旧值，位置因此稳定。
pub(super) fn note_measured_height(state: &mut TextPreviewViewerState, start: u64, height: f32) {
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    let previous = row_height(state, start);
    // 量测与基准常量存在 1-2 ulp 表示差，微差视为无变化。
    if height > previous + 0.01 {
        state.heights.insert(start, height);
    }
}

/// 从锚点起收集可见行；行内偏移使首行可为部分行，由绘制裁剪。
pub(super) fn visible_rows(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> Vec<VisibleRow> {
    let Some(mut index) = anchor_index(state, document) else {
        return Vec::new();
    };
    let rows = document.rows();
    let limit = state.viewport_height;
    let mut entries = Vec::new();
    // 触控板拉过文件头尾时整体挪开画；绘制和点选都从这里取位置，挪开的也点得准。
    let mut y = -state.pixel - state.touch.displacement().y;
    while index < rows.len() && y < limit {
        let height = row_height(state, rows[index].start);
        if y + height > 0.0 {
            entries.push(VisibleRow {
                index,
                visible_y: y,
                height,
            });
        }
        y += height;
        index += 1;
    }
    entries
}

/// 按像素滚动：从锚点逐行走。走不出窗口（窗口外的行还没读到）；窗口到文件尾时，
/// 最后一行底边最多贴住文字视口底边。返回实际滚了多远（被夹住的不算），没动是 0。
pub(super) fn scroll_viewer_by(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    delta_y: f32,
) -> f32 {
    let rows = document.rows();
    let Some(mut index) = anchor_index(state, document) else {
        return 0.0;
    };
    let before = (state.anchor, state.pixel);
    let mut pixel = state.pixel + delta_y;
    while pixel > 0.0 && index + 1 < rows.len() {
        let height = row_height(state, rows[index].start);
        if pixel < height {
            break;
        }
        pixel -= height;
        index += 1;
    }
    while pixel < 0.0 && index > 0 {
        index -= 1;
        pixel += row_height(state, rows[index].start);
    }
    let mut clamped = pixel.min(0.0);
    pixel = pixel.max(0.0);
    // 底部钳制：窗口里锚点之后的行加起来不足一个视口时整体上移。
    let (max_index, max_pixel) = bottom_position(state, document);
    if (index, pixel) > (max_index, max_pixel) {
        let skipped: f32 = (max_index..index)
            .map(|row| row_height(state, rows[row].start))
            .sum();
        clamped = skipped + pixel - max_pixel;
        index = max_index;
        pixel = max_pixel;
    }
    state.anchor = rows[index].start;
    state.pixel = pixel;
    if (state.anchor, state.pixel) == before {
        return 0.0;
    }
    delta_y - clamped
}

/// 触控板滚动作用在查看器上。竖向只有文件头、文件尾是边：窗口外还没读进来的行不算，滚不动
/// 就停在那里等读。横向的边是 0 和已排版行的最大宽度；没有可滚的宽度（换行、没有超宽行）时
/// 不越界。越界的位移竖向由 [`visible_rows`]、横向由 `drawn_scroll_x` 加上，绘制和点选共用。
pub(super) struct TouchTarget<'a> {
    pub(super) state: &'a mut TextPreviewViewerState,
    pub(super) document: &'a TextPreviewDocument,
}

impl ScrollTarget for TouchTarget<'_> {
    fn scroll(&mut self, orientation: Orientation, delta: f32) -> (f32, f32) {
        match orientation {
            Orientation::Horizontal => {
                let before = self.state.scroll_x;
                scroll_viewer_horizontally(self.state, delta);
                let scrolled = self.state.scroll_x - before;
                let beyond = if self.state.max_scroll_x() > 0.0 {
                    delta - scrolled
                } else {
                    0.0
                };
                (scrolled, beyond)
            }
            Orientation::Vertical => {
                let scrolled = scroll_viewer_by(self.state, self.document, delta);
                if scrolled != 0.0 {
                    self.state.drag_fraction = None;
                    refresh_retained_content(self.state, self.document);
                }
                let clamped = delta - scrolled;
                let at_edge = if clamped < 0.0 {
                    self.document.at_start()
                } else {
                    self.document.at_end()
                };
                (scrolled, if at_edge { clamped } else { 0.0 })
            }
        }
    }

    fn extent(&self, orientation: Orientation) -> f32 {
        match orientation {
            Orientation::Horizontal => self.state.text_width(),
            Orientation::Vertical => self.state.viewport_height,
        }
    }
}

/// 不换行时横向滚动，夹在 [0, 最宽行 − 文字区宽] 之间。返回位置是否变了。
pub(super) fn scroll_viewer_horizontally(state: &mut TextPreviewViewerState, delta_x: f32) -> bool {
    let before = state.scroll_x;
    state.scroll_x = (state.scroll_x + delta_x).clamp(0.0, state.max_scroll_x());
    state.scroll_x != before
}

/// 能滚到的最远位置：从窗口最后一行往上累加行高，刚好填满文字视口。
pub(super) fn bottom_position(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> (usize, f32) {
    let rows = document.rows();
    let mut remaining = state.viewport_height;
    let mut index = rows.len();
    while index > 0 {
        let height = row_height(state, rows[index - 1].start);
        if height >= remaining {
            return (index - 1, height - remaining);
        }
        remaining -= height;
        index -= 1;
    }
    (0, 0.0)
}

pub(super) fn is_at_bottom(state: &TextPreviewViewerState, document: &TextPreviewDocument) -> bool {
    let Some(index) = anchor_index(state, document) else {
        return true;
    };
    let (max_index, max_pixel) = bottom_position(state, document);
    document.at_end() && (index, state.pixel + 0.5) >= (max_index, max_pixel)
}
