use iced::advanced::{layout, widget, Clipboard, Layout, Shell, Widget};
use iced::{Element, Length, Point, Rectangle, Size, Vector};

/// 将内容按任意位移（可为负）布局并绘制，自身占满父级。
/// 不使用 iced 的 `pin`：pin 会用 `max - position` 收缩内容 limits，
/// Fixed 尺寸的图片被拖向右下方时会被意外压缩；这里直接保留完整 limits。
pub(crate) fn translated_surface<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    translation: Vector,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(TranslatedSurface {
        content: content.into(),
        translation,
    })
}

struct TranslatedSurface<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    translation: Vector,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for TranslatedSurface<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Message: Clone,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // iced 会把 Length::Fixed 钳制到 limits.max()；缩放后的图片大于
        // 面板，若沿用面板 limits，Fixed 尺寸被压回面板大小，放大就退化
        // 成位移。内容必须用无界 limits 布局，自身仍占满父级。
        let unbounded = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));
        let node = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &unbounded);
        let node = node.move_to(Point::new(self.translation.x, self.translation.y));
        layout::Node::with_children(limits.max(), vec![node])
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(
            tree,
            layout.children().next().expect("translated child"),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().expect("translated child"),
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
        renderer: &Renderer,
    ) -> iced::mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().expect("translated child"),
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
        renderer_style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Some(clipped_viewport) = layout.bounds().intersection(viewport) else {
            return;
        };
        // 媒体面板边界是缩放内容唯一的像素裁剪层：窗口表面只在窗口边缘
        // 裁剪，缺了这一层放大后的图片会盖住窗口内其余 UI。with_layer 在
        // 原绘制时序内推层，chrome 顶部栏与底部控件在其后绘制，仍在图上。
        renderer.with_layer(clipped_viewport, |renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                renderer_style,
                layout.children().next().expect("translated child"),
                cursor,
                &clipped_viewport,
            );
        });
    }
}
