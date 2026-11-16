//! 带位置的指针区：按下、滚轮、双击时把指针在区里的位置一起交出去。
//!
//! iced 的 `mouse_area` 只有 `on_move` 给位置，要拿到「按下、滚轮时指针在哪」只能每次移动都发消息；
//! 每条消息都要重建、重画所有窗口，指针在图片、文档上随便晃一下，主窗口也跟着画。这里位置随
//! 按下、滚轮、双击一起交出去，逐次的移动只在视图挂上 `on_move` 时（正在拖：平移图片、拖选文字）
//! 才报告。语义照 `mouse_area`：移动、按下、滚轮都只在指针在区里时算。

use iced::advanced::mouse::{click, Click};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Point, Rectangle, Size, Vector};

type OnPoint<'a, Message> = Box<dyn Fn(Point) -> Message + 'a>;

pub struct PointerArea<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_press: Option<OnPoint<'a, Message>>,
    on_move: Option<OnPoint<'a, Message>>,
    on_release: Option<Message>,
    on_double_click: Option<OnPoint<'a, Message>>,
    on_scroll: Option<Box<dyn Fn(mouse::ScrollDelta, Point) -> Message + 'a>>,
    on_exit: Option<Message>,
    interaction: Option<mouse::Interaction>,
}

pub fn pointer_area<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> PointerArea<'a, Message, Theme, Renderer> {
    PointerArea {
        content: content.into(),
        on_press: None,
        on_move: None,
        on_release: None,
        on_double_click: None,
        on_scroll: None,
        on_exit: None,
        interaction: None,
    }
}

impl<'a, Message, Theme, Renderer> PointerArea<'a, Message, Theme, Renderer> {
    /// 左键按下，给出按在区里的位置（区的左上角为原点）。
    pub fn on_press(mut self, on_press: impl Fn(Point) -> Message + 'a) -> Self {
        self.on_press = Some(Box::new(on_press));
        self
    }

    /// 指针在区里移动（或区被挪动）时的位置：只在正拖着时挂上。
    pub fn on_move(mut self, on_move: impl Fn(Point) -> Message + 'a) -> Self {
        self.on_move = Some(Box::new(on_move));
        self
    }

    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn on_double_click(mut self, on_double_click: impl Fn(Point) -> Message + 'a) -> Self {
        self.on_double_click = Some(Box::new(on_double_click));
        self
    }

    /// 滚轮，连同指针在区里的位置（缩放的锚点）。
    pub fn on_scroll(
        mut self,
        on_scroll: impl Fn(mouse::ScrollDelta, Point) -> Message + 'a,
    ) -> Self {
        self.on_scroll = Some(Box::new(on_scroll));
        self
    }

    pub fn on_exit(mut self, message: Message) -> Self {
        self.on_exit = Some(message);
        self
    }

    pub fn interaction(mut self, interaction: mouse::Interaction) -> Self {
        self.interaction = Some(interaction);
        self
    }
}

#[derive(Default)]
struct State {
    hovered: bool,
    cursor: Option<Point>,
    bounds: Rectangle,
    previous_click: Option<Click>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PointerArea<'a, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
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
        if shell.is_event_captured() {
            return;
        }
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let inside = cursor.position_in(bounds);
        if state.cursor != cursor.position() || state.bounds != bounds {
            let was_hovered = state.hovered;
            state.hovered = inside.is_some();
            state.cursor = cursor.position();
            state.bounds = bounds;
            match (&self.on_move, inside) {
                (Some(on_move), Some(position)) => shell.publish(on_move(position)),
                _ if was_hovered && !state.hovered => {
                    if let Some(on_exit) = &self.on_exit {
                        shell.publish(on_exit.clone());
                    }
                }
                _ => {}
            }
        }
        let Some(position) = inside else {
            return;
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(on_press) = &self.on_press {
                    shell.publish(on_press(position));
                    shell.capture_event();
                }
                if let Some(on_double_click) = &self.on_double_click {
                    let click = Click::new(
                        cursor.position().unwrap_or(position),
                        mouse::Button::Left,
                        state.previous_click,
                    );
                    if click.kind() == click::Kind::Double {
                        shell.publish(on_double_click(position));
                    }
                    state.previous_click = Some(click);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some(on_release) = &self.on_release {
                    shell.publish(on_release.clone());
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(on_scroll) = &self.on_scroll {
                    shell.publish(on_scroll(*delta, position));
                    shell.capture_event();
                }
            }
            _ => {}
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
        let content = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        match (self.interaction, content) {
            (Some(interaction), mouse::Interaction::None) if cursor.is_over(layout.bounds()) => {
                interaction
            }
            _ => content,
        }
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

impl<'a, Message, Theme, Renderer> From<PointerArea<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(area: PointerArea<'a, Message, Theme, Renderer>) -> Self {
        Element::new(area)
    }
}

#[cfg(test)]
#[path = "pointer_area_tests.rs"]
mod tests;
