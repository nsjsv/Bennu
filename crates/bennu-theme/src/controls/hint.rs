//! 悬浮提示：鼠标停 `TOOLTIP_DELAY` 才出来；按下鼠标或键盘（点按钮、右键或菜单键弹菜单）就收起，
//! 鼠标离开再进来才重新计时。iced 自带的提示按下时不收，弹出的菜单和提示的字叠在一起。
//!
//! 提示是盖在控件上的一层（iced 的提示包着一块空白）：控件本身照常拿到鼠标，只有这一层在
//! 按下之后、或者 `shown` 说不用提示时看不到鼠标——直接对控件藏起鼠标，按钮就点不动了。

#![allow(clippy::disallowed_methods)]

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::text::Renderer as TextRenderer;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::widget::{tooltip as iced_tooltip, Space, Tooltip};
use iced::{keyboard, touch, Element, Event, Length, Rectangle, Size, Theme, Vector};

/// 鼠标停这么久提示才出来：只是划过去不冒东西挡视线。0.5 秒是桌面上通行的等待时长。
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// 全应用的悬浮提示只从这里来。
pub fn tooltip<'a, Message, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    bubble: impl Into<Element<'a, Message, Theme, Renderer>>,
    position: iced_tooltip::Position,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Renderer: TextRenderer + 'a,
{
    gated_tooltip(content, bubble, position, String::new(), |_| true)
}

/// 只在 `shown(控件的状态)` 为真时出提示（文件名被省略了才给全名）。`identity` 换了（虚拟列表
/// 里同一行换成了别的条目）就从头计时，上一个名字攒下的时间不算给新的。
pub(crate) fn gated_tooltip<'a, Message, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    bubble: impl Into<Element<'a, Message, Theme, Renderer>>,
    position: iced_tooltip::Position,
    identity: String,
    shown: impl Fn(&Tree) -> bool + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Renderer: TextRenderer + 'a,
{
    let layer = Tooltip::new(
        Space::new().width(Length::Fill).height(Length::Fill),
        bubble,
        position,
    )
    .delay(TOOLTIP_DELAY);
    Element::new(Hint {
        children: [content.into(), layer.into()],
        identity,
        shown: Box::new(shown),
    })
}

struct Hint<'a, Message, Renderer> {
    /// 控件本身和盖在上面的提示层。
    children: [Element<'a, Message, Theme, Renderer>; 2],
    identity: String,
    shown: Box<dyn Fn(&Tree) -> bool + 'a>,
}

struct HintState {
    identity: String,
    /// 鼠标在上面时按过键：离开之前不再提示。
    pressed: bool,
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Hint<'_, Message, Renderer>
where
    Renderer: TextRenderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HintState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(HintState {
            identity: self.identity.clone(),
            pressed: false,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
        let state = tree.state.downcast_mut::<HintState>();
        if state.identity != self.identity {
            state.identity.clone_from(&self.identity);
            state.pressed = false;
            tree.children[1] = Tree::new(&self.children[1]);
        }
    }

    fn size(&self) -> Size<Length> {
        self.children[0].as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.children[0].as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let content =
            self.children[0]
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
        let size = content.size();
        let layer = self.children[1].as_widget_mut().layout(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        layout::Node::with_children(size, vec![content, layer])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let content_layout = layout.children().next().expect("hint lays out its content");
        self.children[0].as_widget_mut().operate(
            &mut tree.children[0],
            content_layout,
            renderer,
            operation,
        );
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
        let mut child_layouts = layout.children();
        let (Some(content_layout), Some(layer_layout)) =
            (child_layouts.next(), child_layouts.next())
        else {
            return;
        };
        self.children[0].as_widget_mut().update(
            &mut tree.children[0],
            event,
            content_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        let over = cursor.is_over(layout.bounds());
        let state = tree.state.downcast_mut::<HintState>();
        let newly_pressed = over
            && !state.pressed
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(_))
                    | Event::Touch(touch::Event::FingerPressed { .. })
                    // 键盘也能弹菜单（菜单键、Shift+F10）。
                    | Event::Keyboard(keyboard::Event::KeyPressed { .. })
            );
        state.pressed = over && (state.pressed || newly_pressed);
        let pressed = state.pressed;
        if newly_pressed {
            // 当场收起：iced 的提示只看鼠标和重绘事件，按键盘时等不到它自己收。
            tree.children[1] = Tree::new(&self.children[1]);
            shell.invalidate_layout();
        }
        let hint_cursor = if !pressed && (self.shown)(&tree.children[0]) {
            cursor
        } else {
            // 看不到鼠标，iced 的提示就自己收起、停止计时。
            mouse::Cursor::Unavailable
        };
        self.children[1].as_widget_mut().update(
            &mut tree.children[1],
            event,
            layer_layout,
            hint_cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let content_layout = layout.children().next().expect("hint lays out its content");
        self.children[0].as_widget().mouse_interaction(
            &tree.children[0],
            content_layout,
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
        // 提示层本身是空白，浮出来的提示框走 overlay。
        let content_layout = layout.children().next().expect("hint lays out its content");
        self.children[0].as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            content_layout,
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
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use iced::advanced::layout::Limits;
    use iced::advanced::widget::Tree;
    use iced::advanced::{clipboard, mouse, Layout, Shell};
    use iced::widget::{mouse_area, text, tooltip as iced_tooltip, Space};
    use iced::{window, Element, Event, Point, Rectangle, Size, Theme, Vector};

    use super::{tooltip, TOOLTIP_DELAY};

    struct Probe {
        hint: Element<'static, &'static str, Theme, ()>,
        tree: Tree,
        node: iced::advanced::layout::Node,
    }

    impl Probe {
        /// 100×20 的控件，按下时发 "pressed"。
        fn new() -> Self {
            let content = mouse_area(Space::new().width(100.0).height(20.0)).on_press("pressed");
            let mut hint: Element<'static, &'static str, Theme, ()> =
                tooltip(content, text("hint"), iced_tooltip::Position::Bottom);
            let mut tree = Tree::new(&hint);
            let node = hint.as_widget_mut().layout(
                &mut tree,
                &(),
                &Limits::new(Size::ZERO, Size::new(100.0, 20.0)),
            );
            Self { hint, tree, node }
        }

        /// 送一个事件，返回控件发出的消息和这时提示是否浮出来。
        fn deliver(&mut self, event: Event, at: Point) -> (Vec<&'static str>, bool) {
            let viewport = Rectangle::with_size(Size::new(400.0, 400.0));
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            self.hint.as_widget_mut().update(
                &mut self.tree,
                &event,
                Layout::new(&self.node),
                mouse::Cursor::Available(at),
                &(),
                &mut clipboard::Null,
                &mut shell,
                &viewport,
            );
            let shown = self
                .hint
                .as_widget_mut()
                .overlay(
                    &mut self.tree,
                    Layout::new(&self.node),
                    &(),
                    &viewport,
                    Vector::ZERO,
                )
                .is_some();
            (messages, shown)
        }

        fn moved(&mut self, at: Point) -> bool {
            self.deliver(Event::Mouse(mouse::Event::CursorMoved { position: at }), at)
                .1
        }

        fn redrawn(&mut self, at: Point) -> bool {
            self.deliver(
                Event::Window(window::Event::RedrawRequested(std::time::Instant::now())),
                at,
            )
            .1
        }
    }

    /// 虚拟列表里同一行换成了别的条目：上一个名字攒下的时间不算给新的，从头计时。
    #[test]
    fn a_new_identity_starts_the_wait_over() {
        let element = |identity: &str| -> Element<'static, &'static str, Theme, ()> {
            super::gated_tooltip(
                Space::new().width(100.0).height(20.0),
                text("hint"),
                iced_tooltip::Position::Bottom,
                identity.to_owned(),
                |_| true,
            )
        };
        let mut probe = Probe::new();
        probe.hint = element("first");
        probe.tree = Tree::new(&probe.hint);
        probe.moved(INSIDE);
        std::thread::sleep(TOOLTIP_DELAY);
        assert!(probe.redrawn(INSIDE));

        probe.hint = element("second");
        probe.tree.diff(&probe.hint);
        assert!(!probe.redrawn(INSIDE));
    }

    const INSIDE: Point = Point::new(10.0, 10.0);
    const OUTSIDE: Point = Point::new(300.0, 300.0);

    /// 鼠标放上去先不出提示，停够 `TOOLTIP_DELAY` 才出来。
    #[test]
    fn tooltips_wait_before_showing() {
        let mut probe = Probe::new();
        assert!(!probe.moved(INSIDE));
        std::thread::sleep(TOOLTIP_DELAY);
        assert!(probe.redrawn(INSIDE));
    }

    /// 按下（右键弹菜单、点按钮）提示就收起，控件照样收到这次按下；鼠标不离开就不再出来，
    /// 离开再进来重新计时。
    #[test]
    fn a_press_hides_the_hint_until_the_pointer_leaves() {
        let mut probe = Probe::new();
        probe.moved(INSIDE);
        std::thread::sleep(TOOLTIP_DELAY);
        assert!(probe.redrawn(INSIDE));

        let (messages, shown) = probe.deliver(
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            INSIDE,
        );
        assert_eq!(messages, ["pressed"]);
        assert!(!shown);
        std::thread::sleep(TOOLTIP_DELAY);
        assert!(!probe.moved(INSIDE));
        assert!(!probe.redrawn(INSIDE));

        assert!(!probe.moved(OUTSIDE));
        assert!(!probe.moved(INSIDE));
        std::thread::sleep(TOOLTIP_DELAY);
        assert!(probe.redrawn(INSIDE));

        // 键盘弹菜单（菜单键）也一样收起。
        let menu_key = iced::keyboard::Key::Named(iced::keyboard::key::Named::ContextMenu);
        let (_, shown) = probe.deliver(
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: menu_key.clone(),
                modified_key: menu_key,
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: iced::keyboard::Modifiers::default(),
                text: None,
                repeat: false,
            }),
            INSIDE,
        );
        assert!(!shown);
    }
}
