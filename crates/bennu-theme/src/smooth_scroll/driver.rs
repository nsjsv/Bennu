//! 惯性滚动的一对控件，共用本次 view 新建的收件箱：
//!
//! - [`SmoothScroll::capture`] 包在滚动区**里面**的内容外：里层（嵌套的滚动区、文本框）没用掉的
//!   滚轮在这里认领，iced 的滚动区就不会再自己滚一次；
//! - [`SmoothScroll::drive`] 包在滚动区**外面**：取走认领到的滚轮，惯性存在自己的控件状态里，
//!   每画一帧按真实时间推进，用操作直接滚它包着的滚动区。
//!
//! 惯性因此不经过应用：不发消息、不挂定时器，只重画所在窗口。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{keyboard, mouse, window, Element, Event, Length, Rectangle, Size, Vector};

use super::{
    wheel_delta_for_axis, MosInertia, SmoothScrollAxis, SmoothScrollDelta, WheelScrollMode,
};

/// 里层交给外层的一次滚轮。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wheel {
    Inertial(SmoothScrollDelta),
    /// 触控板给的是像素：直接滚，不加惯性。
    Direct(SmoothScrollDelta),
    /// 滚轮被拿去做别的（Ctrl+滚轮调密度）：在途的惯性停下。
    Stop,
}

/// 一个滚动区的惯性滚动：先 [`capture`](Self::capture) 内容、放进滚动区，再
/// [`drive`](Self::drive) 这个滚动区。
pub struct SmoothScroll<'a, Message> {
    axis: SmoothScrollAxis,
    inbox: Rc<Cell<Option<Wheel>>>,
    on_ctrl_wheel: Option<OnCtrlWheel<'a, Message>>,
}

type OnCtrlWheel<'a, Message> = Rc<dyn Fn(mouse::ScrollDelta) -> Message + 'a>;

pub fn smooth_scroll<'a, Message>(axis: SmoothScrollAxis) -> SmoothScroll<'a, Message> {
    SmoothScroll {
        axis,
        inbox: Rc::default(),
        on_ctrl_wheel: None,
    }
}

impl<'a, Message: 'a> SmoothScroll<'a, Message> {
    /// Ctrl+滚轮不滚动，交给应用（调密度档）。
    pub fn on_ctrl_wheel(
        mut self,
        on_ctrl_wheel: impl Fn(mouse::ScrollDelta) -> Message + 'a,
    ) -> Self {
        self.on_ctrl_wheel = Some(Rc::new(on_ctrl_wheel));
        self
    }

    pub fn capture<Theme: 'a, Renderer: iced::advanced::Renderer + 'a>(
        &self,
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(WheelCapture {
            content: content.into(),
            axis: self.axis,
            inbox: Rc::clone(&self.inbox),
            on_ctrl_wheel: self.on_ctrl_wheel.clone(),
        })
    }

    pub fn drive<Theme: 'a, Renderer: iced::advanced::Renderer + 'a>(
        &self,
        scroller: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(InertiaDriver {
            content: scroller.into(),
            inbox: Rc::clone(&self.inbox),
        })
    }
}

struct WheelCapture<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    axis: SmoothScrollAxis,
    inbox: Rc<Cell<Option<Wheel>>>,
    on_ctrl_wheel: Option<OnCtrlWheel<'a, Message>>,
}

/// Shift 决定横滚还是竖滚、Ctrl 决定调密度：按键状态记在控件里，不用每个视图从应用传进来。
#[derive(Debug, Default)]
struct CaptureState {
    modifiers: keyboard::Modifiers,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for WheelCapture<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CaptureState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CaptureState::default())
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
            tree.state.downcast_mut::<CaptureState>().modifiers = *modifiers;
        }
        // 嵌套滚动区必须让里层先认领滚轮，这里只兜底。
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event else {
            return;
        };
        if shell.is_event_captured() || !cursor.is_over(layout.bounds()) {
            return;
        }
        let modifiers = tree.state.downcast_ref::<CaptureState>().modifiers;
        if let Some(on_ctrl_wheel) = self.on_ctrl_wheel.as_ref().filter(|_| modifiers.control()) {
            shell.publish(on_ctrl_wheel(*delta));
            self.inbox.set(Some(Wheel::Stop));
            shell.capture_event();
            return;
        }
        let Some(wheel) = wheel_delta_for_axis(self.axis, modifiers.shift(), *delta) else {
            return;
        };
        self.inbox.set(Some(match wheel.mode {
            WheelScrollMode::MosAnimated => Wheel::Inertial(wheel.delta),
            WheelScrollMode::Direct => Wheel::Direct(wheel.delta),
        }));
        shell.capture_event();
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

struct InertiaDriver<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    inbox: Rc<Cell<Option<Wheel>>>,
}

#[derive(Debug, Default)]
struct DriverState {
    inertia: MosInertia,
    /// 上一步滚完后滚动区应在的位置；下一步前看到的不是它，说明到边了或别处挪过
    /// （拖滚动条、程序跳转），惯性就停下，不跟它拉扯。
    expected: Option<Vector>,
}

impl DriverState {
    fn stop(&mut self) {
        *self = Self::default();
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for InertiaDriver<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<DriverState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(DriverState::default())
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let state = state.downcast_mut::<DriverState>();
        let content = self.content.as_widget_mut();
        // 先滚再把这一帧交给滚动区：它在同一轮里报告新位置，应用按新位置排的内容赶得上这一帧。
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            step(
                state,
                content,
                &mut children[0],
                layout,
                renderer,
                *now,
                shell,
            );
        }
        content.update(
            &mut children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        match self.inbox.take() {
            Some(Wheel::Inertial(delta)) => {
                state.inertia.push(delta, Instant::now());
                shell.request_redraw();
            }
            Some(Wheel::Direct(delta)) => {
                state.stop();
                scroll_first(
                    content,
                    &mut children[0],
                    layout,
                    renderer,
                    Vector::new(delta.x, delta.y),
                    None,
                );
                shell.request_redraw();
            }
            Some(Wheel::Stop) => state.stop(),
            None => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

/// 推进惯性到 `now`，攒够整像素就滚；还在动就约下一次重画。
fn step<Message, Theme, Renderer>(
    state: &mut DriverState,
    content: &mut dyn Widget<Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
    now: Instant,
    shell: &mut Shell<'_, Message>,
) where
    Renderer: iced::advanced::Renderer,
{
    if !state.inertia.is_active() {
        return;
    }
    let step = state.inertia.step(now);
    if step.scroll != Vector::ZERO {
        match scroll_first(content, tree, layout, renderer, step.scroll, state.expected) {
            Some(before) => state.expected = Some(before + step.scroll),
            None => {
                state.stop();
                return;
            }
        }
    }
    match step.next {
        Some(next) => shell.request_redraw_at(next),
        None => state.stop(),
    }
}

fn same_position(a: Vector, b: Vector) -> bool {
    (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5
}

/// 滚子树里第一个滚动区（就是驱动器包着的那个），返回滚之前它的位置。给了 `expected` 而
/// 滚动区不在那儿（到边被挡住、别处挪过）就不滚，返回 `None`。
fn scroll_first<Message, Theme, Renderer>(
    content: &mut dyn Widget<Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
    delta: Vector,
    expected: Option<Vector>,
) -> Option<Vector>
where
    Renderer: iced::advanced::Renderer,
{
    let mut operation = ScrollFirst {
        delta,
        expected,
        outcome: None,
    };
    content.operate(tree, layout, renderer, &mut operation);
    operation.outcome.flatten()
}

struct ScrollFirst {
    delta: Vector,
    expected: Option<Vector>,
    /// 遇到第一个滚动区后才有值：里面是滚之前的位置，不该滚时是 `None`。
    outcome: Option<Option<Vector>>,
}

impl Operation for ScrollFirst {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.outcome.is_none() {
            operate(self);
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if self.outcome.is_some() {
            return;
        }
        if self
            .expected
            .is_some_and(|expected| !same_position(expected, translation))
        {
            self.outcome = Some(None);
            return;
        }
        self.outcome = Some(Some(translation));
        state.scroll_by(
            AbsoluteOffset {
                x: self.delta.x,
                y: self.delta.y,
            },
            bounds,
            content_bounds,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iced::advanced::clipboard;
    use iced::advanced::layout::Limits;
    use iced::advanced::widget::operation::scrollable::scroll_to;
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
}
