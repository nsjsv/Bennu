//! 查看器自己的滚动条：报告给细线滑块的虚拟几何，以及拖动/点击轨道。
//!
//! 整个文件都在窗口里时按像素算（与普通滚动区一致）；否则按字节比例：视口顶部的字节
//! 占「文件长 − 视口内字节」的比例，到底时正好是 1。拖到哪个比例就跳到哪个字节，不需要
//! 知道总行数，也不受 f32 在几亿像素量级上的精度限制。

use bennu_theme::scrollbar::scrollbar_thumb_span;
use bennu_theme::styles::SCROLLBAR_HIT_WIDTH;
use iced::{mouse, Event, Padding, Point, Rectangle};

use super::{
    anchor_index, base_line_height, is_at_bottom, refresh_retained_content, row_height,
    scroll_viewer_by, visible_rows, TextPreviewViewerState, TEXT_PREVIEW_VIEWER_PADDING,
};
use crate::text_document::{TextJump, TextPreviewDocument, TextViewerEvent};

/// 拖滑块：按下点在滑块里的位置，拖动时保持不变。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ThumbDrag {
    grab: f32,
}

/// 报告给细线滑块的虚拟内容高与偏移。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct VirtualGeometry {
    pub(super) content_height: f32,
    pub(super) offset: f32,
}

pub(super) fn virtual_geometry(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    layout_height: f32,
) -> VirtualGeometry {
    let padding = Padding::new(TEXT_PREVIEW_VIEWER_PADDING).y();
    let window_height: f32 = document
        .rows()
        .iter()
        .map(|row| row_height(state, row.start))
        .sum();
    let content_height = if is_whole_file_loaded(document) {
        window_height + padding
    } else {
        (document.estimated_total_rows() as f32 * base_line_height()).max(window_height) + padding
    };
    let fraction = state
        .drag_fraction
        .unwrap_or_else(|| scroll_fraction(state, document));
    VirtualGeometry {
        content_height,
        offset: fraction * (content_height - layout_height).max(0.0),
    }
}

fn is_whole_file_loaded(document: &TextPreviewDocument) -> bool {
    document.at_start() && document.at_end()
}

/// 当前位置占可滚范围的比例。
pub(super) fn scroll_fraction(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> f32 {
    let Some(index) = anchor_index(state, document) else {
        return 0.0;
    };
    if is_at_bottom(state, document) && !(document.at_start() && index == 0 && state.pixel == 0.0) {
        return 1.0;
    }
    let rows = document.rows();
    if is_whole_file_loaded(document) {
        let above: f32 = rows
            .iter()
            .take(index)
            .map(|row| row_height(state, row.start))
            .sum::<f32>()
            + state.pixel;
        let total: f32 = rows.iter().map(|row| row_height(state, row.start)).sum();
        let range = (total - state.viewport_height).max(f32::EPSILON);
        return (above / range).clamp(0.0, 1.0);
    }
    let row = &rows[index];
    let row_fraction = (state.pixel / row_height(state, row.start)).clamp(0.0, 1.0) as f64;
    let top =
        (row.start - document.data_start()) as f64 + row_fraction * (row.end - row.start) as f64;
    let range = (document.len() - document.data_start())
        .saturating_sub(viewport_bytes(state, document))
        .max(1);
    (top / range as f64).clamp(0.0, 1.0) as f32
}

/// 视口里的行占多少字节。
fn viewport_bytes(state: &TextPreviewViewerState, document: &TextPreviewDocument) -> u64 {
    let visible = visible_rows(state, document);
    let rows = document.rows();
    match (visible.first(), visible.last()) {
        (Some(first), Some(last)) => rows[last.index].end - rows[first.index].start,
        _ => 0,
    }
}

pub(super) fn is_over_track(bounds: Rectangle, cursor: mouse::Cursor) -> bool {
    cursor
        .position()
        .is_some_and(|position| track_contains(bounds, position))
}

fn track_contains(bounds: Rectangle, position: Point) -> bool {
    bounds.contains(position) && position.x >= bounds.x + bounds.width - SCROLLBAR_HIT_WIDTH
}

/// 处理滑块的按下、拖动、松开；返回事件是否被滑块用掉。
pub(super) fn handle_event(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> bool {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
            let Some(position) = cursor
                .position()
                .filter(|position| track_contains(bounds, *position))
            else {
                return false;
            };
            let geometry = virtual_geometry(state, document, bounds.height);
            let Some((thumb_start, thumb_length)) = scrollbar_thumb_span(
                geometry.offset,
                bounds.height,
                geometry.content_height,
                bounds.height,
            ) else {
                return false;
            };
            let y = position.y - bounds.y;
            // 按在滑块上保持抓取点；按在轨道上让滑块中心跳到指针处。
            let grab = if (thumb_start..=thumb_start + thumb_length).contains(&y) {
                y - thumb_start
            } else {
                thumb_length / 2.0
            };
            state.thumb_drag = Some(ThumbDrag { grab });
            drag_thumb_to(state, document, bounds, y);
            true
        }
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            if state.thumb_drag.is_none() {
                return false;
            }
            drag_thumb_to(state, document, bounds, position.y - bounds.y);
            true
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            state.thumb_drag.take().is_some()
        }
        _ => false,
    }
}

fn drag_thumb_to(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    y: f32,
) {
    let Some(drag) = state.thumb_drag else {
        return;
    };
    let geometry = virtual_geometry(state, document, bounds.height);
    let Some((_, thumb_length)) =
        scrollbar_thumb_span(0.0, bounds.height, geometry.content_height, bounds.height)
    else {
        return;
    };
    let travel = (bounds.height - thumb_length).max(f32::EPSILON);
    let fraction = ((y - drag.grab) / travel).clamp(0.0, 1.0);
    if is_whole_file_loaded(document) {
        scroll_to_fraction(state, document, fraction);
        return;
    }
    state.drag_fraction = Some(fraction);
    let jump = if fraction <= 0.0 {
        TextJump::Start
    } else if fraction >= 1.0 {
        TextJump::End
    } else {
        let range = (document.len() - document.data_start())
            .saturating_sub(viewport_bytes(state, document)) as f64;
        TextJump::Offset(document.data_start() + (fraction as f64 * range) as u64)
    };
    state
        .pending_events
        .push(TextViewerEvent::JumpRequested(jump));
}

/// 整个文件都在窗口里：直接按像素滚到比例处。
fn scroll_to_fraction(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    fraction: f32,
) {
    let Some(first) = document.rows().front() else {
        return;
    };
    let total: f32 = document
        .rows()
        .iter()
        .map(|row| row_height(state, row.start))
        .sum();
    state.anchor = first.start;
    state.pixel = 0.0;
    state.drag_fraction = None;
    scroll_viewer_by(
        state,
        document,
        fraction * (total - state.viewport_height).max(0.0),
    );
    refresh_retained_content(state, document);
}
