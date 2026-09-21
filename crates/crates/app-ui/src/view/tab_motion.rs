use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

pub(crate) fn translated<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    x: f32,
    y: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(Translated {
        content: content.into(),
        offset: Vector::new(x, y),
        extra_width: 0.0,
    })
}

pub(crate) fn translated_with_width_overflow<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    x: f32,
    y: f32,
    extra_width: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(Translated {
        content: content.into(),
        offset: Vector::new(x, y),
        extra_width,
    })
}

struct Translated<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    offset: Vector,
    extra_width: f32,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Translated<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let content_size = self.content.as_widget().size();
        let child_limits = if self.extra_width > f32::EPSILON {
            layout::Limits::with_compression(
                limits.min(),
                Size::new(limits.max().width + self.extra_width, limits.max().height),
                limits.compression(),
            )
        } else {
            *limits
        };
        let node =
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &child_limits);
        let size = if self.extra_width > f32::EPSILON {
            limits.resolve(content_size.width, content_size.height, node.size())
        } else {
            node.size()
        };

        layout::Node::with_children(
            size,
            vec![node.move_to(Point::new(self.offset.x, self.offset.y))],
        )
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.child(0),
            renderer,
            operation,
        );
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
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.child(0),
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
            layout.child(0),
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
            layout.child(0),
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
            layout.child(0),
            renderer,
            viewport,
            translation,
        )
    }
}
