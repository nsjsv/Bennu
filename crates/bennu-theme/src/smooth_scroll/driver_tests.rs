use std::time::Duration;

use iced::advanced::clipboard;
use iced::advanced::layout::Limits;
use iced::advanced::widget::operation::scrollable::{scroll_to, AbsoluteOffset, Scrollable};
use iced::advanced::widget::{self, Operation};
use iced::widget::{scrollable, Space};
use iced::Point;

use super::*;
use crate::smooth_scroll::{MOS_SCROLL_SPEED, MOS_SCROLL_STEP};

const CTRL_WHEEL: u8 = 1;

struct Harness {
    element: Element<'static, u8, iced::Theme, ()>,
    tree: Tree,
    node: layout::Node,
}

impl Harness {
    fn new(modifiers: keyboard::Modifiers) -> Self {
        let scroll = smooth_scroll(SmoothScrollAxis::Vertical).on_ctrl_wheel(|_| CTRL_WHEEL);
        let content = Space::new().width(Length::Fill).height(2_000.0);
        let mut element = scroll.drive(
            scrollable(scroll.capture(content))
                .id(widget::Id::new("list"))
                .height(100.0),
        );
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
        );
        let mut harness = Self {
            element,
            tree,
            node,
        };
        harness.send(Event::Keyboard(keyboard::Event::ModifiersChanged(
            modifiers,
        )));
        harness
    }

    /// 送一个事件，返回发出的消息和要不要重画。
    fn send(&mut self, event: Event) -> (Vec<u8>, window::RedrawRequest) {
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            Layout::new(&self.node),
            mouse::Cursor::Available(Point::new(50.0, 50.0)),
            &(),
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0)),
        );
        let redraw = shell.redraw_request();
        (messages, redraw)
    }

    fn wheel_down(&mut self) -> (Vec<u8>, window::RedrawRequest) {
        self.send(Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        }))
    }

    /// 触控板的一次手指移动；`y` 为负往下滚（和 winit 的方向一致）。
    fn pixels(&mut self, y: f32) {
        self.send(Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y },
        }));
    }

    fn lift(&mut self) -> window::RedrawRequest {
        self.send(Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 0.0 },
        }))
        .1
    }

    fn displacement(&self) -> f32 {
        self.tree
            .state
            .downcast_ref::<DriverState>()
            .touch
            .displacement()
            .y
    }

    /// 按 60 Hz 画到不再要重画为止。
    fn settle(&mut self) {
        let start = Instant::now();
        for frame in 1..2_000 {
            if self.frame(start + Duration::from_micros(16_667 * frame))
                == window::RedrawRequest::Wait
            {
                return;
            }
        }
        panic!("the motion never came to rest");
    }

    fn frame(&mut self, at: Instant) -> window::RedrawRequest {
        self.send(Event::Window(window::Event::RedrawRequested(at)))
            .1
    }

    fn offset(&mut self) -> f32 {
        struct Read(Option<Vector>);
        impl Operation for Read {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                _id: Option<&widget::Id>,
                _bounds: Rectangle,
                _content_bounds: Rectangle,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                self.0.get_or_insert(translation);
            }
        }
        let mut read = Read(None);
        self.element.as_widget_mut().operate(
            &mut self.tree,
            Layout::new(&self.node),
            &(),
            &mut read,
        );
        read.0.expect("scrollable").y
    }
}

/// 一格滚轮在控件里按帧滚完：不发消息，每帧只滚整像素，停下后不再要重画。
#[test]
fn a_wheel_notch_scrolls_in_whole_pixels_without_messages() {
    let mut harness = Harness::new(keyboard::Modifiers::empty());
    let (messages, redraw) = harness.wheel_down();
    assert!(messages.is_empty());
    assert_eq!(redraw, window::RedrawRequest::NextFrame);

    let start = Instant::now();
    let mut previous = harness.offset();
    let mut at = start;
    for _ in 0..200 {
        at += Duration::from_millis(7);
        let redraw = harness.frame(at);
        let offset = harness.offset();
        assert_eq!(offset.fract(), 0.0, "{offset}");
        assert!(offset >= previous);
        previous = offset;
        if redraw == window::RedrawRequest::Wait {
            break;
        }
    }
    let target = MOS_SCROLL_STEP * MOS_SCROLL_SPEED;
    assert!((previous - target).abs() <= 1.0, "{previous} vs {target}");
    assert_eq!(
        harness.frame(at + Duration::from_millis(7)),
        window::RedrawRequest::Wait
    );
}

/// 别处把滚动区挪走了（拖滚动条、程序跳转）：惯性停下，不把它拉回来。
#[test]
fn moving_the_scroller_elsewhere_stops_the_inertia() {
    let mut harness = Harness::new(keyboard::Modifiers::empty());
    harness.wheel_down();
    let start = Instant::now();
    harness.frame(start + Duration::from_millis(16));
    harness.frame(start + Duration::from_millis(32));
    assert!(harness.offset() > 0.0);

    let mut jump = scroll_to::<()>(
        widget::Id::new("list"),
        AbsoluteOffset {
            x: None,
            y: Some(500.0),
        },
    );
    harness.element.as_widget_mut().operate(
        &mut harness.tree,
        Layout::new(&harness.node),
        &(),
        &mut jump,
    );
    for frame in 3..20 {
        harness.frame(start + Duration::from_millis(16 * frame));
    }
    assert_eq!(harness.offset(), 500.0);
}

/// Ctrl+滚轮交给应用，不滚动。
#[test]
fn ctrl_wheel_goes_to_the_application_instead_of_scrolling() {
    let mut harness = Harness::new(keyboard::Modifiers::CTRL);
    let (messages, _) = harness.wheel_down();
    assert_eq!(messages, vec![CTRL_WHEEL]);
    let start = Instant::now();
    for frame in 1..20 {
        harness.frame(start + Duration::from_millis(16 * frame));
    }
    assert_eq!(harness.offset(), 0.0);
}

/// 触控板：手指移动时内容跟着走；松手（零位移的像素滚动）后接着滑一段再停。
#[test]
fn a_touchpad_flick_keeps_coasting_after_the_fingers_lift() {
    let mut harness = Harness::new(keyboard::Modifiers::empty());
    for _ in 0..6 {
        harness.pixels(-10.0);
        std::thread::sleep(Duration::from_millis(8));
    }
    assert_eq!(harness.offset(), 60.0);

    assert_ne!(harness.lift(), window::RedrawRequest::Wait);
    harness.settle();
    assert!(harness.offset() > 100.0, "{}", harness.offset());
    assert_eq!(harness.displacement(), 0.0);
}

/// 在顶上往上拉：内容不动、画的位置被拉开；松手弹回。
#[test]
fn pulling_past_the_top_springs_back_after_lifting() {
    let mut harness = Harness::new(keyboard::Modifiers::empty());
    for _ in 0..4 {
        harness.pixels(20.0);
    }
    assert_eq!(harness.offset(), 0.0);
    assert!(harness.displacement() < -10.0, "{}", harness.displacement());

    harness.lift();
    harness.settle();
    assert_eq!(harness.displacement(), 0.0);
    assert_eq!(harness.offset(), 0.0);
}

/// 没有手指按着时的零位移像素滚动什么也不做（别的滚动区的手势结束了）。
#[test]
fn a_lift_without_a_gesture_does_nothing() {
    let mut harness = Harness::new(keyboard::Modifiers::empty());
    assert_eq!(harness.lift(), window::RedrawRequest::Wait);
    assert_eq!(harness.offset(), 0.0);
}
