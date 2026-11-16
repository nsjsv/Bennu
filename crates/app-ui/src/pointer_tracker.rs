use std::cell::Cell;
use std::rc::Rc;

use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Point, Rectangle, Size, Vector};

/// 指针最后所在的位置（窗口坐标），由窗口根部的 [`track_pointer`] 静默写入。
///
/// 位置是输入状态，不是应用状态：大多数时候只在按下、悬停重算时读一次。若每次
/// 移动都发消息，iced 每条消息都要重建所有窗口的界面并重画（高回报率鼠标每秒
/// 三百多次），所以只写这里，不打扰 update。
#[derive(Debug, Clone, Default)]
pub(crate) struct PointerPosition(Rc<Cell<Option<Point>>>);

impl PointerPosition {
    pub(crate) fn get(&self) -> Option<Point> {
        self.0.get()
    }

    pub(crate) fn set(&self, position: Option<Point>) {
        self.0.set(position);
    }
}

/// 包住一个窗口的整棵界面：记下指针位置；只有 `on_move` 给出时（有交互正跟着
/// 指针走）才把移动作为消息发出去。
pub(crate) fn track_pointer<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    position: Option<PointerPosition>,
    on_move: Option<Box<dyn Fn(Point) -> Message + 'a>>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(PointerTracker {
        content: content.into(),
        position,
        on_move,
    })
}

struct PointerTracker<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    position: Option<PointerPosition>,
    on_move: Option<Box<dyn Fn(Point) -> Message + 'a>>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PointerTracker<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
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
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // 坐标取自事件本身：指针在浮层上时 iced 把 `cursor` 标成不可用，但根部照样收到移动。
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            if let Some(tracked) = &self.position {
                tracked.set(Some(*position));
            }
            if let Some(on_move) = &self.on_move {
                shell.publish(on_move(*position));
            }
        }
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
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
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
        tree: &widget::Tree,
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
        tree: &'b mut widget::Tree,
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

#[cfg(test)]
mod tests {
    use iced::advanced::layout::Limits;
    use iced::advanced::{clipboard, widget::Tree};

    use super::*;

    /// 让一次指针移动经过记录器：返回记下的位置和发出的消息。
    fn move_pointer_over(follows: bool) -> (Option<Point>, Vec<Point>) {
        let tracked = PointerPosition::default();
        let on_move =
            follows.then(|| Box::new(|position: Point| position) as Box<dyn Fn(Point) -> Point>);
        let mut element: Element<'_, Point, (), ()> =
            track_pointer(iced::widget::Space::new(), Some(tracked.clone()), on_move);
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
        );
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        let moved_to = Point::new(12.0, 24.0);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: moved_to }),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &(),
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0)),
        );
        (tracked.get(), messages)
    }

    /// 没人跟着指针走时只记位置、不发消息；有人跟着时两样都做。
    #[test]
    fn movement_is_recorded_and_only_published_while_followed() {
        let moved_to = Point::new(12.0, 24.0);

        assert_eq!(move_pointer_over(false), (Some(moved_to), Vec::new()));
        assert_eq!(move_pointer_over(true), (Some(moved_to), vec![moved_to]));
    }
}
