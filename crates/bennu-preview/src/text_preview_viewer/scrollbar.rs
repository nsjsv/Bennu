//! 查看器自己的滚动条：报告给细线滑块的虚拟几何，以及拖动/点击轨道。
//!
//! 整个文件都在窗口里时按像素算（与普通滚动区一致）；否则按字节比例：视口顶部的字节
//! 占「文件长 − 视口内字节」的比例，到底时正好是 1。拖到哪个比例就跳到哪个字节，不需要
//! 知道总行数，也不受 f32 在几亿像素量级上的精度限制。

use bennu_theme::scrollbar::scrollbar_thumb_span;
use bennu_theme::styles::SCROLLBAR_HIT_WIDTH;
use iced::{mouse, Event, Padding, Point, Rectangle};

use super::{
    anchor_index, is_at_bottom, refresh_retained_content, row_height, scroll_viewer_by,
    visible_rows, TextPreviewViewerState, TEXT_PREVIEW_VIEWER_PADDING,
};
use crate::text_document::{TextJump, TextPreviewDocument, TextViewerEvent};

/// 拖滑块：按下点在滑块里的位置，拖动时保持不变。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ThumbDrag {
    axis: Axis,
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
        (document.estimated_total_rows() as f32 * state.line_height.max(1.0)).max(window_height)
            + padding
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

pub(super) fn is_over_track(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> bool {
    cursor
        .position()
        .is_some_and(|position| track_at(state, document, bounds, position).is_some())
}

/// 竖向轨道贴右缘、横向轨道贴下缘（不换行且有超宽行时才有）；两向都能滚时竖向让出
/// 右下角，与细线滑块的轨道一致。
fn track_bounds(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    axis: Axis,
) -> Option<Rectangle> {
    let vertical_overflow =
        virtual_geometry(state, document, bounds.height).content_height > bounds.height;
    let horizontal_overflow = state.max_scroll_x() > 0.0;
    match axis {
        Axis::Vertical if vertical_overflow => {
            let corner = if horizontal_overflow {
                SCROLLBAR_HIT_WIDTH
            } else {
                0.0
            };
            Some(Rectangle {
                x: bounds.x + bounds.width - SCROLLBAR_HIT_WIDTH,
                y: bounds.y,
                width: SCROLLBAR_HIT_WIDTH,
                height: (bounds.height - corner).max(0.0),
            })
        }
        Axis::Horizontal if horizontal_overflow => {
            let corner = if vertical_overflow {
                SCROLLBAR_HIT_WIDTH
            } else {
                0.0
            };
            Some(Rectangle {
                x: bounds.x,
                y: bounds.y + bounds.height - SCROLLBAR_HIT_WIDTH,
                width: (bounds.width - corner).max(0.0),
                height: SCROLLBAR_HIT_WIDTH,
            })
        }
        _ => None,
    }
}

fn track_at(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    position: Point,
) -> Option<(Axis, Rectangle)> {
    [Axis::Vertical, Axis::Horizontal]
        .into_iter()
        .find_map(|axis| {
            track_bounds(state, document, bounds, axis)
                .filter(|track| track.contains(position))
                .map(|track| (axis, track))
        })
}

/// 滑块在轨道上的起点与长度。
fn thumb_span(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    axis: Axis,
    track: Rectangle,
) -> Option<(f32, f32)> {
    match axis {
        Axis::Vertical => {
            let geometry = virtual_geometry(state, document, bounds.height);
            scrollbar_thumb_span(
                geometry.offset,
                bounds.height,
                geometry.content_height,
                track.height,
            )
        }
        Axis::Horizontal => scrollbar_thumb_span(
            state.scroll_x,
            bounds.width,
            bounds.width + state.max_scroll_x(),
            track.width,
        ),
    }
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
            let Some((position, (axis, track))) = cursor.position().and_then(|position| {
                Some((position, track_at(state, document, bounds, position)?))
            }) else {
                return false;
            };
            let Some((thumb_start, thumb_length)) =
                thumb_span(state, document, bounds, axis, track)
            else {
                return false;
            };
            let along = along_track(axis, track, position);
            // 按在滑块上保持抓取点；按在轨道上让滑块中心跳到指针处。
            let grab = if (thumb_start..=thumb_start + thumb_length).contains(&along) {
                along - thumb_start
            } else {
                thumb_length / 2.0
            };
            state.thumb_drag = Some(ThumbDrag { axis, grab });
            drag_thumb_to(state, document, bounds, position);
            true
        }
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            if state.thumb_drag.is_none() {
                return false;
            }
            drag_thumb_to(state, document, bounds, *position);
            true
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            state.thumb_drag.take().is_some()
        }
        _ => false,
    }
}

fn along_track(axis: Axis, track: Rectangle, position: Point) -> f32 {
    match axis {
        Axis::Vertical => position.y - track.y,
        Axis::Horizontal => position.x - track.x,
    }
}

fn drag_thumb_to(
    state: &mut TextPreviewViewerState,
    document: &TextPreviewDocument,
    bounds: Rectangle,
    position: Point,
) {
    let Some(drag) = state.thumb_drag else {
        return;
    };
    let Some(track) = track_bounds(state, document, bounds, drag.axis) else {
        return;
    };
    let Some((_, thumb_length)) = thumb_span(state, document, bounds, drag.axis, track) else {
        return;
    };
    let track_length = match drag.axis {
        Axis::Vertical => track.height,
        Axis::Horizontal => track.width,
    };
    let travel = (track_length - thumb_length).max(f32::EPSILON);
    let fraction = ((along_track(drag.axis, track, position) - drag.grab) / travel).clamp(0.0, 1.0);
    if drag.axis == Axis::Horizontal {
        state.scroll_x = fraction * state.max_scroll_x();
        return;
    }
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
