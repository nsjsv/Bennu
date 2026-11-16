use iced::advanced::clipboard;
use iced::advanced::layout::Limits;
use iced::widget::Space;

use super::*;

#[derive(Debug, Clone, PartialEq)]
enum Note {
    Pressed(Point),
    Moved(Point),
    Released,
    DoubleClicked(Point),
    Scrolled(Point),
    Left,
}

/// 区在窗口 (20, 30) 处、100×100；`dragging` 时挂上 `on_move`。
struct Harness {
    element: Element<'static, Note, iced::Theme, ()>,
    tree: Tree,
    node: layout::Node,
}

impl Harness {
    fn new(dragging: bool) -> Self {
        let area = pointer_area(Space::new().width(100.0).height(100.0))
            .on_press(Note::Pressed)
            .on_release(Note::Released)
            .on_double_click(Note::DoubleClicked)
            .on_scroll(|_, at| Note::Scrolled(at))
            .on_exit(Note::Left);
        let area = if dragging {
            area.on_move(Note::Moved)
        } else {
            area
        };
        let mut element: Element<'static, Note, iced::Theme, ()> = area.into();
        let mut tree = Tree::new(&element);
        let node = element
            .as_widget_mut()
            .layout(
                &mut tree,
                &(),
                &Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
            )
            .move_to(Point::new(20.0, 30.0));
        Self {
            element,
            tree,
            node,
        }
    }

    fn send(&mut self, event: Event, at: Point) -> Vec<Note> {
        let mut notes = Vec::new();
        let mut shell = Shell::new(&mut notes);
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            mouse::Cursor::Available(at),
            &(),
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::new(Point::ORIGIN, Size::new(400.0, 400.0)),
        );
        notes
    }

    fn move_to(&mut self, at: Point) -> Vec<Note> {
        self.send(Event::Mouse(mouse::Event::CursorMoved { position: at }), at)
    }

    fn press(&mut self, at: Point) -> Vec<Note> {
        self.send(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            at,
        )
    }
}

/// 不拖时移动什么都不发；按下、滚轮带上区里的位置。
#[test]
fn moving_is_silent_and_presses_carry_the_position() {
    let mut area = Harness::new(false);
    assert!(area.move_to(Point::new(30.0, 40.0)).is_empty());
    assert!(area.move_to(Point::new(31.0, 42.0)).is_empty());
    assert_eq!(
        area.press(Point::new(31.0, 42.0)),
        vec![Note::Pressed(Point::new(11.0, 12.0))]
    );
    assert_eq!(
        area.send(
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            }),
            Point::new(70.0, 80.0),
        ),
        vec![Note::Scrolled(Point::new(50.0, 50.0))]
    );
    assert_eq!(area.move_to(Point::new(300.0, 300.0)), vec![Note::Left]);
}

/// 拖着时（视图挂上了 `on_move`）每次移动都报告位置。
#[test]
fn moves_are_reported_while_dragging() {
    let mut area = Harness::new(true);
    assert_eq!(
        area.move_to(Point::new(30.0, 40.0)),
        vec![Note::Moved(Point::new(10.0, 10.0))]
    );
    assert_eq!(
        area.move_to(Point::new(35.0, 45.0)),
        vec![Note::Moved(Point::new(15.0, 15.0))]
    );
}

/// 连按两下在同一处：第二下同时报告双击。
#[test]
fn a_second_press_in_place_is_a_double_click() {
    let mut area = Harness::new(false);
    let at = Point::new(50.0, 60.0);
    assert_eq!(area.press(at), vec![Note::Pressed(Point::new(30.0, 30.0))]);
    let _ = area.send(
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        at,
    );
    assert_eq!(
        area.press(at),
        vec![
            Note::Pressed(Point::new(30.0, 30.0)),
            Note::DoubleClicked(Point::new(30.0, 30.0))
        ]
    );
}
