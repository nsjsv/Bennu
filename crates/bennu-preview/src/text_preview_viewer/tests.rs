use iced::advanced::widget::operation::scrollable::AbsoluteOffset;
use iced::advanced::widget::operation::Scrollable as _;
use iced::{Point, Rectangle};

use super::*;
use crate::text_document::{document_from_bytes, TextJump};

/// 文字视口 384 px（布局 400 减上下内边距）、文字宽度 376 px 的查看器状态。
fn synced_state(document: &TextPreviewDocument) -> TextPreviewViewerState {
    let mut state = TextPreviewViewerState::default();
    sync_with_document(&mut state, document, 392.0, 400.0);
    state
}

fn lines(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("line {index}\n"))
        .collect::<String>()
        .into_bytes()
}

/// 最后一个可见行的底边相对文字视口顶部的位置。
fn last_visible_bottom(
    state: &TextPreviewViewerState,
    document: &TextPreviewDocument,
) -> (usize, f32) {
    let visible = visible_rows(state, document);
    let last = visible.last().expect("visible rows");
    (last.index, last.visible_y + last.height)
}

#[test]
fn scrolling_to_the_end_puts_the_last_line_exactly_at_the_viewport_bottom() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let mut state = synced_state(&document);
    assert_eq!(state.viewport_height, 384.0);

    scroll_viewer_by(&mut state, &document, 1.0e6);
    let (index, bottom) = last_visible_bottom(&state, &document);
    assert_eq!(index, document.rows().len() - 1);
    assert!(
        (bottom - state.viewport_height).abs() < 0.01,
        "bottom {bottom}"
    );
    assert!(is_at_bottom(&state, &document));
    assert_eq!(scrollbar::scroll_fraction(&state, &document), 1.0);

    // 再往下滚不动；往回滚离开底部。
    assert!(!scroll_viewer_by(&mut state, &document, 50.0));
    assert!(scroll_viewer_by(&mut state, &document, -base_line_height()));
    assert!(!is_at_bottom(&state, &document));
}

#[test]
fn scrolling_up_stops_at_the_first_line() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let mut state = synced_state(&document);
    scroll_viewer_by(&mut state, &document, base_line_height() * 3.5);
    assert_eq!(state.anchor, document.rows()[3].start);
    assert!((state.pixel - base_line_height() * 0.5).abs() < 0.01);
    scroll_viewer_by(&mut state, &document, -1.0e6);
    assert_eq!((state.anchor, state.pixel), (0, 0.0));
    assert_eq!(scrollbar::scroll_fraction(&state, &document), 0.0);
}

#[test]
fn wrapped_rows_measure_taller_and_still_end_at_the_viewport_bottom() {
    let mut text = lines(40);
    text.extend_from_slice("word ".repeat(400).as_bytes());
    text.push(b'\n');
    let document = document_from_bytes(text, TextJump::Start);
    let mut state = synced_state(&document);
    // 滚到底再同步：最后的长行排版后量出真实高度，位置按新高度重新夹。
    scroll_viewer_by(&mut state, &document, 1.0e6);
    refresh_retained_content(&mut state, &document);
    scroll_viewer_by(&mut state, &document, 1.0e6);
    let last_start = document.rows().back().expect("last").start;
    assert!(row_height(&state, last_start) > base_line_height() * 2.0);
    let (_, bottom) = last_visible_bottom(&state, &document);
    assert!((bottom - state.viewport_height).abs() < 0.01);
}

#[test]
fn scrolling_stops_at_the_window_edge_and_reports_the_visible_range() {
    let document = document_from_bytes(lines(200_000), TextJump::Start);
    assert!(!document.at_end());
    let mut state = synced_state(&document);
    scroll_viewer_by(&mut state, &document, 1.0e7);
    // 窗口外的行还没读到：最后一个窗口行贴住视口底部，不越界。
    let (index, bottom) = last_visible_bottom(&state, &document);
    assert_eq!(index, document.rows().len() - 1);
    assert!((bottom - state.viewport_height).abs() < 0.01);
    assert!(!is_at_bottom(&state, &document));

    queue_viewport_report(&mut state, &document);
    let Some(TextViewerEvent::ViewportChanged(viewport)) = state.pending_events.pop() else {
        panic!("viewport report");
    };
    assert_eq!(
        viewport.last_row,
        document.rows().back().expect("last").start
    );
    assert!(!viewport.at_bottom);
    // 字节比例：在窗口末尾，离文件末尾还很远。
    assert!(scrollbar::scroll_fraction(&state, &document) < 0.2);
}

#[test]
fn a_jump_from_the_document_moves_the_anchor_to_the_target_row() {
    let bytes = lines(200_000);
    let len = bytes.len() as u64;
    let mut document = document_from_bytes(bytes, TextJump::Start);
    let mut state = synced_state(&document);

    let reads = document.jump_to(TextJump::Offset(len / 2));
    apply_reads(&mut document, reads);
    sync_with_document(&mut state, &document, 392.0, 400.0);
    let jump = document.jump().expect("jump");
    assert_eq!(state.anchor, jump.row);
    assert_eq!(state.pixel, 0.0);
    let fraction = scrollbar::scroll_fraction(&state, &document);
    assert!((fraction - 0.5).abs() < 0.01, "fraction {fraction}");

    // 跳到末尾：贴住底部。
    let reads = document.jump_to(TextJump::End);
    apply_reads(&mut document, reads);
    sync_with_document(&mut state, &document, 392.0, 400.0);
    assert!(is_at_bottom(&state, &document));
    assert_eq!(scrollbar::scroll_fraction(&state, &document), 1.0);
}

fn apply_reads(document: &mut TextPreviewDocument, reads: Vec<crate::text_document::TextRead>) {
    for read in reads {
        if let crate::text_document::TextRead::Rows {
            generation,
            request,
        } = read
        {
            let source = document.source();
            let outcome = crate::text_document::read_rows(
                source.as_ref(),
                document.decoding(),
                document.len(),
                request,
            )
            .map_err(|error| error.to_string());
            document.accept_rows(generation, request, outcome);
        }
    }
}

#[test]
fn wheel_scroll_by_operation_moves_the_viewer() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let mut state = synced_state(&document);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(400.0, 400.0));
    let mut bridge = ScrollBridge {
        state: &mut state,
        document: &document,
    };
    bridge.scroll_by(
        AbsoluteOffset {
            x: 0.0,
            y: base_line_height() * 2.0,
        },
        bounds,
        bounds,
    );
    assert_eq!(state.anchor, document.rows()[2].start);
}

#[test]
fn dragging_the_thumb_in_a_large_file_requests_a_proportional_jump() {
    let document = document_from_bytes(lines(200_000), TextJump::Start);
    let mut state = synced_state(&document);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(400.0, 400.0));
    let track_x = bounds.width - 2.0;
    let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let cursor = mouse::Cursor::Available(Point::new(track_x, 200.0));
    assert!(scrollbar::handle_event(
        &mut state, &document, &press, bounds, cursor
    ));
    let Some(TextViewerEvent::JumpRequested(TextJump::Offset(offset))) = state.pending_events.pop()
    else {
        panic!("jump request");
    };
    let fraction = offset as f64 / document.len() as f64;
    assert!((fraction - 0.5).abs() < 0.05, "fraction {fraction}");
    // 拖到底：跳到文件末尾。
    let moved = Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(track_x, 1_000.0),
    });
    assert!(scrollbar::handle_event(
        &mut state, &document, &moved, bounds, cursor
    ));
    assert_eq!(
        state.pending_events.pop(),
        Some(TextViewerEvent::JumpRequested(TextJump::End))
    );
    let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
    assert!(scrollbar::handle_event(
        &mut state, &document, &release, bounds, cursor
    ));
}

#[test]
fn dragging_the_thumb_in_a_loaded_file_scrolls_by_pixels() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let mut state = synced_state(&document);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(400.0, 400.0));
    let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let cursor = mouse::Cursor::Available(Point::new(bounds.width - 2.0, 399.0));
    assert!(scrollbar::handle_event(
        &mut state, &document, &press, bounds, cursor
    ));
    assert!(state.pending_events.is_empty());
    assert!(is_at_bottom(&state, &document));
}

#[test]
fn selection_text_joins_segments_without_breaks_and_lines_with_newlines() {
    let long = "x".repeat(25_000);
    let text = format!("first\n{long}\nlast\n");
    let document = document_from_bytes(text.into_bytes(), TextJump::Start);
    let rows = document.rows();
    assert!(rows.len() >= 5);
    let from = TextPosition {
        row: rows[0].start,
        column: 2,
    };
    let to = TextPosition {
        row: document.len(),
        column: 0,
    };
    assert_eq!(
        selection::selection_text(&document, from, to).as_deref(),
        Some(format!("rst\n{long}\nlast").as_str())
    );
    // 反向选择结果相同。
    assert_eq!(
        selection::selection_text(&document, to, from),
        selection::selection_text(&document, from, to)
    );
}

#[test]
fn selections_reaching_outside_the_window_are_left_to_the_engine() {
    let document = document_from_bytes(lines(200_000), TextJump::Start);
    let from = TextPosition { row: 0, column: 0 };
    let to = TextPosition {
        row: document.len(),
        column: 0,
    };
    assert_eq!(selection::selection_text(&document, from, to), None);
}

#[test]
fn keyboard_motions_move_through_rows_and_reach_the_document_edges() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let rows = document.rows();
    let start = TextPosition {
        row: rows[0].start,
        column: 4,
    };
    let down = selection::move_position(&document, start, text_editor::Motion::Down, 10);
    assert_eq!(
        down,
        TextPosition {
            row: rows[1].start,
            column: 4
        }
    );
    let page = selection::move_position(&document, start, text_editor::Motion::PageDown, 10);
    assert_eq!(page.row, rows[9].start);
    let end = selection::move_position(&document, start, text_editor::Motion::DocumentEnd, 10);
    assert_eq!(
        end,
        TextPosition {
            row: document.len(),
            column: 0
        }
    );
    let left = selection::move_position(&document, down, text_editor::Motion::Home, 10);
    assert_eq!(left.column, 0);
    let wrapped = selection::move_position(&document, left, text_editor::Motion::Left, 10);
    assert_eq!(
        wrapped,
        TextPosition {
            row: rows[0].start,
            column: rows[0].text.len()
        }
    );
}

#[test]
fn word_range_covers_same_class_run() {
    assert_eq!(selection::word_range("hello world", 2), (0, 5));
    assert_eq!(selection::word_range("hello world", 5), (5, 6));
    assert_eq!(selection::word_range("a.b", 1), (1, 2));
}

#[test]
fn a_new_document_resets_position_and_selection() {
    let document = document_from_bytes(lines(100), TextJump::Start);
    let mut state = synced_state(&document);
    scroll_viewer_by(&mut state, &document, 200.0);
    state.cursor = Some(TextPosition { row: 6, column: 1 });
    let mut other = document_from_bytes(lines(5), TextJump::Start);
    // 不同文档：id 不同。
    other = TextPreviewDocument::reidentified_for_tests(other, 2);
    sync_with_document(&mut state, &other, 392.0, 400.0);
    assert_eq!(state.anchor, 0);
    assert_eq!(state.cursor, None);
}
