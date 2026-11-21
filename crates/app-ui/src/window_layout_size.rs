//! 主窗口排出来有多大：盖在窗口整棵界面外面的一层，排版时记下大小，变了就在下一次重画时报出去。
//!
//! 不用窗口的 Resized 事件：事件里的大小按换算那一刻的界面缩放（字号）折算，送到应用时字号可能
//! 已经换了（启动时窗口先按默认字号开，读到设置才换成存下的字号），按它算的栏宽、网格就一直偏。
//! 排版用的就是此刻的缩放，这里记下的和画出来的一致；字号一换，重新排版时自然报出新的大小。

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{window, Element, Event, Length, Rectangle, Size, Vector};

pub(crate) fn report_window_layout_size<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_change: fn(Size) -> Message,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(WindowLayoutSize {
        content: content.into(),
        on_change,
    })
}

struct WindowLayoutSize<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_change: fn(Size) -> Message,
}

#[derive(Debug, Default)]
struct SizeState {
    laid_out: Size,
    reported: Option<Size>,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for WindowLayoutSize<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SizeState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SizeState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
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
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        tree.state.downcast_mut::<SizeState>().laid_out = node.size();
        node
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
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
        // 排版里不能发消息：每次画之前都先有一次重画事件，在这里看大小变没变。
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            let state = tree.state.downcast_mut::<SizeState>();
            if state.reported != Some(state.laid_out) {
                state.reported = Some(state.laid_out);
                shell.publish((self.on_change)(state.laid_out));
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
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
        tree: &Tree,
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
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
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
    use iced::advanced::layout::Limits;
    use iced::advanced::widget::Tree;
    use iced::advanced::{clipboard, mouse, Layout, Shell};
    use iced::widget::Space;
    use iced::{window, Element, Event, Length, Rectangle, Size, Theme};

    use super::report_window_layout_size;

    /// 排出来的大小变了才报，报一次；同样大小再重画不报。
    #[test]
    fn reports_the_laid_out_size_once_per_change() {
        let mut root: Element<'static, Size, Theme, ()> = report_window_layout_size(
            Space::new().width(Length::Fill).height(Length::Fill),
            |size| size,
        );
        let mut tree = Tree::new(&root);
        let redraw =
            |root: &mut Element<'static, Size, Theme, ()>, tree: &mut Tree, size: Size| {
                let node = root
                    .as_widget_mut()
                    .layout(tree, &(), &Limits::new(Size::ZERO, size));
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                root.as_widget_mut().update(
                    tree,
                    &Event::Window(window::Event::RedrawRequested(std::time::Instant::now())),
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &(),
                    &mut clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(size),
                );
                messages
            };

        assert_eq!(
            redraw(&mut root, &mut tree, Size::new(1016.0, 1108.0)),
            [Size::new(1016.0, 1108.0)]
        );
        assert!(redraw(&mut root, &mut tree, Size::new(1016.0, 1108.0)).is_empty());
        // 字号换大以后同一个窗口排出来的布局大小变小，照样报。
        assert_eq!(
            redraw(&mut root, &mut tree, Size::new(943.4, 1028.9)),
            [Size::new(943.4, 1028.9)]
        );
    }
}
