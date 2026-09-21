use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::widget::container;
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector};

use crate::appearance::selection_marquee_style;
use crate::model::{Message, SelectionMarquee};

pub(crate) fn selection_marquee_layer<'a, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    marquee: Option<&SelectionMarquee>,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(SelectionMarqueeLayer {
        content: content.into(),
        marquee_rectangle: marquee.map(SelectionMarquee::rectangle),
    })
}

struct SelectionMarqueeLayer<'a, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    marquee_rectangle: Option<Rectangle>,
}

impl<'a, Renderer> Widget<Message, Theme, Renderer> for SelectionMarqueeLayer<'a, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
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

        let Some(marquee_rectangle) = self.marquee_rectangle else {
            return;
        };
        let Some(clip_bounds) = layout.bounds().intersection(viewport) else {
            return;
        };
        if marquee_rectangle.intersection(&clip_bounds).is_none() {
            return;
        }

        renderer.with_layer(clip_bounds, |renderer| {
            container::draw_background(
                renderer,
                &selection_marquee_style(theme),
                marquee_rectangle,
            );
        });
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
    use std::collections::HashSet;

    use iced::advanced::{image, layout, mouse, renderer, widget::Tree, Layout};
    use iced::widget::Space;
    use iced::{Background, Point, Rectangle, Size, Transformation};

    use super::*;
    use crate::model::{
        BrowserPaneId, SelectionMarqueePhase, SelectionMarqueeScrollAnchor, SelectionMarqueeSource,
    };

    #[derive(Default)]
    struct RecordingRenderer {
        layers: Vec<Rectangle>,
        quads: Vec<renderer::Quad>,
    }

    impl iced::advanced::Renderer for RecordingRenderer {
        fn start_layer(&mut self, bounds: Rectangle) {
            self.layers.push(bounds);
        }

        fn end_layer(&mut self) {}

        fn start_transformation(&mut self, _transformation: Transformation) {}

        fn end_transformation(&mut self) {}

        fn fill_quad(&mut self, quad: renderer::Quad, _background: impl Into<Background>) {
            self.quads.push(quad);
        }

        fn reset(&mut self, _new_bounds: Rectangle) {}

        fn allocate_image(
            &mut self,
            _handle: &image::Handle,
            _callback: impl FnOnce(Result<image::Allocation, image::Error>) + Send + 'static,
        ) {
            panic!("selection marquee rendering test must not allocate images");
        }
    }

    #[test]
    fn marquee_layer_clips_drawing_to_file_content_bounds() {
        let marquee = SelectionMarquee {
            gesture_origin: Point::new(20.0, 10.0),
            start: Point::new(20.0, 10.0),
            current: Point::new(120.0, 110.0),
            source: SelectionMarqueeSource::PaneBlank,
            phase: SelectionMarqueePhase::Selecting,
            scroll_anchor: SelectionMarqueeScrollAnchor::List {
                pane_id: BrowserPaneId::PRIMARY,
                offset_y: 0.0,
            },
            base_selection: HashSet::new(),
            preserve_existing: false,
        };
        let content: Element<'_, Message, Theme, RecordingRenderer> =
            Space::new().width(Length::Fill).height(Length::Fill).into();
        let mut layer = selection_marquee_layer(content, Some(&marquee));
        let mut tree = Tree::new(layer.as_widget());
        let mut renderer = RecordingRenderer::default();
        let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 100.0));
        let node = layer
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits)
            .move_to(Point::new(0.0, 40.0));
        let viewport = Rectangle::new(Point::ORIGIN, Size::new(200.0, 200.0));

        layer.as_widget().draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &renderer::Style::default(),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &viewport,
        );

        assert_eq!(
            renderer.layers,
            vec![Rectangle::new(
                Point::new(0.0, 40.0),
                Size::new(200.0, 100.0),
            )]
        );
        assert_eq!(renderer.quads.len(), 1);
        assert_eq!(renderer.quads[0].bounds, marquee.rectangle());
        assert_eq!(
            renderer.quads[0].bounds.intersection(&renderer.layers[0]),
            Some(Rectangle::new(
                Point::new(20.0, 40.0),
                Size::new(100.0, 70.0),
            ))
        );
    }
}
