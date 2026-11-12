//! 文档（PDF/Office 分页）预览面板：自 app-ui view/document_preview_panel.rs
//! 下沉。事件滚动接线经 ScrollRegionWiring 由宿主注入，其余逐字节保真。

use iced::widget::{container, image, Column, Space};
use iced::{Alignment, ContentFit, Element, Length};

use bennu_theme::preview_styles::document_page_style;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};

use crate::document_preview::{document_viewport_height, DocumentPageView, PagedDocumentPreview};
use crate::panel_text::readable_text;
use crate::preview::PreviewSize;
use crate::scroll_wiring::ScrollRegionWiring;
use bennu_localization::tr;

const DOCUMENT_PAGE_GAP: f32 = 12.0;

pub fn document_preview_panel<Message>(
    document: &PagedDocumentPreview,
    preview_size: PreviewSize,
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
            DocumentPageView::Error(error) => readable_text(error.to_owned())
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .into(),
            DocumentPageView::Loading => readable_text(tr!("Rendering page...")).into(),
            DocumentPageView::Deferred => {
                readable_text(tr!("Page deferred by preview memory limit")).into()
            }
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
    let scroller = scroll
        .scrollable(pages.into())
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .width(Length::Fill)
        .height(Length::Fixed(document_viewport_height(preview_size.height)));
    let scroller = enhanced_scrollbar(scroller, ScrollbarAxis::Vertical);
    scroller
}
