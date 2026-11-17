//! 音频、视频、图片预览的键盘：包住面板的可聚焦壳。音视频：回车播放/暂停，←/→ 后退/前进 5 秒
//! （Shift 加按 30 秒），↑/↓ 音量 ±5%，F 视频进出全屏（Esc 退全屏归宿主的 Esc 路由），I 视频信息层。
//! 图片：+/- 缩放一档，0 适应窗口，1 原始大小，I 信息层，←/→ 上一张/下一张。空格不接，照旧开关预览窗口。点一下面板才聚焦
//! （独立预览窗口打开时由 `focus_preview_keyboard` 聚焦），点别处失焦——右侧面板里
//! 方向键默认还归文件列表。

use std::time::Duration;

use bennu_theme::smooth_scroll::{wheel_direction, WheelDirection};
use iced::advanced::widget::{self, operation, Operation};
use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::keyboard::{self, key::Named};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use crate::document_preview::{
    DocumentPageTurn, DocumentPreviewMessage, DocumentTextMessage, DocumentZoom,
};
use crate::image_preview_viewport::PreviewImageViewportMessage;
use crate::preview_message::{ImageStep, MediaKeyCommand, PreviewMessage};
use crate::text_document::SearchDirection;

const SHORT_SEEK: Duration = Duration::from_secs(5);
const LONG_SEEK: Duration = Duration::from_secs(30);
const VOLUME_STEP: f32 = 0.05;

#[derive(Debug, Default)]
pub(crate) struct MediaKeysState {
    pub(crate) is_focused: bool,
    modifiers: keyboard::Modifiers,
}

impl operation::Focusable for MediaKeysState {
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

pub(crate) fn media_keys<'a, Message>(content: Element<'a, Message>) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(MediaKeys {
        content,
        keymap: Box::new(|key, modifiers| {
            media_key_command(key, modifiers).map(PreviewMessage::MediaKey)
        }),
        wheel: None,
    })
}

pub(crate) fn image_keys<'a, Message>(content: Element<'a, Message>) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(MediaKeys {
        content,
        keymap: Box::new(image_key_message),
        wheel: None,
    })
}

/// 文档：+/-/0（带不带 Ctrl 都行）缩放，Ctrl+滚轮缩放，PageUp/PageDown/Home/End 翻页，
/// Ctrl+F 搜索、F3/Shift+F3 下一处/上一处、Ctrl+C 复制选中的文字。
pub(crate) fn document_keys<'a, Message>(content: Element<'a, Message>) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(MediaKeys {
        content,
        keymap: Box::new(document_key_message),
        wheel: Some(document_wheel_message),
    })
}

/// 文件夹（只在独立预览窗口里套）：↑/↓ 选行，→ 展开，← 收起或回上一层，回车打开。
/// 键盘移动后要把选中行滚进视口，滚动区 id 随消息带给引擎。
pub(crate) fn folder_keys<'a, Message>(
    content: Element<'a, Message>,
    scroll_region_id: iced::widget::Id,
) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(MediaKeys {
        content,
        keymap: Box::new(move |key, modifiers| {
            folder_key(key, modifiers).map(|key| {
                PreviewMessage::FolderPreview(crate::folder_preview::FolderPreviewMessage::Key(
                    key,
                    scroll_region_id.clone(),
                ))
            })
        }),
        wheel: None,
    })
}

fn folder_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<crate::folder_preview::FolderKey> {
    use crate::folder_preview::FolderKey;
    if modifiers.command() || modifiers.alt() || modifiers.logo() || modifiers.control() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Named(Named::ArrowUp) => Some(FolderKey::Up),
        keyboard::Key::Named(Named::ArrowDown) => Some(FolderKey::Down),
        keyboard::Key::Named(Named::ArrowRight) => Some(FolderKey::Expand),
        keyboard::Key::Named(Named::ArrowLeft) => Some(FolderKey::Collapse),
        keyboard::Key::Named(Named::Enter) => Some(FolderKey::Open),
        _ => None,
    }
}

type Keymap<'a> = Box<dyn Fn(&keyboard::Key, keyboard::Modifiers) -> Option<PreviewMessage> + 'a>;
/// 滚轮先问这个：给出消息就不让里面的滚动区滚（Ctrl+滚轮缩放）。
type WheelMap = fn(mouse::ScrollDelta, keyboard::Modifiers) -> Option<PreviewMessage>;

struct MediaKeys<'a, Message> {
    content: Element<'a, Message>,
    keymap: Keymap<'a>,
    wheel: Option<WheelMap>,
}

/// 带 Ctrl/Alt/Super 的组合键不接，留给应用快捷键。
pub(crate) fn media_key_command(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<MediaKeyCommand> {
    if modifiers.command() || modifiers.alt() || modifiers.logo() || modifiers.control() {
        return None;
    }
    let step = if modifiers.shift() {
        LONG_SEEK
    } else {
        SHORT_SEEK
    };
    match key.as_ref() {
        keyboard::Key::Named(Named::Enter) if !modifiers.shift() => {
            Some(MediaKeyCommand::TogglePlayback)
        }
        keyboard::Key::Named(Named::ArrowRight) => Some(MediaKeyCommand::SeekForward(step)),
        keyboard::Key::Named(Named::ArrowLeft) => Some(MediaKeyCommand::SeekBackward(step)),
        keyboard::Key::Named(Named::ArrowUp) if !modifiers.shift() => {
            Some(MediaKeyCommand::ChangeVolume(VOLUME_STEP))
        }
        keyboard::Key::Named(Named::ArrowDown) if !modifiers.shift() => {
            Some(MediaKeyCommand::ChangeVolume(-VOLUME_STEP))
        }
        keyboard::Key::Character(character) if character.eq_ignore_ascii_case("f") => {
            Some(MediaKeyCommand::ToggleFullscreen)
        }
        keyboard::Key::Character(character) if character.eq_ignore_ascii_case("i") => {
            Some(MediaKeyCommand::ToggleInfo)
        }
        _ => None,
    }
}

/// 同上，带 Ctrl/Alt/Super 的不接；Shift 照接（多数键盘上 + 要按 Shift）。
pub(crate) fn image_key_message(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<PreviewMessage> {
    if modifiers.command() || modifiers.alt() || modifiers.logo() || modifiers.control() {
        return None;
    }
    let character = match key.as_ref() {
        keyboard::Key::Named(Named::ArrowLeft) => {
            return Some(PreviewMessage::ImageStepRequested(ImageStep::Previous))
        }
        keyboard::Key::Named(Named::ArrowRight) => {
            return Some(PreviewMessage::ImageStepRequested(ImageStep::Next))
        }
        keyboard::Key::Character(character) => character,
        _ => return None,
    };
    if character.eq_ignore_ascii_case("i") {
        return Some(PreviewMessage::ImageInfoToggled);
    }
    let viewport = match character {
        "+" | "=" => PreviewImageViewportMessage::ZoomedIn,
        "-" => PreviewImageViewportMessage::ZoomedOut,
        "0" => PreviewImageViewportMessage::FitRequested,
        "1" => PreviewImageViewportMessage::ActualSizeRequested,
        _ => return None,
    };
    Some(PreviewMessage::PreviewImageViewport(viewport))
}

pub(crate) fn document_key_message(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<PreviewMessage> {
    if modifiers.alt() || modifiers.logo() {
        return None;
    }
    let text = |message| {
        Some(PreviewMessage::DocumentPreview(
            DocumentPreviewMessage::Text(message),
        ))
    };
    match key.as_ref() {
        keyboard::Key::Character(character) if modifiers.command() => {
            match character.to_ascii_lowercase().as_str() {
                "f" => return text(DocumentTextMessage::SearchOpened),
                "c" => return text(DocumentTextMessage::CopyRequested),
                _ => {}
            }
        }
        keyboard::Key::Named(Named::F3) => {
            let direction = if modifiers.shift() {
                SearchDirection::Backward
            } else {
                SearchDirection::Forward
            };
            return text(DocumentTextMessage::Stepped(direction));
        }
        _ => {}
    }
    let message = match key.as_ref() {
        keyboard::Key::Named(Named::PageDown) => {
            DocumentPreviewMessage::PageTurned(DocumentPageTurn::Next)
        }
        keyboard::Key::Named(Named::PageUp) => {
            DocumentPreviewMessage::PageTurned(DocumentPageTurn::Previous)
        }
        keyboard::Key::Named(Named::Home) => {
            DocumentPreviewMessage::PageTurned(DocumentPageTurn::First)
        }
        keyboard::Key::Named(Named::End) => {
            DocumentPreviewMessage::PageTurned(DocumentPageTurn::Last)
        }
        keyboard::Key::Character("+" | "=") => DocumentPreviewMessage::Zoomed(DocumentZoom::In),
        keyboard::Key::Character("-") => DocumentPreviewMessage::Zoomed(DocumentZoom::Out),
        keyboard::Key::Character("0") => DocumentPreviewMessage::Zoomed(DocumentZoom::FitWidth),
        _ => return None,
    };
    Some(PreviewMessage::DocumentPreview(message))
}

fn document_wheel_message(
    delta: mouse::ScrollDelta,
    modifiers: keyboard::Modifiers,
) -> Option<PreviewMessage> {
    if !modifiers.command() {
        return None;
    }
    let zoom = match wheel_direction(delta)? {
        WheelDirection::Up => DocumentZoom::In,
        WheelDirection::Down => DocumentZoom::Out,
    };
    Some(PreviewMessage::DocumentPreview(
        DocumentPreviewMessage::Zoomed(zoom),
    ))
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MediaKeys<'_, Message>
where
    Message: Clone + From<PreviewMessage>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<MediaKeysState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(MediaKeysState::default())
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
        let state = tree.state.downcast_mut::<MediaKeysState>();
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
        let state = tree.state.downcast_mut::<MediaKeysState>();
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                state.is_focused = cursor.is_over(layout.bounds());
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(layout.bounds()) =>
            {
                if let Some(message) = self.wheel.and_then(|wheel| wheel(*delta, state.modifiers)) {
                    shell.publish(Message::from(message));
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
        }
        // 里面先接：文档的页码框在打字时，数字和 +/- 归输入框，不当缩放键。
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
        let state = tree.state.downcast_mut::<MediaKeysState>();
        if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
            if state.is_focused && !shell.is_event_captured() {
                if let Some(message) = (self.keymap)(key, *modifiers) {
                    shell.publish(Message::from(message));
                    shell.capture_event();
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn named(key: Named) -> keyboard::Key {
        keyboard::Key::Named(key)
    }

    #[test]
    fn keys_map_to_playback_commands() {
        let none = keyboard::Modifiers::empty();
        let shift = keyboard::Modifiers::SHIFT;
        assert_eq!(
            media_key_command(&named(Named::Enter), none),
            Some(MediaKeyCommand::TogglePlayback)
        );
        assert_eq!(
            media_key_command(&named(Named::ArrowRight), none),
            Some(MediaKeyCommand::SeekForward(SHORT_SEEK))
        );
        assert_eq!(
            media_key_command(&named(Named::ArrowLeft), shift),
            Some(MediaKeyCommand::SeekBackward(LONG_SEEK))
        );
        assert_eq!(
            media_key_command(&named(Named::ArrowDown), none),
            Some(MediaKeyCommand::ChangeVolume(-VOLUME_STEP))
        );
        let character = |text: &str| keyboard::Key::Character(text.into());
        assert_eq!(
            media_key_command(&character("f"), none),
            Some(MediaKeyCommand::ToggleFullscreen)
        );
        assert_eq!(
            media_key_command(&character("I"), shift),
            Some(MediaKeyCommand::ToggleInfo)
        );
        // Ctrl+F 之类留给应用（文本预览的搜索）。
        assert_eq!(
            media_key_command(&character("f"), keyboard::Modifiers::CTRL),
            None
        );
    }

    #[test]
    fn image_keys_zoom_and_leave_chords_alone() {
        let character = |text: &str| keyboard::Key::Character(text.into());
        let zoom = |text: &str, modifiers| match image_key_message(&character(text), modifiers) {
            Some(PreviewMessage::PreviewImageViewport(message)) => Some(format!("{message:?}")),
            _ => None,
        };
        let none = keyboard::Modifiers::empty();
        assert_eq!(
            zoom("+", keyboard::Modifiers::SHIFT).as_deref(),
            Some("ZoomedIn")
        );
        assert_eq!(zoom("=", none).as_deref(), Some("ZoomedIn"));
        assert_eq!(zoom("-", none).as_deref(), Some("ZoomedOut"));
        assert_eq!(zoom("0", none).as_deref(), Some("FitRequested"));
        assert_eq!(zoom("1", none).as_deref(), Some("ActualSizeRequested"));
        assert_eq!(zoom("1", keyboard::Modifiers::CTRL), None);
        assert_eq!(zoom("x", none), None);
        assert!(matches!(
            image_key_message(&named(Named::ArrowRight), none),
            Some(PreviewMessage::ImageStepRequested(ImageStep::Next))
        ));
        assert!(matches!(
            image_key_message(&named(Named::ArrowLeft), none),
            Some(PreviewMessage::ImageStepRequested(ImageStep::Previous))
        ));
        assert!(matches!(
            image_key_message(&character("i"), none),
            Some(PreviewMessage::ImageInfoToggled)
        ));
        assert!(image_key_message(&named(Named::ArrowLeft), keyboard::Modifiers::CTRL).is_none());
    }

    #[test]
    fn document_keys_zoom_and_turn_pages() {
        let none = keyboard::Modifiers::empty();
        let document = |key: &keyboard::Key, modifiers| match document_key_message(key, modifiers) {
            Some(PreviewMessage::DocumentPreview(message)) => Some(format!("{message:?}")),
            _ => None,
        };
        let character = |text: &str| keyboard::Key::Character(text.into());
        assert_eq!(
            document(&named(Named::PageDown), none).as_deref(),
            Some("PageTurned(Next)")
        );
        assert_eq!(
            document(&named(Named::Home), none).as_deref(),
            Some("PageTurned(First)")
        );
        assert_eq!(
            document(&character("="), keyboard::Modifiers::CTRL).as_deref(),
            Some("Zoomed(In)")
        );
        assert_eq!(
            document(&character("0"), none).as_deref(),
            Some("Zoomed(FitWidth)")
        );
        assert!(document_wheel_message(
            mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            keyboard::Modifiers::CTRL
        )
        .is_some());
        // 不按 Ctrl 的滚轮照常滚动。
        assert!(
            document_wheel_message(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }, none).is_none()
        );
        // 按着 Ctrl 时手指离开触控板不是一次缩小。
        assert!(document_wheel_message(
            mouse::ScrollDelta::Pixels { x: 0.0, y: 0.0 },
            keyboard::Modifiers::CTRL
        )
        .is_none());
    }

    #[test]
    fn space_and_shortcut_chords_are_left_to_the_host() {
        let none = keyboard::Modifiers::empty();
        assert_eq!(media_key_command(&named(Named::Space), none), None);
        assert_eq!(
            media_key_command(&named(Named::ArrowLeft), keyboard::Modifiers::CTRL),
            None
        );
        assert_eq!(
            media_key_command(&named(Named::Enter), keyboard::Modifiers::ALT),
            None
        );
    }
}
