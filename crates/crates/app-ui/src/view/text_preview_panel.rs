use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};
use crate::model::{
    MarkdownPreviewMode, Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility,
    TextPreviewDocument, TextPreviewFormat, TextPreviewLineLimitNotice, SCROLLBAR_HOVER_WIDTH,
};
use crate::text_preview_viewer::text_preview_viewer;
use crate::typography::{localized_text, readable_text};
use iced::widget::{column, row, scrollable, Column, Space};
use iced::{Element, Length};

use super::markdown_preview::markdown_preview_body;
use super::option_controls::{segmented_choice_row, SegmentedChoice};

const MARKDOWN_MODE_SWITCH_RESERVED_HEIGHT: f32 = 40.0;
const MARKDOWN_MIN_BODY_SCROLL_HEIGHT: f32 = 120.0;
const TEXT_PREVIEW_LIMIT_NOTICE_RESERVED_HEIGHT: f32 = 30.0;
const TEXT_PREVIEW_MIN_BODY_SCROLL_HEIGHT: f32 = 120.0;
const TEXT_PREVIEW_SCROLLBAR_WIDTH: f32 = 6.0;
pub(super) fn text_preview_panel<'a>(
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
) -> Column<'a, Message> {
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

fn plain_text_preview_body<'a>(
    document: Option<&'a TextPreviewDocument>,
    scroll_height: f32,
    content_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    let Some(document) = document else {
        return readable_text("Text preview is not ready").size(14).into();
    };
    let scroll_region = ScrollbarRegion::TextPreview;
    let wheel_region = scroll_region.clone();
    let viewer = text_preview_viewer(
        document,
        scroll_height,
        // 滚动几何宿主是输入源：只镜像文档副本预取分块，不回推宿主。
        |lines, _offset_y, viewport_height| Message::TextPreviewContentScrolled {
            lines,
            viewport_height,
        },
        move |delta| Message::SmoothScrollWheel(wheel_region.clone(), delta),
        Message::TextPreviewContentHeightChanged,
        // 查看器内部滚动（键盘/光标跟随）才回推宿主滚动位置。
        |lines, offset_y, viewport_height| Message::TextPreviewViewerScrolled {
            lines,
            offset_y,
            viewport_height,
        },
    );
    let geometry = scrollable(smooth_scroll_content(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(content_height.max(0.0))),
        scroll_region.clone(),
    ))
    .id(smooth_scroll_id(&scroll_region))
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        TEXT_PREVIEW_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .height(Length::Fixed(scroll_height))
    .width(Length::Fixed(SCROLLBAR_HOVER_WIDTH))
    .on_scroll(scrollbar_on_scroll(scroll_region, |viewport| {
        Message::TextPreviewViewportSynced {
            offset_y: viewport.absolute_offset().y,
            viewport_height: viewport.bounds().height,
        }
    }));
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

fn text_preview_line_limit_notice(notice: TextPreviewLineLimitNotice) -> Element<'static, Message> {
    localized_text(notice.label()).size(12).into()
}

fn text_preview_chunk_error(error: &str) -> Element<'static, Message> {
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
fn markdown_text_preview_body<'a>(
    rendered: &'a str,
    document: Option<&'a TextPreviewDocument>,
    line_limit_notice: Option<TextPreviewLineLimitNotice>,
    scroll_height: f32,
    text_preview_content_height: f32,
    text_scrollbar_visibility: ScrollbarVisibility,
    text_scrollbar_viewport: Option<ScrollbarViewport>,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
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
        ),
        MarkdownPreviewMode::Raw => plain_text_preview_body(
            Some(document),
            body_height,
            text_preview_content_height,
            text_scrollbar_visibility,
            text_scrollbar_viewport,
        ),
    };

    column![markdown_preview_mode_switch(mode), body]
        .spacing(8)
        .into()
}

fn markdown_preview_mode_switch(mode: MarkdownPreviewMode) -> Element<'static, Message> {
    segmented_choice_row(vec![
        markdown_preview_mode_choice("Rendered", MarkdownPreviewMode::Rendered, mode),
        markdown_preview_mode_choice("Raw", MarkdownPreviewMode::Raw, mode),
    ])
}

fn markdown_preview_mode_choice(
    label: &'static str,
    mode: MarkdownPreviewMode,
    selected_mode: MarkdownPreviewMode,
) -> SegmentedChoice {
    SegmentedChoice {
        label,
        selected: mode == selected_mode,
        message: Message::MarkdownPreviewModeSelected(mode),
        tooltip: None,
    }
}
