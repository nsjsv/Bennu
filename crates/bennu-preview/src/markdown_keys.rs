//! Markdown 渲染模式的键盘：渲染内容不是查看器，外面包一层可聚焦的壳接住搜索与缩放
//! 快捷键（Ctrl+F、F3、Shift+F3、Esc 关搜索条、Ctrl+=/-/0、Ctrl+滚轮）。点一下内容
//! 才聚焦（独立预览窗口打开时由 `focus_preview_keyboard` 聚焦），点别处失焦。

use iced::advanced::widget::{self, operation, Operation};
use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::keyboard;
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use crate::preview_message::PreviewMessage;
use crate::text_document::{FindEvent, SearchDirection, TextViewerEvent};
use crate::text_preview_viewer::view_shortcut;
use crate::text_view::TextViewChange;

#[derive(Debug, Default)]
pub(crate) struct MarkdownKeysState {
    pub(crate) is_focused: bool,
    modifiers: keyboard::Modifiers,
}

impl operation::Focusable for MarkdownKeysState {
    fn is_focused(&self) -> bool {
        self.is_focused
    }

    fn focus(&mut self) {
        self.is_focused = true;
    }

    fn unfocus(&mut self) {
        self.is_focused = false;
    }
}

/// 包住渲染内容；`escape` 是 Esc 要发的消息（搜索条开着时关它，没开时 None，Esc 交给宿主）。
pub(crate) fn markdown_keys<'a, Message>(
    content: Element<'a, Message>,
    escape: Option<Message>,
) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(MarkdownKeys { content, escape })
}

struct MarkdownKeys<'a, Message> {
    content: Element<'a, Message>,
    escape: Option<Message>,
}

fn viewer_message<Message: From<PreviewMessage>>(event: TextViewerEvent) -> Message {
    Message::from(PreviewMessage::TextViewer(event))
}

impl<Message> MarkdownKeys<'_, Message>
where
    Message: Clone + From<PreviewMessage>,
{
    fn key_message(&self, key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
        let find = |event| Some(viewer_message(TextViewerEvent::Find(event)));
        match key.as_ref() {
            keyboard::Key::Named(keyboard::key::Named::Escape) => self.escape.clone(),
            keyboard::Key::Named(keyboard::key::Named::F3) if modifiers.shift() => {
                find(FindEvent::Step(SearchDirection::Backward))
            }
            keyboard::Key::Named(keyboard::key::Named::F3) => {
                find(FindEvent::Step(SearchDirection::Forward))
            }
            keyboard::Key::Character(character)
                if modifiers.command() && character.eq_ignore_ascii_case("f") =>
            {
                find(FindEvent::OpenSearch { seed: None })
            }
            _ => view_shortcut(key, modifiers)
                .map(|change| viewer_message(TextViewerEvent::ChangeView(change))),
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MarkdownKeys<'_, Message>
where
    Message: Clone + From<PreviewMessage>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<MarkdownKeysState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(MarkdownKeysState::default())
    }

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
        renderer: &iced::Renderer,
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
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<MarkdownKeysState>();
        operation.focusable(None, layout.bounds(), state);
        operation.custom(None, layout.bounds(), state);
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
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<MarkdownKeysState>();
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                state.is_focused = cursor.is_over(bounds);
            }
            // Ctrl+滚轮缩放字号，不让里面的滚动区滚。
            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if state.modifiers.command() && cursor.is_over(bounds) =>
            {
                let upward = match delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => {
                        *y > 0.0
                    }
                };
                let change = if upward {
                    TextViewChange::ZoomIn
                } else {
                    TextViewChange::ZoomOut
                };
                shell.publish(viewer_message(TextViewerEvent::ChangeView(change)));
                shell.capture_event();
                return;
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if state.is_focused && !shell.is_event_captured() =>
            {
                if let Some(message) = self.key_message(key, *modifiers) {
                    shell.publish(message);
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
        }
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
        renderer: &iced::Renderer,
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
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
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
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
