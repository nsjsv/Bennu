use std::time::{Duration, Instant};

use super::*;

fn pixels(y: f32) -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Pixels { x: 0.0, y },
    })
}

/// 按 60 Hz 推进到不再要重画为止。
pub(super) fn settle(state: &mut TextPreviewViewerState, document: &TextPreviewDocument) {
    let mut clipboard = RecordingClipboard::default();
    let start = Instant::now();
    for frame in 1..2_000 {
        let now = start + Duration::from_micros(16_667 * frame);
        let tick = Event::Window(iced::window::Event::RedrawRequested(now));
        if send(state, document, tick, at(100.0, 100.0), &mut clipboard)
            .redraw
            .is_none()
        {
            return;
        }
    }
    panic!("the motion never came to rest");
}

fn first_row_top(state: &TextPreviewViewerState, document: &TextPreviewDocument) -> f32 {
    visible_rows(state, document)
        .first()
        .expect("visible rows")
        .visible_y
}

/// 在文件头往上拉：位置不动，行整体往下挪开画；松手弹回原位。
#[test]
fn pulling_past_the_start_of_the_file_springs_back() {
    let document = document_from_bytes(lines(400), TextJump::Start);
    let mut state = synced_state(&document);
    let mut clipboard = RecordingClipboard::default();
    for _ in 0..4 {
        send(
            &mut state,
            &document,
            pixels(20.0),
            at(100.0, 100.0),
            &mut clipboard,
        );
    }
    assert_eq!((state.anchor, state.pixel), (0, 0.0));
    assert!(first_row_top(&state, &document) > 10.0);

    send(
        &mut state,
        &document,
        pixels(0.0),
        at(100.0, 100.0),
        &mut clipboard,
    );
    settle(&mut state, &document);
    assert_eq!(first_row_top(&state, &document), 0.0);
    assert!(!state.touch.is_active());
}

/// 手指划一下松开：接着滑一段再停。
#[test]
fn a_touchpad_flick_keeps_coasting_after_the_fingers_lift() {
    let document = document_from_bytes(lines(4_000), TextJump::Start);
    let mut state = synced_state(&document);
    let mut clipboard = RecordingClipboard::default();
    for _ in 0..6 {
        send(
            &mut state,
            &document,
            pixels(-10.0),
            at(100.0, 100.0),
            &mut clipboard,
        );
        std::thread::sleep(Duration::from_millis(8));
    }
    let lifted_at = (state.anchor, state.pixel);

    send(
        &mut state,
        &document,
        pixels(0.0),
        at(100.0, 100.0),
        &mut clipboard,
    );
    settle(&mut state, &document);
    assert!((state.anchor, state.pixel) > lifted_at);
    assert!(!state.touch.is_active());
}

/// 按着 Ctrl 时手指离开不是一次缩小字号（手势结束没有方向）。
#[test]
fn lifting_the_fingers_with_ctrl_held_does_not_zoom() {
    let document = document_from_bytes(lines(20), TextJump::Start);
    let mut state = synced_state(&document);
    state.modifiers = keyboard::Modifiers::CTRL;
    let mut clipboard = RecordingClipboard::default();
    send(
        &mut state,
        &document,
        pixels(0.0),
        at(100.0, 100.0),
        &mut clipboard,
    );
    assert!(state.pending_events.is_empty());
}
