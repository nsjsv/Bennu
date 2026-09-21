use iced::widget::{button, container, row, scrollable, Button, Column, Space};
use iced::{Element, Length};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{
    app_content_style, enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction,
    selected_sidebar_item_style, sidebar_style, transparent_button_style,
};
use crate::model::{Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility};
use crate::typography::readable_text;

use super::sidebar_panel::sidebar_floating_panel_margin;

const AUXILIARY_SIDEBAR_WIDTH: f32 = 196.0;

pub(super) fn auxiliary_split_window<'a>(
    sidebar: Element<'a, Message>,
    detail: Element<'a, Message>,
) -> Element<'a, Message> {
    container(row![sidebar, detail].height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}
pub(super) fn auxiliary_detail_surface_with_sidebar_space<'a>(
    detail: Element<'a, Message>,
) -> Element<'a, Message> {
    container(
        row![
            Space::new().width(Length::Fixed(AUXILIARY_SIDEBAR_WIDTH)),
            detail,
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(app_content_style)
    .into()
}

pub(super) fn auxiliary_sidebar<'a>(content: Column<'a, Message>) -> Element<'a, Message> {
    let sidebar = container(content.padding(14))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(sidebar_style);

    container(sidebar)
        .padding(sidebar_floating_panel_margin())
        .width(Length::Fixed(AUXILIARY_SIDEBAR_WIDTH))
        .height(Length::Fill)
        .into()
}

pub(super) fn auxiliary_full_height_sidebar<'a>(
    content: Column<'a, Message>,
) -> Element<'a, Message> {
    let sidebar = container(content.padding(14))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(sidebar_style);
    container(sidebar)
        .padding(sidebar_floating_panel_margin())
        .width(Length::Fixed(AUXILIARY_SIDEBAR_WIDTH))
        .height(Length::Fill)
        .into()
}

pub(super) fn auxiliary_sidebar_button(
    label: &'static str,
    selected: bool,
    message: Message,
) -> Button<'static, Message> {
    let label = container(readable_text(label).size(13))
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
