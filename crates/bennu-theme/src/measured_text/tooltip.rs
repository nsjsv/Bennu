use iced::advanced::{layout, overlay, renderer, text, widget, Clipboard, Layout, Shell, Widget};
use iced::widget::{tooltip, Tooltip};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use super::{
    displayed_content_is_ellipsized, MeasuredMiddleEllipsizedText,
    MeasuredMiddleEllipsizedTextState,
};

/// 文字被省略了，鼠标停一会儿（`TOOLTIP_DELAY`）显示 `hint`；没被省略的不出提示。
pub(super) fn with_hint<'a, Message: 'a>(
    text: MeasuredMiddleEllipsizedText,
    hint: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let content = text.content.clone();
    let tooltip = crate::controls::tooltip(text, hint, tooltip::Position::Bottom);
    Element::new(EllipsizedTextTooltip { content, tooltip })
}

struct EllipsizedTextTooltip<'a, Message> {
    content: String,
    tooltip: Tooltip<'a, Message>,
}

struct EllipsizedTextTooltipState {
    content: String,
}

impl<Message> EllipsizedTextTooltip<'_, Message> {
    fn is_ellipsized(&self, tree: &widget::Tree) -> bool {
        tree.children
            .first()
            .and_then(|tooltip_tree| tooltip_tree.children.first())
            .map(|text_tree| {
                let state = text_tree
                    .state
                    .downcast_ref::<MeasuredMiddleEllipsizedTextState<
                        <iced::Renderer as text::Renderer>::Paragraph,
                        <iced::Renderer as text::Renderer>::Font,
                    >>();

                displayed_content_is_ellipsized(&self.content, &state.displayed_content)
            })
            .unwrap_or(false)
    }

    fn reset_tooltip_state(&self, tree: &mut widget::Tree) {
        let tooltip: &dyn Widget<Message, iced::Theme, iced::Renderer> = &self.tooltip;
        let fresh_tooltip_tree = widget::Tree::new(tooltip);
        tree.children[0].state = fresh_tooltip_tree.state;
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EllipsizedTextTooltip<'_, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<EllipsizedTextTooltipState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(EllipsizedTextTooltipState {
            content: self.content.clone(),
        })
    }

    fn children(&self) -> Vec<widget::Tree> {
        let tooltip: &dyn Widget<Message, iced::Theme, iced::Renderer> = &self.tooltip;
        vec![widget::Tree::new(tooltip)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let content_changed = tree
            .state
            .downcast_ref::<EllipsizedTextTooltipState>()
            .content
            != self.content;
        let tooltip: &dyn Widget<Message, iced::Theme, iced::Renderer> = &self.tooltip;
        tree.diff_children(&[tooltip]);

        if content_changed {
            self.reset_tooltip_state(tree);
            tree.state
                .downcast_mut::<EllipsizedTextTooltipState>()
                .content
                .clone_from(&self.content);
        }
    }

    fn size(&self) -> Size<Length> {
        self.tooltip.size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.tooltip.size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self.tooltip.layout(&mut tree.children[0], renderer, limits);

        if !self.is_ellipsized(tree) {
            self.reset_tooltip_state(tree);
        }

        node
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let cursor = if self.is_ellipsized(tree) {
            cursor
        } else {
            self.reset_tooltip_state(tree);
            iced::mouse::Cursor::Unavailable
        };

        self.tooltip.update(
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
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::mouse::Interaction {
        self.tooltip
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        inherited_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.tooltip.draw(
            &tree.children[0],
            renderer,
            theme,
            inherited_style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        if !self.is_ellipsized(tree) {
            self.reset_tooltip_state(tree);
        }

        self.tooltip.overlay(
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
    use super::super::measured_middle_ellipsized_centered_text_with_hint;
    use super::EllipsizedTextTooltipState;
    use iced::advanced::widget::Tree;
    use iced::Element;

    #[test]
    fn diff_tracks_content_identity_for_delay_reset() {
        let first: Element<'_, ()> = measured_middle_ellipsized_centered_text_with_hint(
            "first-long-name.txt",
            12.0,
            16.0,
            "first tooltip",
        );
        let mut tree = Tree::new(first.as_widget());
        let second: Element<'_, ()> = measured_middle_ellipsized_centered_text_with_hint(
            "second-long-name.txt",
            12.0,
            16.0,
            "second tooltip",
        );

        tree.diff(second.as_widget());

        assert_eq!(
            tree.state
                .downcast_ref::<EllipsizedTextTooltipState>()
                .content,
            "second-long-name.txt"
        );
    }
}
