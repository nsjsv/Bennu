//! 文档页上的标记层：搜索匹配、当前匹配、选中的文字，按页内比例画成半透明色块。

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{self, Widget};
use iced::advanced::Renderer as _;
use iced::{mouse, Color, Element, Length, Rectangle, Size};

use crate::document_preview::{PageMarkKind, PageRect};

/// 匹配用黄、当前那处用橙、选中用主题色，都半透明，盖在字上还看得清字。
fn mark_color(kind: PageMarkKind, theme: &iced::Theme) -> Color {
    match kind {
        PageMarkKind::Match => Color::from_rgba(1.0, 0.84, 0.0, 0.35),
        PageMarkKind::CurrentMatch => Color::from_rgba(1.0, 0.55, 0.0, 0.55),
        PageMarkKind::Selection => bennu_theme::ui_colors(theme).primary.scale_alpha(0.3),
    }
}

pub(crate) fn page_marks<'a, Message: 'a>(
    width: f32,
    height: f32,
    marks: Vec<(PageRect, PageMarkKind)>,
) -> Element<'a, Message> {
    Element::new(PageMarks {
        width,
        height,
        marks,
    })
}

struct PageMarks {
    width: f32,
    height: f32,
    marks: Vec<(PageRect, PageMarkKind)>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PageMarks {
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
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        for (rect, kind) in &self.marks {
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x: bounds.x + rect.left * bounds.width,
                        y: bounds.y + rect.top * bounds.height,
                        width: (rect.right - rect.left).max(0.0) * bounds.width,
                        height: (rect.bottom - rect.top).max(0.0) * bounds.height,
                    },
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..iced::Border::default()
                    },
                    ..Quad::default()
                },
                mark_color(*kind, theme),
            );
        }
    }
}
