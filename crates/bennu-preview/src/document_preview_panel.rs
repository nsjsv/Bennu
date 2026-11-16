//! 文档（PDF/Office 分页）预览面板：页按缩放排版，放大后能横向滚；底部浮条是缩放和页码
//! （页码框里输数字回车跳页）；顶上是搜索条（Ctrl+F），页上画匹配和拖选的文字。
//! 事件滚动接线经 ScrollRegionWiring 由宿主注入。

use iced::widget::{button, container, image, row, text, tooltip, Column, Space, Stack};
use iced::{Alignment, ContentFit, Element, Length};

use bennu_theme::controls::{icon_button, text_field, toggle_button_style, tooltip_bubble};
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::preview_styles::document_page_style;
use bennu_theme::scrollbar::enhanced_scrollbar_both;
use bennu_theme::styles::{enhanced_both_scrollbar_direction, enhanced_scrollbar_style};
use bennu_theme::typography::CAPTION_SIZE;

use crate::document_page_marks::page_marks;
use crate::document_preview::{
    document_viewport_height, DocumentPageView, DocumentPreviewMessage, DocumentSearch,
    DocumentTextLayer, DocumentTextMessage, DocumentZoom, PagedDocumentPreview,
};
use crate::media_controls::{floating_bar, floating_bar_text, floating_icon_button};
use crate::media_keys::document_keys;
use crate::panel_text::readable_text;
use crate::pointer_area::pointer_area;
use crate::preview::PreviewSize;
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;
use crate::text_document::SearchDirection;
use crate::text_find_bar::{find_bar, find_input_id, summary_label, FindKeys};
use bennu_localization::tr;

const DOCUMENT_PAGE_GAP: f32 = 12.0;
const ZOOM_PERCENT_WIDTH: f32 = 52.0;
const PAGE_NUMBER_FIELD_WIDTH: f32 = 52.0;
const FIND_ICON_SIZE: f32 = 14.0;

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
        let width = document.page_width();
        let height = layout.height;
        let page = container(page)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .center_x(Length::Fixed(width))
            .center_y(Length::Fixed(height))
            .clip(true)
            .style(document_page_style);
        let marked = Stack::with_children([
            page.into(),
            page_marks(width, height, document.page_marks(page_index)),
        ]);
        // 拖选：指针位置换成页内比例；文字层好了光标变成文字光标。
        let text_message = |message| {
            Message::from(PreviewMessage::DocumentPreview(
                DocumentPreviewMessage::Text(message),
            ))
        };
        let interaction = if matches!(document.text_layer(), DocumentTextLayer::Ready(_)) {
            iced::mouse::Interaction::Text
        } else {
            iced::mouse::Interaction::Idle
        };
        // 按下的位置随按下一起带过去；逐次的移动只在拖选时要（选区可以拖过别的页，每页都挂）。
        let page = pointer_area(marked)
            .on_press(move |point| {
                text_message(DocumentTextMessage::Pressed {
                    page: page_index,
                    x: point.x / width,
                    y: point.y / height,
                })
            })
            .on_release(text_message(DocumentTextMessage::Released))
            .interaction(interaction);
        let page = if document.is_selecting() {
            page.on_move(move |point| {
                text_message(DocumentTextMessage::PointerMoved {
                    page: page_index,
                    x: point.x / width,
                    y: point.y / height,
                })
            })
        } else {
            page
        };
        pages = pages.push(page);
        if position + 1 < wanted_pages.len() {
            pages = pages.push(Space::new().height(Length::Fixed(DOCUMENT_PAGE_GAP)));
        }
    }

    let bottom_spacer = document.bottom_spacer_height();
    if bottom_spacer > 0.0 {
        pages = pages.push(Space::new().height(Length::Fixed(bottom_spacer)));
    }
    let scroller = scroll.scrollable(pages.into(), |scroller| {
        enhanced_scrollbar_both(
            scroller
                .direction(enhanced_both_scrollbar_direction())
                .style(enhanced_scrollbar_style)
                .width(Length::Fill)
                .height(Length::Fixed(document_viewport_height(preview_size.height))),
        )
    });
    let mut layers = Stack::with_children([scroller, page_controls(document)])
        .width(Length::Fill)
        .height(Length::Fill);
    if let Some(search) = document.search() {
        layers = layers.push(
            container(document_find_bar(document, search))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Top),
        );
    }
    document_keys(layers.into())
}

/// 和文本预览一样的搜索条：输入、区分大小写、「3/17」、上一处、下一处、关闭。
fn document_find_bar<Message>(
    document: &PagedDocumentPreview,
    search: &DocumentSearch,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let text_message = |message| {
        Message::from(PreviewMessage::DocumentPreview(
            DocumentPreviewMessage::Text(message),
        ))
    };
    let step = |direction| text_message(DocumentTextMessage::Stepped(direction));
    let searching = !search.matches.is_empty();
    let arrow = |symbol, direction| {
        icon_button(themed_icon(symbol, IconTone::Normal, FIND_ICON_SIZE))
            .on_press_maybe(searching.then(|| step(direction)))
    };
    let status = match document.text_layer() {
        DocumentTextLayer::NotLoaded | DocumentTextLayer::Loading => {
            tr!("Reading the document's text…")
        }
        DocumentTextLayer::Failed(error) => error.clone(),
        DocumentTextLayer::Ready(_) => summary_label(document.search_summary()),
    };
    let content = row![
        text_field(&tr!("Search"), &search.query)
            .id(find_input_id())
            .size(CAPTION_SIZE)
            .on_input(move |query| text_message(DocumentTextMessage::QueryChanged(query)))
            .width(Length::Fill),
        tooltip(
            button(text("Aa").size(CAPTION_SIZE))
                .padding([3, 8])
                .style(toggle_button_style(search.case_sensitive))
                .on_press(text_message(DocumentTextMessage::CaseToggled)),
            tooltip_bubble(tr!("Match case")),
            tooltip::Position::Bottom,
        ),
        text(status).size(CAPTION_SIZE),
        arrow(IconSymbol::ArrowUp, SearchDirection::Backward),
        arrow(IconSymbol::ArrowDown, SearchDirection::Forward),
        icon_button(themed_icon(
            IconSymbol::Close,
            IconTone::Normal,
            FIND_ICON_SIZE
        ))
        .on_press(text_message(DocumentTextMessage::SearchClosed)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let close = text_message(DocumentTextMessage::SearchClosed);
    let open = text_message(DocumentTextMessage::SearchOpened);
    find_bar(
        content.into(),
        FindKeys {
            escape: close,
            enter: step(SearchDirection::Forward),
            shift_enter: step(SearchDirection::Backward),
            next: step(SearchDirection::Forward),
            previous: step(SearchDirection::Backward),
            open_search: open.clone(),
            // 文档没有跳行，Ctrl+G 也回到搜索框。
            open_goto: open,
        },
    )
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
