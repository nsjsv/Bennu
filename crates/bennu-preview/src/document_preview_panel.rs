//! 文档（PDF/Office 分页）预览面板：自 app-ui view/document_preview_panel.rs
//! 下沉。事件滚动接线经 ScrollRegionWiring 由宿主注入，其余逐字节保真。

use iced::widget::{container, image, scrollable, Column, Space};
use iced::{Alignment, ContentFit, Element, Length};

use bennu_theme::preview_styles::document_page_style;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis, ScrollbarViewport};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, ScrollbarVisibility,
};

use crate::document_preview::{document_viewport_height, DocumentPageView, PagedDocumentPreview};
use crate::panel_text::localized_text;
use crate::preview::PreviewSize;
use crate::scroll_wiring::ScrollRegionWiring;

const DOCUMENT_SCROLLBAR_WIDTH: f32 = 6.0;
const DOCUMENT_PAGE_GAP: f32 = 12.0;
const DOCUMENT_STATUS_TEXT_SIZE: u32 = 13;

pub fn document_preview_panel<Message>(
    document: &PagedDocumentPreview,
    preview_size: PreviewSize,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Element<'static, Message>
where
    Message: 'static,
{
    let wanted_pages = document.wanted_pages();
    let mut pages = Column::new().width(Length::Fill).align_x(Alignment::Center);
    let top_spacer = document.top_spacer_height();
    if top_spacer > 0.0 {
        pages = pages.push(Space::new().height(Length::Fixed(top_spacer)));
    }

    for (position, page_index) in wanted_pages.iter().copied().enumerate() {
        let Some(layout) = document.page_layout(page_index) else {
            continue;
        };
        let page: Element<'static, Message> = match document.page_view(page_index) {
            DocumentPageView::Ready(handle) => image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Contain)
                .into(),
            DocumentPageView::Error(error) => localized_text(error.to_owned())
                .size(DOCUMENT_STATUS_TEXT_SIZE)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .into(),
            DocumentPageView::Loading => localized_text("Rendering page...")
                .size(DOCUMENT_STATUS_TEXT_SIZE)
                .into(),
            DocumentPageView::Deferred => localized_text("Page deferred by preview memory limit")
                .size(DOCUMENT_STATUS_TEXT_SIZE)
                .into(),
        };
        let page = container(page)
            .width(Length::Fixed(document.page_width()))
            .height(Length::Fixed(layout.height))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .clip(true)
            .style(document_page_style);
        pages = pages.push(page);
        if position + 1 < wanted_pages.len() {
            pages = pages.push(Space::new().height(Length::Fixed(DOCUMENT_PAGE_GAP)));
        }
    }

    let bottom_spacer = document.bottom_spacer_height();
    if bottom_spacer > 0.0 {
        pages = pages.push(Space::new().height(Length::Fixed(bottom_spacer)));
    }

    let ScrollRegionWiring {
        smooth_scroll_wrap,
        scrollable_id,
        on_scroll,
    } = scroll;
    let scroller = scrollable(smooth_scroll_wrap(pages.into()))
        .id(scrollable_id)
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            DOCUMENT_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .width(Length::Fill)
        .height(Length::Fixed(document_viewport_height(preview_size.height)))
        .on_scroll(on_scroll);
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        DOCUMENT_SCROLLBAR_WIDTH,
    );
    scroller
}
