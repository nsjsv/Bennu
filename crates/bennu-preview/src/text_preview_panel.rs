//! 文本/Markdown 预览面板：自 app-ui view/text_preview_panel.rs 下沉。
//! 渲染器敏感部件与共用词汇经参数由宿主注入——查看器（Paragraph 泛型
//! 部件，宿主实例化）、模式切换行（segmented_choice_row 是设置窗口
//! 共用词汇，5a 结论留 app-ui）；滚动接线经 ScrollRegionWiring 注入。

use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis, ScrollbarViewport};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, ScrollbarVisibility,
    SCROLLBAR_HOVER_WIDTH,
};

use crate::markdown_preview::markdown_preview_body;
use crate::panel_text::{localized_text, readable_text};
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use crate::text_preview::{
    MarkdownPreviewMode, TextPreviewDocument, TextPreviewFormat, TextPreviewLineLimitNotice,
};
use iced::widget::{column, row, scrollable, Column, Space};
use iced::{Element, Length};

const MARKDOWN_MODE_SWITCH_RESERVED_HEIGHT: f32 = 40.0;
const MARKDOWN_MIN_BODY_SCROLL_HEIGHT: f32 = 120.0;
const TEXT_PREVIEW_LIMIT_NOTICE_RESERVED_HEIGHT: f32 = 30.0;
const TEXT_PREVIEW_MIN_BODY_SCROLL_HEIGHT: f32 = 120.0;
const TEXT_PREVIEW_SCROLLBAR_WIDTH: f32 = 6.0;
pub fn text_preview_panel<'a, Message>(
    rendered: &'a str,
    format: TextPreviewFormat,
    line_limit_notice: Option<TextPreviewLineLimitNotice>,
    document: Option<&'a TextPreviewDocument>,
    scroll_height: f32,
    text_preview_content_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
    plain_viewer: impl Fn(&'a TextPreviewDocument, f32) -> Element<'a, Message> + 'a,
    text_scroll: ScrollRegionWiring<'a, Message>,
    markdown_scroll: ScrollRegionWiring<'static, Message>,
    markdown_mode_switch: impl Fn(MarkdownPreviewMode) -> Element<'static, Message>,
) -> Column<'a, Message>
where
    Message: 'static + From<PreviewMessage>,
{
    let line_limit_notice = document
        .and_then(TextPreviewDocument::line_limit_notice)
        .or(line_limit_notice);
    let chunk_error = document.and_then(TextPreviewDocument::chunk_error);
    let external_notice_is_visible =
        text_preview_external_notice_is_visible(format, document, line_limit_notice);
    let footer_count = usize::from(external_notice_is_visible) + usize::from(chunk_error.is_some());
    let body_height = (scroll_height
        - TEXT_PREVIEW_LIMIT_NOTICE_RESERVED_HEIGHT * footer_count as f32)
        .max(TEXT_PREVIEW_MIN_BODY_SCROLL_HEIGHT);
    let body: Element<'_, Message> = match format {
        TextPreviewFormat::Plain => plain_text_preview_body(
            document,
            body_height,
            text_preview_content_height,
            scrollbar_visibility,
            scrollbar_viewport,
            plain_viewer,
            text_scroll,
        ),
        TextPreviewFormat::Markdown => markdown_text_preview_body(
            rendered,
            document,
            line_limit_notice,
            body_height,
            text_preview_content_height,
            scrollbar_visibility,
            scrollbar_viewport,
            markdown_scrollbar_visibility,
            markdown_scrollbar_viewport,
            plain_viewer,
            text_scroll,
            markdown_scroll,
            markdown_mode_switch,
        ),
    };

    let mut panel = column![body].spacing(8);
    if external_notice_is_visible {
        let Some(notice) = line_limit_notice else {
            return panel;
        };
        panel = panel.push(text_preview_line_limit_notice(notice));
    }
    if let Some(error) = chunk_error {
        panel = panel.push(text_preview_chunk_error(error));
    }
    panel
}

fn plain_text_preview_body<'a, Message>(
    document: Option<&'a TextPreviewDocument>,
    scroll_height: f32,
    content_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    plain_viewer: impl Fn(&'a TextPreviewDocument, f32) -> Element<'a, Message> + 'a,
    scroll: ScrollRegionWiring<'a, Message>,
) -> Element<'a, Message>
where
    Message: 'static + From<PreviewMessage>,
{
    let Some(document) = document else {
        return readable_text("Text preview is not ready").size(14).into();
    };
    let viewer = plain_viewer(document, scroll_height);
    let ScrollRegionWiring {
        smooth_scroll_wrap,
        scrollable_id,
        on_scroll,
    } = scroll;
    let geometry = scrollable(smooth_scroll_wrap(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(content_height.max(0.0)))
            .into(),
    ))
    .id(scrollable_id)
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        TEXT_PREVIEW_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .height(Length::Fixed(scroll_height))
    .width(Length::Fixed(SCROLLBAR_HOVER_WIDTH))
    .on_scroll(on_scroll);
    let base = row![viewer, geometry]
        .width(Length::Fill)
        .height(Length::Fixed(scroll_height));
    enhanced_scrollbar(
        base,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        TEXT_PREVIEW_SCROLLBAR_WIDTH,
    )
}

fn text_preview_line_limit_notice<Message: 'static>(
    notice: TextPreviewLineLimitNotice,
) -> Element<'static, Message> {
    localized_text(notice.label()).size(12).into()
}

fn text_preview_chunk_error<Message: 'static>(error: &str) -> Element<'static, Message> {
    localized_text(format!("Could not load more text preview: {error}"))
        .size(12)
        .into()
}

fn text_preview_external_notice_is_visible(
    format: TextPreviewFormat,
    document: Option<&TextPreviewDocument>,
    notice: Option<TextPreviewLineLimitNotice>,
) -> bool {
    notice.is_some_and(|_| {
        document
            .filter(|document| {
                format == TextPreviewFormat::Plain
                    || document.markdown_preview_mode() == MarkdownPreviewMode::Raw
            })
            .map(TextPreviewDocument::is_scrolled_to_preview_end)
            .unwrap_or(false)
    })
}
fn markdown_text_preview_body<'a, Message>(
    rendered: &'a str,
    document: Option<&'a TextPreviewDocument>,
    line_limit_notice: Option<TextPreviewLineLimitNotice>,
    scroll_height: f32,
    text_preview_content_height: f32,
    text_scrollbar_visibility: ScrollbarVisibility,
    text_scrollbar_viewport: Option<ScrollbarViewport>,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
    plain_viewer: impl Fn(&'a TextPreviewDocument, f32) -> Element<'a, Message> + 'a,
    text_scroll: ScrollRegionWiring<'a, Message>,
    markdown_scroll: ScrollRegionWiring<'static, Message>,
    markdown_mode_switch: impl Fn(MarkdownPreviewMode) -> Element<'static, Message>,
) -> Element<'a, Message>
where
    Message: 'static + From<PreviewMessage>,
{
    let Some(document) = document else {
        return readable_text("Text preview is not ready").size(14).into();
    };
    let mode = document.markdown_preview_mode();
    let body_height =
        (scroll_height - MARKDOWN_MODE_SWITCH_RESERVED_HEIGHT).max(MARKDOWN_MIN_BODY_SCROLL_HEIGHT);
    let body = match mode {
        MarkdownPreviewMode::Rendered => markdown_preview_body(
            rendered,
            line_limit_notice,
            body_height,
            markdown_scrollbar_visibility,
            markdown_scrollbar_viewport,
            markdown_scroll,
        ),
        MarkdownPreviewMode::Raw => plain_text_preview_body(
            Some(document),
            body_height,
            text_preview_content_height,
            text_scrollbar_visibility,
            text_scrollbar_viewport,
            plain_viewer,
            text_scroll,
        ),
    };

    column![markdown_mode_switch(mode), body].spacing(8).into()
}
