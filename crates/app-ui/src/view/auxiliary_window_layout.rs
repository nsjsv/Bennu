use iced::widget::{button, container, scrollable, Button, Column};
use iced::{Element, Length};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, selected_sidebar_item_style,
    transparent_button_style,
};
use crate::model::{Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility};
use bennu_preview::panel_text::readable_text;

/// 设置、属性窗口的侧栏宽度（主窗口侧栏可拖宽，另算）。
pub(super) const AUXILIARY_SIDEBAR_WIDTH: f32 = 196.0;

/// 侧栏顶栏下面的分类列表。
pub(super) fn auxiliary_sidebar_list<'a>(items: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    Column::with_children(items)
        .spacing(2)
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}

pub(super) fn auxiliary_sidebar_button(
    label: String,
    selected: bool,
    message: Message,
) -> Button<'static, Message> {
    let label = container(readable_text(label))
        .padding([7, 10])
        .width(Length::Fill);
    let label = if selected {
        label.style(selected_sidebar_item_style)
    } else {
        label
    };

    button(label)
        .on_press(message)
        .width(Length::Fill)
        .style(transparent_button_style())
}

pub(super) fn auxiliary_detail_scroller<'a>(
    content: Column<'a, Message>,
    scroll_region: ScrollbarRegion,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    scroll_message: Message,
) -> Element<'a, Message> {
    let content = container(content).padding(18).width(Length::Fill);
    let scroller = scrollable(smooth_scroll_content(content, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), move |_| {
            scroll_message.clone()
        }));
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    );

    container(scroller)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
