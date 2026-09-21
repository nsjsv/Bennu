use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

const POPUP_GAP: f32 = 4.0;

pub(crate) fn anchored_popup<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
    })
}

struct AnchoredPopup<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    anchor: Element<'a, Message, Theme, Renderer>,
    popup: Option<Element<'a, Message, Theme, Renderer>>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for AnchoredPopup<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn children(&self) -> Vec<widget::Tree> {
        let mut children = vec![widget::Tree::new(&self.anchor)];
        if let Some(popup) = &self.popup {
            children.push(widget::Tree::new(popup));
        }
        children
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let mut children = Vec::with_capacity(1 + usize::from(self.popup.is_some()));
        children.push(self.anchor.as_widget());
        if let Some(popup) = &self.popup {
            children.push(popup.as_widget());
        }
        tree.diff_children(&children);
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
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
        self.anchor
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
        self.anchor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.anchor.as_widget().mouse_interaction(
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
        self.anchor.as_widget().draw(
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
        let mut overlays = Vec::new();
        let mut children = tree.children.iter_mut();

        if let Some(anchor_tree) = children.next() {
            if let Some(anchor_overlay) = self.anchor.as_widget_mut().overlay(
                anchor_tree,
                layout,
                renderer,
                viewport,
                translation,
            ) {
                overlays.push(anchor_overlay);
            }
        }

        if let (Some(popup), Some(popup_tree)) = (self.popup.as_mut(), children.next()) {
            let anchor_bounds =
                Rectangle::new(layout.position() + translation, layout.bounds().size());
            overlays.push(overlay::Element::new(Box::new(AnchoredPopupOverlay {
                popup,
                state: popup_tree,
                anchor_bounds,
            })));
        }

        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

struct AnchoredPopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    popup: &'b mut Element<'a, Message, Theme, Renderer>,
    state: &'b mut widget::Tree,
    anchor_bounds: Rectangle,
}

impl<'a, 'b, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for AnchoredPopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let target_width = self.anchor_bounds.width.min(bounds.width).max(0.0);
        let below_y = self.anchor_bounds.y + self.anchor_bounds.height + POPUP_GAP;
        let space_below = (bounds.height - below_y).max(0.0);
        let space_above = (self.anchor_bounds.y - POPUP_GAP).max(0.0);
        let show_below = space_below >= space_above;
        let max_height = if show_below { space_below } else { space_above };
        let limits = layout::Limits::new(Size::ZERO, Size::new(target_width, max_height))
            .width(target_width);
        let node = self
            .popup
            .as_widget_mut()
            .layout(self.state, renderer, &limits);
        let size = node.size();
        let x = self
            .anchor_bounds
            .x
            .max(0.0)
            .min((bounds.width - size.width).max(0.0));
        let desired_y = if show_below {
            below_y
        } else {
            self.anchor_bounds.y - POPUP_GAP - size.height
        };
        let y = desired_y
            .max(0.0)
            .min((bounds.height - size.height).max(0.0));
        node.move_to(Point::new(x, y))
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();

        self.popup.as_widget_mut().update(
            self.state, event, layout, cursor, renderer, clipboard, shell, &bounds,
        )
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        self.popup
            .as_widget()
            .mouse_interaction(self.state, layout, cursor, &bounds, renderer)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        self.popup
            .as_widget()
            .draw(self.state, renderer, theme, style, layout, cursor, &bounds);
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.popup
            .as_widget_mut()
            .operate(self.state, layout, renderer, operation);
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'c>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        let bounds = layout.bounds();
        self.popup
            .as_widget_mut()
            .overlay(self.state, layout, renderer, &bounds, Vector::ZERO)
    }
}
