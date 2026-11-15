//! 透明图片底下垫的棋盘格。只画露在视口里的那部分格子：放大后图片可能有上万像素宽，
//! 整张画会是几十万个方块。格子跟着图片走（以图片左上角为原点），拖动时不会在图下滑动。

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{self, Widget};
use iced::advanced::Renderer as _;
use iced::{mouse, Color, Element, Length, Rectangle, Size};

const SQUARE: f32 = 8.0;
const LIGHT: Color = Color::from_rgb(0.93, 0.93, 0.93);
const DARK: Color = Color::from_rgb(0.80, 0.80, 0.80);

pub(crate) fn checkerboard<'a, Message: 'a>(width: f32, height: f32) -> Element<'a, Message> {
    Element::new(Checkerboard { width, height })
}

struct Checkerboard {
    width: f32,
    height: f32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Checkerboard {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width), Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fixed(self.width),
            Length::Fixed(self.height),
            Size::new(self.width, self.height),
        ))
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(visible) = bounds.intersection(viewport) else {
            return;
        };
        fill(renderer, visible, LIGHT);
        let first_column = ((visible.x - bounds.x) / SQUARE).floor() as i64;
        let first_row = ((visible.y - bounds.y) / SQUARE).floor() as i64;
        let last_column = ((visible.x + visible.width - bounds.x) / SQUARE).ceil() as i64;
        let last_row = ((visible.y + visible.height - bounds.y) / SQUARE).ceil() as i64;
        for row in first_row..last_row {
            for column in first_column..last_column {
                if (row + column) % 2 == 0 {
                    continue;
                }
                let square = Rectangle {
                    x: bounds.x + column as f32 * SQUARE,
                    y: bounds.y + row as f32 * SQUARE,
                    width: SQUARE,
                    height: SQUARE,
                };
                if let Some(square) = square.intersection(&visible) {
                    fill(renderer, square, DARK);
                }
            }
        }
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Color) {
    renderer.fill_quad(
        Quad {
            bounds,
            ..Quad::default()
        },
        color,
    );
}
