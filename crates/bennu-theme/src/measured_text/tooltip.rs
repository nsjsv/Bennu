//! 被省略的文字才给的提示：放不下、中间省略了，鼠标停一会儿显示 `hint`（完整的名字）；没被
//! 省略的不出提示。等待、按下就收这些都在 `controls::gated_tooltip`。

use iced::advanced::{text, widget};
use iced::widget::tooltip;
use iced::Element;

use super::{
    displayed_content_is_ellipsized, MeasuredMiddleEllipsizedText,
    MeasuredMiddleEllipsizedTextState,
};

pub(super) fn with_hint<'a, Message: 'a>(
    text: MeasuredMiddleEllipsizedText,
    hint: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let content = text.content.clone();
    let full_text = content.clone();
    crate::controls::gated_tooltip(
        Element::new(text),
        hint,
        tooltip::Position::Bottom,
        content,
        move |text_tree: &widget::Tree| {
            let state = text_tree
                .state
                .downcast_ref::<MeasuredMiddleEllipsizedTextState<
                    <iced::Renderer as text::Renderer>::Paragraph,
                    <iced::Renderer as text::Renderer>::Font,
                >>();
            displayed_content_is_ellipsized(&full_text, &state.displayed_content)
        },
    )
}
