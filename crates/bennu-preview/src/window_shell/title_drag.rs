//! 顶栏空白处的拖动面：按下发拖动消息，第二次快速按下发最大化切换消息。
//!
//! 它垫在顶栏最底层，上面的按钮、标题自己处理事件；落到这里的按下就是空白处。
//! 每次按下只发一条消息：iced 的 `mouse_area` 在双击时会先发 `on_press` 再发
//! `on_double_click`，拖动和最大化会同时触发。

use iced::advanced::{layout, mouse, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{touch, Element, Event, Length, Point, Rectangle, Size};

pub(super) fn title_drag_surface<'a, Message: Clone + 'a>(
    height: f32,
    drag: Message,
    double_click: Message,
) -> Element<'a, Message> {
    Element::new(TitleDragSurface {
        height,
        drag,
        double_click,
    })
}

struct TitleDragSurface<Message> {
    height: f32,
    drag: Message,
    double_click: Message,
}

#[derive(Default)]
struct PreviousClick(Option<mouse::Click>);

#[derive(Debug, PartialEq, Eq)]
enum TitlePress {
    Drag,
    ToggleMaximize,
}

/// 这次按下是拖动还是最大化切换；第三次快速按下重新算拖动。
fn title_press(position: Point, previous: Option<mouse::Click>) -> (TitlePress, mouse::Click) {
    let click = mouse::Click::new(position, mouse::Button::Left, previous);
    let press = match click.kind() {
        mouse::click::Kind::Double => TitlePress::ToggleMaximize,
        mouse::click::Kind::Single | mouse::click::Kind::Triple => TitlePress::Drag,
    };
    (press, click)
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for TitleDragSurface<Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PreviousClick>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(PreviousClick::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(self.height), Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(layout.bounds()) else {
                    return;
                };
                let previous = tree.state.downcast_mut::<PreviousClick>();
                let (press, click) = title_press(position, previous.0);
                previous.0 = Some(click);
                shell.publish(match press {
                    TitlePress::Drag => self.drag.clone(),
                    TitlePress::ToggleMaximize => self.double_click.clone(),
                });
                shell.capture_event();
            }
            Event::Touch(touch::Event::FingerPressed { position, .. })
                if layout.bounds().contains(*position) =>
            {
                shell.publish(self.drag.clone());
                shell.capture_event();
            }
            _ => {}
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
            mouse::Interaction::Grab
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_quick_press_toggles_maximize_and_third_starts_over_as_a_drag() {
        let position = Point::new(80.0, 16.0);
        let (first, click) = title_press(position, None);
        assert_eq!(first, TitlePress::Drag);
        let (second, click) = title_press(position, Some(click));
        assert_eq!(second, TitlePress::ToggleMaximize);
        let (third, _) = title_press(position, Some(click));
        assert_eq!(third, TitlePress::Drag);
    }
}
