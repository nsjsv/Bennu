use iced::advanced::text;
use iced::advanced::{layout, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size};

use crate::measured_text::measured_text_natural_width;

use super::model::allocate_breadcrumb_widths;

pub const ADDRESS_BAR_HEIGHT: f32 = 34.0;
pub const ADDRESS_TEXT_SIZE: u32 = 14;
pub const BREADCRUMB_ICON_SIZE: f32 = 16.0;
pub const BREADCRUMB_SEPARATOR_SIZE: f32 = 13.0;
pub const BREADCRUMB_SEPARATOR_WIDTH: f32 = 17.0;
pub const BREADCRUMB_HOME_WIDTH: f32 = 30.0;
pub const BREADCRUMB_HORIZONTAL_PADDING: f32 = 7.0;
pub const BREADCRUMB_MINIMUM_TEXT_WIDTH: f32 = 58.0;

/// 面包屑段的测量口径：Home 段固定宽，文本段按自然宽度参与弹性分配。
pub enum BreadcrumbMeasurement {
    Home,
    Text(String),
}

/// 弹性面包屑布局 widget：按子元素自然宽度与保底宽度分配视口宽度。
/// 它不产生消息、不解释子元素内容——段按钮/分隔符由各前端自行组装，
/// 这里只持有 children 并按“段/分隔符交错”的约定布局。
pub struct ElasticBreadcrumbs<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: text::Renderer,
{
    children: Vec<Element<'a, Message, Theme, Renderer>>,
    measurements: Vec<BreadcrumbMeasurement>,
    viewport_width: f32,
}

impl<'a, Message, Theme, Renderer> ElasticBreadcrumbs<'a, Message, Theme, Renderer>
where
    Renderer: text::Renderer,
{
    pub fn new(
        children: Vec<Element<'a, Message, Theme, Renderer>>,
        measurements: Vec<BreadcrumbMeasurement>,
        viewport_width: f32,
    ) -> Self {
        Self {
            children,
            measurements,
            viewport_width,
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ElasticBreadcrumbs<'_, Message, Theme, Renderer>
where
    Renderer: text::Renderer,
{
    fn children(&self) -> Vec<widget::Tree> {
        self.children.iter().map(widget::Tree::new).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Fixed(ADDRESS_BAR_HEIGHT))
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let natural_widths = self
            .measurements
            .iter()
            .map(|measurement| match measurement {
                BreadcrumbMeasurement::Home => BREADCRUMB_HOME_WIDTH,
                BreadcrumbMeasurement::Text(label) => {
                    measured_text_natural_width(renderer, label, ADDRESS_TEXT_SIZE)
                        + BREADCRUMB_HORIZONTAL_PADDING * 2.0
                }
            })
            .collect::<Vec<_>>();
        let minimum_widths = self
            .measurements
            .iter()
            .zip(&natural_widths)
            .map(|(measurement, natural_width)| match measurement {
                BreadcrumbMeasurement::Home => *natural_width,
                BreadcrumbMeasurement::Text(_) => natural_width.min(BREADCRUMB_MINIMUM_TEXT_WIDTH),
            })
            .collect::<Vec<_>>();
        let separator_total_width =
            BREADCRUMB_SEPARATOR_WIDTH * self.measurements.len().saturating_sub(1) as f32;
        let allocation = allocate_breadcrumb_widths(
            &natural_widths,
            &minimum_widths,
            separator_total_width,
            self.viewport_width,
        );

        let mut segment_index = 0usize;
        let mut child_offset_x = 0.0;
        let mut positioned_nodes = Vec::with_capacity(self.children.len());
        for (child_index, (child, child_tree)) in
            self.children.iter_mut().zip(&mut tree.children).enumerate()
        {
            let child_width = if child_index % 2 == 0 {
                let width = allocation.segment_widths[segment_index];
                segment_index += 1;
                width
            } else {
                BREADCRUMB_SEPARATOR_WIDTH
            };
            let child_limits = layout::Limits::new(
                Size::new(child_width, ADDRESS_BAR_HEIGHT),
                Size::new(child_width, ADDRESS_BAR_HEIGHT),
            );
            let child_node = child
                .as_widget_mut()
                .layout(child_tree, renderer, &child_limits)
                .move_to(Point::new(child_offset_x, 0.0));
            child_offset_x += child_width;
            positioned_nodes.push(child_node);
        }

        let resolved_height = limits
            .resolve(
                Length::Shrink,
                Length::Fixed(ADDRESS_BAR_HEIGHT),
                Size::ZERO,
            )
            .height;
        layout::Node::with_children(
            Size::new(allocation.content_width, resolved_height),
            positioned_nodes,
        )
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, child_tree), child_layout) in self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                child
                    .as_widget_mut()
                    .operate(child_tree, child_layout, renderer, operation);
            }
        });
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
        for ((child, child_tree), child_layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                child_tree,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, child_tree), child_layout)| {
                child.as_widget().mouse_interaction(
                    child_tree,
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, child_tree), child_layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            child.as_widget().draw(
                child_tree,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }
}
