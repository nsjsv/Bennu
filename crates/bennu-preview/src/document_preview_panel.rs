//! 文档（PDF/Office 分页）预览面板：页按缩放排版，放大后能横向滚；底部浮条是缩放和页码
//! （页码框里输数字回车跳页）。事件滚动接线经 ScrollRegionWiring 由宿主注入。

use iced::widget::{container, image, row, Column, Space, Stack};
use iced::{Alignment, ContentFit, Element, Length};

use bennu_theme::controls::text_field;
use bennu_theme::icons::IconSymbol;
use bennu_theme::preview_styles::document_page_style;
use bennu_theme::scrollbar::enhanced_scrollbar_both;
use bennu_theme::styles::{enhanced_both_scrollbar_direction, enhanced_scrollbar_style};

use crate::document_preview::{
    document_viewport_height, DocumentPageView, DocumentPreviewMessage, DocumentZoom,
    PagedDocumentPreview,
};
use crate::media_controls::{floating_bar, floating_bar_text, floating_icon_button};
use crate::media_keys::document_keys;
use crate::panel_text::readable_text;
use crate::preview::PreviewSize;
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use bennu_localization::tr;

const DOCUMENT_PAGE_GAP: f32 = 12.0;
const ZOOM_PERCENT_WIDTH: f32 = 52.0;
const PAGE_NUMBER_FIELD_WIDTH: f32 = 52.0;

pub fn document_preview_panel<Message>(
    document: &PagedDocumentPreview,
    preview_size: PreviewSize,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let wanted_pages = document.wanted_pages();
    let mut pages = Column::new()
        .width(Length::Fixed(document.content_width()))
        .align_x(Alignment::Center);
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
            .center_x(Length::Fixed(document.page_width()))
            .center_y(Length::Fixed(layout.height))
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
        .direction(enhanced_both_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .width(Length::Fill)
        .height(Length::Fixed(document_viewport_height(preview_size.height)));
    let layers = Stack::with_children([enhanced_scrollbar_both(scroller), page_controls(document)])
        .width(Length::Fill)
        .height(Length::Fill);
    document_keys(layers.into())
}

/// 缩小、比例（相对铺满宽度）、放大、回到铺满宽度，页码框 / 总页数。
fn page_controls<Message>(document: &PagedDocumentPreview) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let message = |message| Message::from(PreviewMessage::DocumentPreview(message));
    let page_number = document
        .page_number_draft()
        .map_or_else(|| (document.current_page() + 1).to_string(), str::to_owned);
    let mut controls = row![
        floating_icon_button(
            IconSymbol::ZoomOut,
            message(DocumentPreviewMessage::Zoomed(DocumentZoom::Out)),
            1.0
        ),
        floating_bar_text(
            format!("{:.0}%", document.zoom() * 100.0),
            ZOOM_PERCENT_WIDTH,
            1.0
        ),
        floating_icon_button(
            IconSymbol::ZoomIn,
            message(DocumentPreviewMessage::Zoomed(DocumentZoom::In)),
            1.0
        ),
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    if (document.zoom() - 1.0).abs() > f32::EPSILON {
        controls = controls.push(floating_icon_button(
            IconSymbol::Maximize,
            message(DocumentPreviewMessage::Zoomed(DocumentZoom::FitWidth)),
            1.0,
        ));
    }
    controls = controls
        .push(Space::new().width(Length::Fixed(8.0)))
        .push(
            text_field("", &page_number)
                .on_input(move |draft| message(DocumentPreviewMessage::PageNumberEdited(draft)))
                .on_submit(message(DocumentPreviewMessage::PageNumberSubmitted))
                .width(Length::Fixed(PAGE_NUMBER_FIELD_WIDTH))
                .align_x(iced::alignment::Horizontal::Center),
        )
        .push(readable_text(format!("/ {}", document.page_count())))
        .push(Space::new().width(Length::Fixed(4.0)));
    floating_bar(controls.into(), 1.0)
}
