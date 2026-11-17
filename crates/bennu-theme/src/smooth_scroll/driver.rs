//! 惯性滚动的一对控件，共用本次 view 新建的一条连线（[`Link`]）：
//!
//! - [`SmoothScroll::capture`] 包在滚动区**里面**的内容外：里层（嵌套的滚动区、文本框）没用掉的
//!   滚轮在这里认领，iced 的滚动区就不会再自己滚一次；拉过边缘时把内容挪开画；
//! - [`SmoothScroll::drive`] 包在滚动区**外面**：取走认领到的滚轮，滚轮惯性、触控板的跟手/
//!   滑行/回弹存在自己的控件状态里，每画一帧按真实时间推进，用操作直接滚它包着的滚动区。
//!
//! 惯性因此不经过应用：不发消息、不挂定时器，只重画所在窗口。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{keyboard, mouse, window, Element, Event, Length, Rectangle, Size, Vector};

use super::scroller::with_first_scroller;
use super::touch::TouchScroll;
use super::{
    is_gesture_end, wheel_delta_for_axis, MosInertia, SmoothScrollAxis, SmoothScrollDelta,
    WheelScrollMode,
};

/// 里层交给外层的一次滚轮。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wheel {
    Inertial(SmoothScrollDelta),
    /// 触控板的手指移动（像素）：内容跟着手指走。
    Direct(SmoothScrollDelta),
    /// 手指离开触控板。
    Lifted,
    /// 滚轮被拿去做别的（Ctrl+滚轮调密度）：在途的惯性停下。
    Stop,
}

/// 认领滚轮的里层和推进惯性的外层之间：滚轮往外交，拉过边缘的位移往里给。
#[derive(Debug, Default)]
struct Link {
    wheel: Cell<Option<Wheel>>,
    /// 内容画出来要挪开的距离（滚动方向，正 = 越过末端）。
    displacement: Cell<Vector>,
}

/// 一个滚动区的惯性滚动：先 [`capture`](Self::capture) 内容、放进滚动区，再
/// [`drive`](Self::drive) 这个滚动区。
pub struct SmoothScroll<'a, Message> {
    axis: SmoothScrollAxis,
    link: Rc<Link>,
    on_ctrl_wheel: Option<OnCtrlWheel<'a, Message>>,
}

type OnCtrlWheel<'a, Message> = Rc<dyn Fn(mouse::ScrollDelta) -> Message + 'a>;

pub fn smooth_scroll<'a, Message>(axis: SmoothScrollAxis) -> SmoothScroll<'a, Message> {
    SmoothScroll {
        axis,
        link: Rc::default(),
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
            link: Rc::clone(&self.link),
            on_ctrl_wheel: self.on_ctrl_wheel.clone(),
        })
    }

    pub fn drive<Theme: 'a, Renderer: iced::advanced::Renderer + 'a>(
        &self,
        scroller: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(InertiaDriver {
            content: scroller.into(),
            link: Rc::clone(&self.link),
        })
    }
}

struct WheelCapture<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    axis: SmoothScrollAxis,
    link: Rc<Link>,
    on_ctrl_wheel: Option<OnCtrlWheel<'a, Message>>,
}

impl<Message, Theme, Renderer> WheelCapture<'_, Message, Theme, Renderer> {
    /// 内容画在哪就在哪点中：越界挪开画时，光标也换到内容自己的坐标。
    fn shift(&self) -> Vector {
        -self.link.displacement.get()
    }
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
        let shift = self.shift();
        // 嵌套滚动区必须让里层先认领滚轮，这里只兜底。
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor - shift,
            renderer,
            clipboard,
            shell,
            &(*viewport - shift),
        );
        let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event else {
            return;
        };
        // 松手不认领、也不看光标在不在：每层都告诉自己的驱动器，只有正被手指拖着的那层
        // 会接着滑（手势可能是外层在滚，而里层先看到这个事件）。
        if is_gesture_end(*delta) {
            self.link.wheel.set(Some(Wheel::Lifted));
            return;
        }
        if shell.is_event_captured() || !cursor.is_over(layout.bounds()) {
            return;
        }
        let modifiers = tree.state.downcast_ref::<CaptureState>().modifiers;
        if let Some(on_ctrl_wheel) = self.on_ctrl_wheel.as_ref().filter(|_| modifiers.control()) {
            shell.publish(on_ctrl_wheel(*delta));
            self.link.wheel.set(Some(Wheel::Stop));
            shell.capture_event();
            return;
        }
        let Some(wheel) = wheel_delta_for_axis(self.axis, modifiers.shift(), *delta) else {
            return;
        };
        self.link.wheel.set(Some(match wheel.mode {
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
        let shift = self.shift();
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor - shift,
            &(*viewport - shift),
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
        let shift = self.shift();
        let content = self.content.as_widget();
        if shift == Vector::ZERO {
            content.draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
            return;
        }
        renderer.with_translation(shift, |renderer| {
            content.draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor - shift,
                &(*viewport - shift),
            );
        });
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
    link: Rc<Link>,
}

#[derive(Debug, Default)]
struct DriverState {
    inertia: MosInertia,
    touch: TouchScroll,
    /// 上一步自己滚完后滚动区应在的位置；下一步前看到的不是它，说明到边了或别处挪过
    /// （拖滚动条、程序跳转），滚轮惯性、触控板滑行就停下，不跟它拉扯。
    expected: Option<Vector>,
}

impl DriverState {
    fn stop(&mut self) {
        self.inertia.stop();
        self.touch.stop();
        self.expected = None;
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
        self.link.displacement.set(state.touch.displacement());
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
        let Some(wheel) = self.link.wheel.take() else {
            return;
        };
        let now = Instant::now();
        match wheel {
            Wheel::Inertial(delta) => {
                if !state.inertia.is_active() {
                    state.expected = None;
                }
                state.touch.stop();
                state.inertia.push(delta, now);
                shell.request_redraw();
            }
            Wheel::Direct(delta) => {
                state.inertia.stop();
                state.expected = None;
                let touch = &mut state.touch;
                with_first_scroller(content, &mut children[0], layout, renderer, |scroller| {
                    touch.follow(Vector::new(delta.x, delta.y), now, scroller);
                });
                shell.request_redraw();
                if let Some(next) = state.touch.next_redraw() {
                    shell.request_redraw_at(next);
                }
            }
            Wheel::Lifted if state.touch.is_held() => {
                state.expected = None;
                state.touch.release(now);
                shell.request_redraw();
            }
            Wheel::Lifted => {}
            Wheel::Stop => state.stop(),
        }
        self.link.displacement.set(state.touch.displacement());
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<DriverState>();
        self.link.displacement.set(state.touch.displacement());
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
        let state = tree.state.downcast_ref::<DriverState>();
        self.link.displacement.set(state.touch.displacement());
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

/// 推进到 `now`：滚轮惯性和触控板滑行攒够整像素就滚，回弹只改画的位置；还在动就约下一次
/// 重画。自己滚的这两种动作看到滚动区不在上一步留下的位置（到边、别处挪过）就停下。
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
    if state.inertia.is_active() {
        let step = state.inertia.step(now);
        if step.scroll != Vector::ZERO {
            let DriverState {
                inertia, expected, ..
            } = state;
            let mut moved = false;
            with_first_scroller(content, tree, layout, renderer, |scroller| {
                let before = scroller.offset();
                if expected.is_some_and(|expected| !same_position(expected, before)) {
                    return;
                }
                scroller.scroll_by(step.scroll);
                moved = scroller.offset() != before;
                *expected = Some(scroller.offset());
            });
            // 到边滚不动了，或者别处挪过：停下。
            if !moved {
                inertia.stop();
            }
        }
        match step.next.filter(|_| state.inertia.is_active()) {
            Some(next) => shell.request_redraw_at(next),
            None => state.expected = None,
        }
    }
    if state.touch.next_redraw().is_some() {
        let DriverState {
            touch, expected, ..
        } = state;
        with_first_scroller(content, tree, layout, renderer, |scroller| {
            if touch.is_coasting()
                && expected.is_some_and(|expected| !same_position(expected, scroller.offset()))
            {
                touch.stop();
                return;
            }
            touch.advance(now, scroller);
            *expected = Some(scroller.offset());
        });
        if let Some(next) = state.touch.next_redraw() {
            shell.request_redraw_at(next);
        }
    }
}

fn same_position(a: Vector, b: Vector) -> bool {
    (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
