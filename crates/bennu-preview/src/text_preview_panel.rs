//! 文本/Markdown 预览面板：自 app-ui view/text_preview_panel.rs 下沉。
//! 查看器（Paragraph 泛型部件）由宿主实例化注入；Markdown 渲染的滚动接线经
//! ScrollRegionWiring 注入。查看器自己就是滚动区，外面只包细线滑块。

use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};
use bennu_theme::scrollbar::{enhanced_scrollbar, enhanced_scrollbar_both, ScrollbarAxis};

use crate::markdown_preview::markdown_preview_body;
use crate::panel_text::readable_text;
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use crate::text_preview::{
    MarkdownModeChoice, MarkdownPreviewMode, TextPreviewDocument, TextPreviewFormat,
};
use crate::text_status_bar::text_status_bar;
use crate::text_view::TextViewSettings;
use bennu_localization::tr;
use iced::widget::{column, Column};
use iced::Element;

pub fn text_preview_panel<'a, Message>(
    document: Option<&'a TextPreviewDocument>,
    view: TextViewSettings,
    plain_viewer: impl Fn(&'a TextPreviewDocument, TextViewSettings) -> Element<'a, Message> + 'a,
    markdown_scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let Some(document) = document else {
        return column![readable_text(tr!("Text preview is not ready"))];
    };
    // 不换行时多一条横向细线滑块。
    let raw = || {
        let viewer = plain_viewer(document, view);
        if view.wrap {
            enhanced_scrollbar(viewer, ScrollbarAxis::Vertical)
        } else {
            enhanced_scrollbar_both(viewer)
        }
    };
    let mut rendered = false;
    let body: Element<'a, Message> = match document.format() {
        TextPreviewFormat::Plain => raw(),
        TextPreviewFormat::Markdown => {
            let choice = MarkdownModeChoice::for_document(document);
            let body = match (choice, document.markdown()) {
                (MarkdownModeChoice::Switchable(MarkdownPreviewMode::Rendered), Some(markdown)) => {
                    rendered = true;
                    markdown_preview_body(markdown, markdown_scroll)
                }
                _ => raw(),
            };
            column![markdown_mode_switch(choice), body]
                .spacing(8)
                .into()
        }
    };

    let mut panel = column![body];
    if let Some(error) = document.read_error() {
        panel = panel.push(readable_text(tr!(
            "Could not load more text preview: %1",
            error
        )));
    }
    // 渲染模式不按行排版，换行开关不适用。
    panel.push(text_status_bar(document, view, !rendered))
}

/// 渲染/原文切换行。超过 2 MB 的 Markdown 没有渲染全文，「渲染」置灰并说明原因。
fn markdown_mode_switch<Message>(choice: MarkdownModeChoice) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let select = |mode| {
        Some(Message::from(PreviewMessage::MarkdownPreviewModeSelected(
            mode,
        )))
    };
    let (selected, rendered_message, rendered_tooltip) = match choice {
        MarkdownModeChoice::Switchable(mode) => (mode, select(MarkdownPreviewMode::Rendered), None),
        MarkdownModeChoice::RawOnly => (
            MarkdownPreviewMode::Raw,
            None,
            Some(tr!(
                "This file is larger than 2 MB, so only the raw text is shown."
            )),
        ),
    };
    segmented_choice_row(vec![
        SegmentedChoice {
            label: tr!("Rendered"),
            selected: selected == MarkdownPreviewMode::Rendered,
            message: rendered_message,
            tooltip: rendered_tooltip,
        },
        SegmentedChoice {
            label: tr!("Raw"),
            selected: selected == MarkdownPreviewMode::Raw,
            message: select(MarkdownPreviewMode::Raw),
            tooltip: None,
        },
    ])
}
