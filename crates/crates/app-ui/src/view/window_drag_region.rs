use iced::advanced::{layout, mouse, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{touch, window, Element, Event, Length, Point, Rectangle, Size, Vector};

use crate::model::Message;

pub(crate) fn window_drag_region(
    content: Element<'_, Message>,
    window: window::Id,
) -> Element<'_, Message> {
    Element::new(WindowDragRegion { content, window })
}

struct WindowDragRegion<'a> {
    content: Element<'a, Message>,
    window: window::Id,
}

#[derive(Default)]
struct WindowDragRegionState {
    previous_click: Option<mouse::Click>,
}

impl Widget<Message, iced::Theme, iced::Renderer> for WindowDragRegion<'_> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<WindowDragRegionState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(WindowDragRegionState::default())
    }

    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
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
        renderer: &iced::Renderer,
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
        renderer: &iced::Renderer,
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
        let content_interaction = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        if !window_drag_can_start(content_interaction) {
            return;
        }

        let state = tree.state.downcast_mut::<WindowDragRegionState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.is_over(layout.bounds()) =>
            {
                if let Some(position) = cursor.position() {
                    let (message, click) =
                        window_title_press_message(self.window, position, state.previous_click);
                    state.previous_click = Some(click);
                    shell.publish(message);
                    shell.capture_event();
                }
            }
            Event::Touch(touch::Event::FingerPressed { position, .. })
                if layout.bounds().contains(*position) =>
            {
                shell.publish(Message::WindowDragRequested(self.window));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let content_interaction = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        if window_drag_can_start(content_interaction) && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Grab
        } else {
            content_interaction
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
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

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut widget::Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

fn window_drag_can_start(content_interaction: mouse::Interaction) -> bool {
    content_interaction == mouse::Interaction::None
}

fn window_title_press_message(
    window: window::Id,
    position: Point,
    previous_click: Option<mouse::Click>,
) -> (Message, mouse::Click) {
    let click = mouse::Click::new(position, mouse::Button::Left, previous_click);
    let message = if click.kind() == mouse::click::Kind::Double {
        Message::WindowMaximizeToggled(window)
    } else {
        Message::WindowDragRequested(window)
    };
    (message, click)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_content_never_qualifies_as_window_drag_background() {
        assert!(window_drag_can_start(mouse::Interaction::None));
        for interaction in [
            mouse::Interaction::Text,
            mouse::Interaction::Pointer,
            mouse::Interaction::Grab,
        ] {
            assert!(!window_drag_can_start(interaction));
        }
    }

    #[test]
    fn second_title_bar_press_toggles_maximize_without_starting_drag() {
        let window = window::Id::unique();
        let position = Point::new(80.0, 16.0);
        let (first_message, first_click) = window_title_press_message(window, position, None);
        assert!(matches!(first_message, Message::WindowDragRequested(id) if id == window));

        let (second_message, _) = window_title_press_message(window, position, Some(first_click));
        assert!(matches!(second_message, Message::WindowMaximizeToggled(id) if id == window));
    }
}
