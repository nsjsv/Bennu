//! 输入框里按 Esc 时发一条消息（输入行用它把草稿恢复成已保存的值）。
//!
//! iced 的 text_input 聚焦时收到 Escape 会自己失焦并捕获这次事件，没聚焦时不碰它
//! （iced_widget 0.14 text_input.rs 的 `Named::Escape` 分支）。所以“这次 Escape 被
//! 里面的内容捕获了”就等于“刚才有输入框在编辑”，不需要另外记录焦点状态。

use iced::advanced::{layout, mouse, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::keyboard::{self, key};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// `content` 里的输入框被聚焦时按 Esc → 发 `message`。
pub fn on_escape<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    message: Message,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(OnEscape {
        content: content.into(),
        message,
    })
}

struct OnEscape<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    message: Message,
}

/// 判定只看事件本身和“里面的内容是否刚捕获了它”：捕获状态在整棵树的一次分发里
/// 是共享的，前面的兄弟控件已经捕获时不能算到这里头上。
fn escape_taken_by_content(event: &Event, captured_before: bool, captured_after: bool) -> bool {
    let is_escape = matches!(
        event,
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key::Named::Escape),
            ..
        })
    );
    is_escape && !captured_before && captured_after
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for OnEscape<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced::advanced::Renderer,
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
        let captured_before = shell.is_event_captured();
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
        if escape_taken_by_content(event, captured_before, shell.is_event_captured()) {
            shell.publish(self.message.clone());
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
    use super::escape_taken_by_content;
    use iced::keyboard::{self, key, Key, Location, Modifiers};
    use iced::Event;

    fn key_press(key: Key) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            key,
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn escape_captured_by_the_content_is_taken() {
        let escape = key_press(Key::Named(key::Named::Escape));
        assert!(escape_taken_by_content(&escape, false, true));
    }

    #[test]
    fn escape_nobody_captured_is_left_for_the_window() {
        let escape = key_press(Key::Named(key::Named::Escape));
        assert!(!escape_taken_by_content(&escape, false, false));
    }

    #[test]
    fn escape_captured_before_reaching_the_content_is_not_ours() {
        let escape = key_press(Key::Named(key::Named::Escape));
        assert!(!escape_taken_by_content(&escape, true, true));
    }

    #[test]
    fn other_captured_keys_are_ignored() {
        let enter = key_press(Key::Named(key::Named::Enter));
        assert!(!escape_taken_by_content(&enter, false, true));
    }
}
