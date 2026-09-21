use iced::advanced::{layout, mouse, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size};

pub(crate) fn input_blocking_space<'a, Message>(
    width: Length,
    height: Length,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(InputBlockingSpace { width, height })
}

struct InputBlockingSpace {
    width: Length,
    height: Length,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for InputBlockingSpace {
    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, self.width, self.height)
    }

    fn update(
        &mut self,
        _tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if cursor.is_over(layout.bounds())
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(_))
                    | Event::Mouse(mouse::Event::ButtonReleased(_))
            )
        {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        _renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}
