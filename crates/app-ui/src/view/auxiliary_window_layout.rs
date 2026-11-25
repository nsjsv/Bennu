use iced::widget::{button, container, scrollable, Button, Column};
use iced::{Element, Length};

use crate::app::smooth_scroll::{smooth_scroll, smooth_scroll_id};
use crate::appearance::{enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction};
use crate::model::{Message, ScrollbarRegion};
use bennu_preview::panel_text::readable_text;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::sidebar_item_button_style;

/// 设置窗口的侧栏宽度（主窗口侧栏可拖宽，另算）。
pub(super) const AUXILIARY_SIDEBAR_WIDTH: f32 = 196.0;

/// 侧栏顶栏下面的分类列表。
pub(super) fn auxiliary_sidebar_list<'a>(items: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    Column::with_children(items)
        .spacing(12)
        .padding([9, 18])
        .width(Length::Fill)
        .into()
}

pub(super) fn auxiliary_sidebar_button(
    label: String,
    selected: bool,
    message: Message,
) -> Button<'static, Message> {
    button(readable_text(label))
        .on_press(message)
        .padding([7, 10])
        .width(Length::Fill)
        .style(move |theme, status| sidebar_item_button_style(theme, status, selected))
}

pub(super) fn auxiliary_detail_scroller<'a>(
    content: Column<'a, Message>,
    scroll_region: ScrollbarRegion,
) -> Element<'a, Message> {
    let content = container(content).padding(18).width(Length::Fill);
    let smooth = smooth_scroll(&scroll_region);
    let scroller = scrollable(smooth.capture(content))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style);
    let scroller = smooth.drive(enhanced_scrollbar(scroller, ScrollbarAxis::Vertical));

    container(scroller)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
