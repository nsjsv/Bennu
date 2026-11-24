use std::time::Duration;

use super::touch_tests::settle;
use super::*;

/// 5 个短行、一个 400 字的长行、再 300 个短行：不换行时能横向滚，竖向也能滚。
fn long_row_document() -> TextPreviewDocument {
    let mut text = lines(5);
    text.extend_from_slice("x".repeat(400).as_bytes());
    text.push(b'\n');
    text.extend(lines(300));
    document_from_bytes(text, TextJump::Start)
}

fn state_with_wrap(document: &TextPreviewDocument, wrap: bool) -> TextPreviewViewerState {
    let mut view = TextViewSettings::default();
    if wrap != view.wrap {
        view = view.apply(TextViewChange::ToggleWrap);
    }
    let mut state = TextPreviewViewerState::default();
    sync_with_document(
        &mut state,
        document,
        view,
        HighlightPalette::Light,
        Size::new(480.0, 400.0),
    );
    state
}

fn pixels(x: f32, y: f32) -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Pixels { x, y },
    })
}

fn lines_wheel(x: f32, y: f32) -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x, y },
    })
}

fn modifiers(modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers))
}

fn deliver(state: &mut TextPreviewViewerState, document: &TextPreviewDocument, event: Event) {
    send(
        state,
        document,
        event,
        at(100.0, 100.0),
        &mut RecordingClipboard::default(),
    );
}

/// 触控板往两边划：横向位置跟着手指走，只在 0 和最宽行之间；拉过两头时整体挪开画，
/// 松手弹回；竖向位置一动不动。
#[test]
fn sideways_touchpad_swipes_scroll_the_long_rows_and_stop_at_both_ends() {
    let document = long_row_document();
    let mut state = state_with_wrap(&document, false);
    let max = state.max_scroll_x();
    assert!(max > 0.0);

    deliver(&mut state, &document, pixels(-40.0, 0.0));
    assert_eq!(state.scroll_x, 40.0);
    deliver(&mut state, &document, pixels(15.0, 0.0));
    assert_eq!(state.scroll_x, 25.0);
    assert_eq!((state.anchor, state.pixel), (0, 0.0));

    deliver(&mut state, &document, pixels(-1.0e6, 0.0));
    assert_eq!(state.scroll_x, max);
    assert!(state.drawn_scroll_x() > max);
    deliver(&mut state, &document, pixels(0.0, 0.0));
    settle(&mut state, &document);
    assert_eq!((state.scroll_x, state.drawn_scroll_x()), (max, max));

    deliver(&mut state, &document, pixels(1.0e6, 0.0));
    assert_eq!(state.scroll_x, 0.0);
    assert!(state.drawn_scroll_x() < 0.0);
    deliver(&mut state, &document, pixels(0.0, 0.0));
    settle(&mut state, &document);
    assert_eq!((state.scroll_x, state.drawn_scroll_x()), (0.0, 0.0));
    assert_eq!((state.anchor, state.pixel), (0, 0.0));
}

/// 斜着划两个方向一起动；松手后横向也接着滑一段。
#[test]
fn a_diagonal_swipe_moves_both_axes_and_the_sideways_part_coasts() {
    let document = long_row_document();
    let mut state = state_with_wrap(&document, false);
    for _ in 0..6 {
        deliver(&mut state, &document, pixels(-10.0, -6.0));
        std::thread::sleep(Duration::from_millis(8));
    }
    assert_eq!(state.scroll_x, 60.0);
    assert!((state.anchor, state.pixel) > (0, 0.0));
    let lifted_at = state.scroll_x;

    deliver(&mut state, &document, pixels(0.0, 0.0));
    settle(&mut state, &document);
    assert!(state.scroll_x > lifted_at);
}

/// Shift+竖向滚轮横滚（走滚轮惯性），不带 Shift 的竖向滚轮还是竖滚；倾斜滚轮的横向格数横滚。
#[test]
fn shift_wheel_scrolls_sideways_and_the_plain_wheel_still_scrolls_down() {
    let document = long_row_document();
    let mut state = state_with_wrap(&document, false);
    let max = state.max_scroll_x();

    deliver(&mut state, &document, lines_wheel(0.0, -3.0));
    settle(&mut state, &document);
    assert_eq!(state.scroll_x, 0.0);
    assert!((state.anchor, state.pixel) > (0, 0.0));
    let scrolled_down = (state.anchor, state.pixel);

    deliver(&mut state, &document, modifiers(keyboard::Modifiers::SHIFT));
    deliver(&mut state, &document, lines_wheel(0.0, -1.0e3));
    settle(&mut state, &document);
    assert_eq!(state.scroll_x, max);
    assert_eq!((state.anchor, state.pixel), scrolled_down);
    deliver(&mut state, &document, lines_wheel(0.0, 1.0e3));
    settle(&mut state, &document);
    assert_eq!(state.scroll_x, 0.0);

    deliver(
        &mut state,
        &document,
        modifiers(keyboard::Modifiers::empty()),
    );
    deliver(&mut state, &document, lines_wheel(-2.0, 0.0));
    settle(&mut state, &document);
    assert!(state.scroll_x > 0.0);
    assert_eq!((state.anchor, state.pixel), scrolled_down);
}

/// 换行时没有横向可滚：触控板横划、Shift+滚轮都不动，也不拉过边缘；竖向照常滚。
#[test]
fn wrapping_ignores_horizontal_input_but_keeps_scrolling_vertically() {
    let document = long_row_document();
    let mut state = state_with_wrap(&document, true);
    assert_eq!(state.max_scroll_x(), 0.0);

    deliver(&mut state, &document, pixels(-80.0, 0.0));
    assert_eq!(state.scroll_x, 0.0);
    assert_eq!(state.touch.displacement(), Vector::ZERO);
    deliver(&mut state, &document, pixels(0.0, 0.0));
    settle(&mut state, &document);

    deliver(&mut state, &document, modifiers(keyboard::Modifiers::SHIFT));
    deliver(&mut state, &document, lines_wheel(0.0, -3.0));
    settle(&mut state, &document);
    assert_eq!(state.scroll_x, 0.0);
    assert_eq!((state.anchor, state.pixel), (0, 0.0));

    deliver(
        &mut state,
        &document,
        modifiers(keyboard::Modifiers::empty()),
    );
    deliver(&mut state, &document, pixels(-80.0, -30.0));
    assert_eq!(state.scroll_x, 0.0);
    assert_eq!(state.touch.displacement(), Vector::ZERO);
    assert!((state.anchor, state.pixel) > (0, 0.0));
}

/// 不换行但没有超宽行：同样没有横向可滚，不拉过边缘。
#[test]
fn without_a_wide_row_there_is_nothing_to_scroll_sideways() {
    let document = document_from_bytes(lines(40), TextJump::Start);
    let mut state = state_with_wrap(&document, false);
    assert_eq!(state.max_scroll_x(), 0.0);

    deliver(&mut state, &document, pixels(-80.0, 0.0));
    assert_eq!(state.scroll_x, 0.0);
    assert_eq!(state.touch.displacement(), Vector::ZERO);
}
