use iced::advanced::clipboard;
use iced::{mouse, Event, Point, Size};

use super::super::interaction;
use super::super::{sync_with_document, visible_rows, TextPreviewViewerState};
use super::*;
use crate::text_document::test_support::settle;
use crate::text_document::{document_from_bytes, TextJump, TextPosition, TextViewerEvent};
use crate::text_view::TextViewSettings;

fn marked_lines(count: usize, marked: &[usize]) -> Vec<u8> {
    (0..count)
        .map(|index| {
            let mark = if marked.contains(&index) { " mark" } else { "" };
            format!("line {index}{mark}\n")
        })
        .collect::<String>()
        .into_bytes()
}

fn sync(state: &mut TextPreviewViewerState, document: &TextPreviewDocument) {
    sync_with_document(
        state,
        document,
        TextViewSettings::default(),
        Size::new(480.0, 400.0),
    );
}

fn press(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        modified_key: key.clone(),
        key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

fn send(state: &mut TextPreviewViewerState, document: &TextPreviewDocument, event: Event) -> bool {
    interaction::handle_event(
        state,
        document,
        &event,
        Rectangle::new(Point::ORIGIN, Size::new(480.0, 400.0)),
        mouse::Cursor::Available(Point::new(10.0, 10.0)),
        &mut clipboard::Null,
    )
    .captured
}

fn search(document: &mut TextPreviewDocument, text: &str) {
    let reads = document.open_search(None);
    settle(document, reads);
    let reads = document.set_search_text(text.to_owned());
    settle(document, reads);
}

#[test]
fn escape_closes_the_find_bar_before_clearing_the_selection() {
    let mut document = document_from_bytes(marked_lines(20, &[]), TextJump::Start);
    document.open_goto();
    let mut state = TextPreviewViewerState::default();
    sync(&mut state, &document);
    state.is_focused = true;
    state.selection_anchor = Some(TextPosition { row: 0, column: 0 });
    state.cursor = Some(TextPosition { row: 0, column: 4 });
    let escape = || {
        press(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::empty(),
        )
    };
    assert!(send(&mut state, &document, escape()));
    assert_eq!(
        state.pending_events.pop(),
        Some(TextViewerEvent::Find(FindEvent::Close))
    );
    assert_ne!(state.selection_anchor, state.cursor, "the selection stays");
    document.close_find();
    assert!(send(&mut state, &document, escape()));
    assert_eq!(state.selection_anchor, state.cursor);
}

#[test]
fn find_shortcuts_open_the_bars_and_step() {
    let document = document_from_bytes(marked_lines(20, &[]), TextJump::Start);
    let mut state = TextPreviewViewerState::default();
    sync(&mut state, &document);
    state.is_focused = true;
    // 选中一行里的「line 3」：Ctrl+F 拿它当搜索词。
    let row = document.rows()[3].start;
    state.selection_anchor = Some(TextPosition { row, column: 0 });
    state.cursor = Some(TextPosition { row, column: 6 });
    let ctrl = keyboard::Modifiers::CTRL;
    assert!(send(
        &mut state,
        &document,
        press(keyboard::Key::Character("f".into()), ctrl)
    ));
    assert_eq!(
        state.pending_events.pop(),
        Some(TextViewerEvent::Find(FindEvent::OpenSearch {
            seed: Some("line 3".to_owned())
        }))
    );
    assert!(send(
        &mut state,
        &document,
        press(keyboard::Key::Character("g".into()), ctrl)
    ));
    assert_eq!(
        state.pending_events.pop(),
        Some(TextViewerEvent::Find(FindEvent::OpenGoto))
    );
    let f3 = || keyboard::Key::Named(keyboard::key::Named::F3);
    assert!(send(
        &mut state,
        &document,
        press(f3(), keyboard::Modifiers::SHIFT)
    ));
    assert_eq!(
        state.pending_events.pop(),
        Some(TextViewerEvent::Find(FindEvent::Step(
            SearchDirection::Backward
        )))
    );
}

#[test]
fn a_revealed_match_moves_into_view_a_third_down_and_gets_the_cursor() {
    let mut document = document_from_bytes(marked_lines(500, &[5, 300]), TextJump::Start);
    let mut state = TextPreviewViewerState::default();
    sync(&mut state, &document);
    search(&mut document, "mark");
    sync(&mut state, &document);
    // 第一个匹配本来就看得见：不动。
    assert_eq!(state.anchor, document.rows()[0].start);
    let first = document.current_match().expect("first");
    assert_eq!(state.cursor, Some(first.start));

    let reads = document.step_search(SearchDirection::Forward);
    settle(&mut document, reads);
    sync(&mut state, &document);
    let current = document.current_match().expect("second");
    assert_eq!(state.cursor, Some(current.start));
    let visible = visible_rows(&state, &document);
    let entry = visible
        .iter()
        .find(|entry| document.rows()[entry.index].start == current.start.row)
        .expect("the match is visible");
    let third = state.viewport_height / 3.0;
    assert!(
        (entry.visible_y - third).abs() <= state.line_height,
        "at {} (a third is {third})",
        entry.visible_y
    );
    // 当前匹配的底色与其他匹配不同。
    let retained = state
        .retained_rows
        .iter()
        .find(|row| row.start == current.start.row)
        .expect("laid out");
    assert_eq!(retained.marks.len(), 1);
    assert!(retained.marks[0].current);
}
