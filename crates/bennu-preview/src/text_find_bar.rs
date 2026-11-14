//! 文本预览上方的搜索条 / 跳行条。
//!
//! 条里的输入框聚焦时，Esc、Enter、Shift+Enter、F3、Ctrl+F、Ctrl+G 由外面一层先接住：
//! 输入框自己只认 Enter（不分 Shift），Esc 会被它吃掉只失焦。焦点在不在条里，由这一层
//! 对自己的子树跑一次焦点操作得出，不另记状态。

use bennu_localization::tr;
use bennu_theme::controls::{icon_button, text_field, toggle_button_style, tooltip_bubble};
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::advanced::{layout, mouse, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::keyboard::{self, key};
use iced::widget::{button, container, row, text, tooltip};
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Task, Vector};

use crate::preview_message::PreviewMessage;
use crate::text_document::{
    FindBar, FindEvent, LineCountStatus, SearchDirection, SearchSummary, TextPreviewDocument,
    TextViewerEvent,
};

const ICON_SIZE: f32 = 14.0;

/// 条里输入框的部件 id（搜索词与行号共用：同一时间只有一个条）。
pub(crate) fn find_input_id() -> widget::Id {
    widget::Id::new("text-preview-find-input")
}

/// 打开条后键盘给输入框，已有的文字全选（直接打字就替换）。
pub(crate) fn focus_find_input<T: Send + 'static>() -> Task<T> {
    Task::batch([
        iced::widget::operation::focus(find_input_id()),
        iced::widget::operation::select_all(find_input_id()),
    ])
}

/// 条里的输入框没有焦点时才发 `message`。宿主处理「被捕获的 Space 关预览」前用它确认
/// 这一下不是在输入框里打字。
pub fn unless_find_input_focused<T>(message: T) -> Task<T>
where
    T: Clone + Send + 'static,
{
    iced::advanced::widget::operate(FindInputFocusCheck {
        target: find_input_id(),
        focused: false,
        message,
    })
}

struct FindInputFocusCheck<T> {
    target: widget::Id,
    focused: bool,
    message: T,
}

impl<T: Clone + Send + 'static> Operation<T> for FindInputFocusCheck<T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if id == Some(&self.target) && state.is_focused() {
            self.focused = true;
        }
    }

    fn finish(&self) -> Outcome<T> {
        if self.focused {
            Outcome::None
        } else {
            Outcome::Some(self.message.clone())
        }
    }
}

fn find_message<Message: From<PreviewMessage>>(event: FindEvent) -> Message {
    Message::from(PreviewMessage::TextViewer(TextViewerEvent::Find(event)))
}

/// 条开着时的条；没开时 None。
pub(crate) fn text_find_bar<'a, Message>(
    document: &'a TextPreviewDocument,
) -> Option<Element<'a, Message>>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let step = |direction| find_message(FindEvent::Step(direction));
    let (content, enter, shift_enter): (Element<'a, Message>, Message, Message) =
        match document.find_bar()? {
            FindBar::Search => (
                search_row(document),
                step(SearchDirection::Forward),
                step(SearchDirection::Backward),
            ),
            FindBar::GotoLine { input } => (
                goto_row(document, input),
                find_message(FindEvent::GotoSubmitted),
                find_message(FindEvent::GotoSubmitted),
            ),
        };
    let bar = container(content)
        .padding(Padding::from([4, 8]))
        .width(Length::Fill)
        .style(|theme| container::Style {
            background: Some(ui_colors(theme).surface_container_low.into()),
            text_color: Some(ui_colors(theme).on_surface_variant),
            ..container::Style::default()
        });
    Some(Element::new(FindBarKeys {
        content: bar.into(),
        keys: FindKeys {
            escape: find_message(FindEvent::Close),
            enter,
            shift_enter,
            next: step(SearchDirection::Forward),
            previous: step(SearchDirection::Backward),
            open_search: find_message(FindEvent::OpenSearch { seed: None }),
            open_goto: find_message(FindEvent::OpenGoto),
        },
    }))
}

fn search_row<'a, Message>(document: &'a TextPreviewDocument) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let query = document.search_query();
    let searching = document.search_summary().is_some();
    let case = tooltip(
        button(text("Aa").size(CAPTION_SIZE))
            .padding([3, 8])
            .style(toggle_button_style(query.case_sensitive))
            .on_press(find_message(FindEvent::CaseToggled)),
        tooltip_bubble(tr!("Match case")),
        tooltip::Position::Bottom,
    );
    let arrow = |symbol, direction| {
        icon_button(themed_icon(symbol, IconTone::Normal, ICON_SIZE))
            .on_press_maybe(searching.then(|| find_message(FindEvent::Step(direction))))
    };
    row![
        text_field(&tr!("Search"), &query.text)
            .id(find_input_id())
            .size(CAPTION_SIZE)
            .on_input(|text| find_message(FindEvent::QueryChanged(text)))
            .width(Length::Fill),
        case,
        text(summary_label(document.search_summary())).size(CAPTION_SIZE),
        arrow(IconSymbol::ArrowUp, SearchDirection::Backward),
        arrow(IconSymbol::ArrowDown, SearchDirection::Forward),
        close_button(),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn goto_row<'a, Message>(document: &'a TextPreviewDocument, input: &str) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let placeholder = match document.line_count_status() {
        LineCountStatus::Done { total } => tr!("Line number (1–%1)", total),
        LineCountStatus::Counting { .. } => tr!("Line number (counting lines…)"),
    };
    let mut bar = row![
        text(tr!("Go to line")).size(CAPTION_SIZE),
        text_field(&placeholder, input)
            .id(find_input_id())
            .size(CAPTION_SIZE)
            .on_input(|input| find_message(FindEvent::GotoInputChanged(input)))
            .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if document.goto_in_flight() {
        bar = bar.push(text(tr!("Finding the line…")).size(CAPTION_SIZE));
    }
    bar.push(close_button()).into()
}

fn close_button<'a, Message>() -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    icon_button(themed_icon(IconSymbol::Close, IconTone::Normal, ICON_SIZE))
        .on_press(find_message(FindEvent::Close))
        .into()
}

/// 「3/17」；还没搜完加「+」；当前匹配的序号还没数到是「–」。
pub(crate) fn summary_label(summary: Option<SearchSummary>) -> String {
    let Some(summary) = summary else {
        return String::new();
    };
    if summary.finished && summary.total == 0 {
        return tr!("No results");
    }
    if !summary.finished && summary.total == 0 && summary.ordinal.is_none() {
        return tr!("Searching…");
    }
    let ordinal = summary
        .ordinal
        .map_or_else(|| "–".to_owned(), |ordinal| ordinal.to_string());
    let more = if summary.finished { "" } else { "+" };
    format!("{ordinal}/{}{more}", summary.total)
}

#[derive(Clone)]
struct FindKeys<Message> {
    escape: Message,
    enter: Message,
    shift_enter: Message,
    next: Message,
    previous: Message,
    open_search: Message,
    open_goto: Message,
}

impl<Message: Clone> FindKeys<Message> {
    fn message_for(&self, key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
        let message = match key.as_ref() {
            keyboard::Key::Named(key::Named::Escape) => &self.escape,
            keyboard::Key::Named(key::Named::Enter) if modifiers.shift() => &self.shift_enter,
            keyboard::Key::Named(key::Named::Enter) => &self.enter,
            keyboard::Key::Named(key::Named::F3) if modifiers.shift() => &self.previous,
            keyboard::Key::Named(key::Named::F3) => &self.next,
            keyboard::Key::Character(character) if modifiers.command() => {
                match character.to_ascii_lowercase().as_str() {
                    "f" => &self.open_search,
                    "g" => &self.open_goto,
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(message.clone())
    }
}

/// 条里的输入框聚焦时先接住条的快捷键。
struct FindBarKeys<'a, Message> {
    content: Element<'a, Message>,
    keys: FindKeys<Message>,
}

/// 子树里有没有聚焦的控件。
struct FocusProbe {
    focused: bool,
}

impl Operation for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        self.focused |= state.is_focused();
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for FindBarKeys<'_, Message> {
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
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
            if let Some(message) = self.keys.message_for(key, *modifiers) {
                let mut probe = FocusProbe { focused: false };
                self.content.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    renderer,
                    &mut probe,
                );
                if probe.focused && !shell.is_event_captured() {
                    shell.publish(message);
                    shell.capture_event();
                    return;
                }
            }
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

#[cfg(test)]
#[path = "text_find_bar_tests.rs"]
mod tests;
