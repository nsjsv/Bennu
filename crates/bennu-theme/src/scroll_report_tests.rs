use iced::advanced::clipboard;
use iced::advanced::layout::Limits;
use iced::advanced::widget::operation::scrollable::{scroll_to, AbsoluteOffset};
use iced::keyboard::{key, Key, Location, Modifiers};
use iced::widget::{mouse_area, scrollable, Column, Space};
use iced::Point;

use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Note {
    At(f32),
    Entered(usize),
}

const ROW: f32 = 100.0;

struct Harness {
    element: Element<'static, Note, iced::Theme, ()>,
    tree: Tree,
    node: layout::Node,
    cursor: mouse::Cursor,
}

impl Harness {
    /// 20 行、每行 100 像素、可视 100 像素的列表；光标给了就压在可视区中间。
    fn new(quiet: Range<f32>, cursor_over_rows: bool) -> Self {
        let rows = Column::with_children((0..20).map(|index| {
            mouse_area(Space::new().width(Length::Fill).height(ROW))
                .on_enter(Note::Entered(index))
                .into()
        }));
        let mut element = report_scroll(
            scrollable(rows).id(widget::Id::new("list")).height(ROW),
            quiet,
            |position| Note::At(position.offset.y),
        );
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, ROW)),
        );
        let cursor = if cursor_over_rows {
            mouse::Cursor::Available(Point::new(50.0, 50.0))
        } else {
            mouse::Cursor::Unavailable
        };
        Self {
            element,
            tree,
            node,
            cursor,
        }
    }

    fn send(&mut self, event: Event) -> (Vec<Note>, window::RedrawRequest) {
        let mut notes = Vec::new();
        let mut shell = Shell::new(&mut notes);
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            self.cursor,
            &(),
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::new(Point::ORIGIN, Size::new(100.0, ROW)),
        );
        let redraw = shell.redraw_request();
        (notes, redraw)
    }

    fn frame(&mut self, at: Instant) -> (Vec<Note>, window::RedrawRequest) {
        self.send(Event::Window(window::Event::RedrawRequested(at)))
    }

    fn scroll_to(&mut self, y: f32) {
        let mut jump = scroll_to::<()>(
            widget::Id::new("list"),
            AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        );
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &(),
            &mut jump,
        );
    }
}

fn key_press() -> Event {
    let key = Key::Named(key::Named::ArrowDown);
    Event::Keyboard(keyboard::Event::KeyPressed {
        modified_key: key.clone(),
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        key,
        location: Location::Standard,
        modifiers: Modifiers::default(),
        text: None,
        repeat: false,
    })
}

/// 第一次见到滚动区就报告，之后位置不变什么都不发。
#[test]
fn the_first_frame_reports_and_an_unchanged_position_stays_quiet() {
    let mut harness = Harness::new(0.0..1_000.0, false);
    let start = Instant::now();
    assert_eq!(harness.frame(start).0, vec![Note::At(0.0)]);
    assert_eq!(
        harness.frame(start + Duration::from_millis(16)),
        (Vec::new(), window::RedrawRequest::Wait)
    );
    assert!(harness.send(key_press()).0.is_empty());
}

/// 在范围里挪动不报告，停下一段时间后报告一次。
#[test]
fn moving_within_the_quiet_range_is_reported_once_it_settles() {
    let mut harness = Harness::new(0.0..1_000.0, false);
    let start = Instant::now();
    harness.frame(start);
    harness.scroll_to(40.0);
    let (notes, redraw) = harness.frame(start + Duration::from_millis(16));
    assert!(notes.is_empty());
    assert_eq!(
        redraw,
        window::RedrawRequest::At(start + Duration::from_millis(16) + SETTLE)
    );
    harness.scroll_to(80.0);
    let moved = start + Duration::from_millis(32);
    assert!(harness.frame(moved).0.is_empty());
    assert!(harness.frame(moved + SETTLE / 2).0.is_empty());
    assert_eq!(harness.frame(moved + SETTLE).0, vec![Note::At(80.0)]);
}

/// 出了应用给的范围马上报告。
#[test]
fn leaving_the_quiet_range_is_reported_at_once() {
    let mut harness = Harness::new(0.0..50.0, false);
    let start = Instant::now();
    harness.frame(start);
    harness.scroll_to(30.0);
    assert!(harness
        .frame(start + Duration::from_millis(16))
        .0
        .is_empty());
    harness.scroll_to(80.0);
    assert_eq!(
        harness.frame(start + Duration::from_millis(32)).0,
        vec![Note::At(80.0)]
    );
}

/// 里面的控件发消息（滚动后光标下换了一行）时，位置排在它前面。
#[test]
fn a_message_from_inside_comes_after_the_current_position() {
    let mut harness = Harness::new(0.0..1_000.0, true);
    let start = Instant::now();
    assert_eq!(
        harness.frame(start).0,
        vec![Note::At(0.0), Note::Entered(0)]
    );
    harness.scroll_to(ROW);
    assert_eq!(
        harness.frame(start + Duration::from_millis(16)).0,
        vec![Note::At(ROW), Note::Entered(1)]
    );
}

/// 按键前把位置补上：键盘走订阅，处理按键时应用手里的位置得是准的。
#[test]
fn a_key_press_brings_the_position_up_to_date() {
    let mut harness = Harness::new(0.0..1_000.0, false);
    let start = Instant::now();
    harness.frame(start);
    harness.scroll_to(30.0);
    assert!(harness
        .frame(start + Duration::from_millis(16))
        .0
        .is_empty());
    assert_eq!(harness.send(key_press()).0, vec![Note::At(30.0)]);
}

/// 空范围：每次变化都报告（框选时应用要跟着每一帧）。
#[test]
fn an_empty_quiet_range_reports_every_change() {
    let mut harness = Harness::new(0.0..0.0, false);
    let start = Instant::now();
    harness.frame(start);
    for (frame, y) in [(1, 3.0), (2, 7.0), (3, 12.0)] {
        harness.scroll_to(y);
        assert_eq!(
            harness.frame(start + Duration::from_millis(16 * frame)).0,
            vec![Note::At(y)]
        );
    }
}
