//! 目录/归档预览树面板：缩进条目行、目录展开切换、加载/错误状态行。
//! 自 app-ui view/preview_panel.rs 下沉；滚动接线（smooth-scroll 包装/
//! scrollable id/视口回传）经 ScrollRegionWiring 由宿主注入。

use iced::widget::{column, container, mouse_area, row, scrollable, Column, Space};
use iced::{Alignment, Element, Length};

use bennu_theme::icons::{
    icon_tone_style, preview_entry_icon_symbol, rotated_chevron_right_view, themed_icon, IconTone,
};
use bennu_theme::measured_text::format_middle_ellipsized_text;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis, ScrollbarViewport};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, ScrollbarVisibility,
};

use crate::panel_text::{localized_text, readable_text};
use crate::preview::{PreviewTreeDirectoryChildren, PreviewTreeEntry};
use crate::preview_message::PreviewMessage;
use crate::scroll_wiring::ScrollRegionWiring;

const PREVIEW_ICON_SIZE: f32 = 16.0;
const PREVIEW_ENTRY_NAME_MAX_CHARS: usize = 48;
const PREVIEW_TREE_INDENT_WIDTH: f32 = 18.0;
const PREVIEW_TREE_TOGGLE_WIDTH: f32 = 16.0;
const PREVIEW_TREE_TOGGLE_ROTATION_DEGREES: f32 = 90.0;
const TREE_SCROLLBAR_WIDTH: f32 = 6.0;

pub(crate) fn directory_preview_panel<Message>(
    entries: &[PreviewTreeEntry],
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let listing = preview_tree_listing(entries, "Empty directory");
    tree_scroller(
        listing,
        scroll_height,
        scrollbar_visibility,
        scrollbar_viewport,
        scroll,
    )
}

pub(crate) fn archive_preview_panel<Message>(
    entries: &[PreviewTreeEntry],
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let listing = preview_tree_listing(entries, "Empty archive");
    tree_scroller(
        listing,
        scroll_height,
        scrollbar_visibility,
        scrollbar_viewport,
        scroll,
    )
}

/// 目录/归档树共用滚动容器：固定高度 scrollable + enhanced_scrollbar
/// 浮层拇指；滚轮惯性包装与视口回传闭包来自宿主持有的滚动管线。
fn tree_scroller<Message>(
    listing: Column<'static, Message>,
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    scroll: ScrollRegionWiring<'static, Message>,
) -> Column<'static, Message>
where
    Message: 'static + Clone,
{
    let ScrollRegionWiring {
        smooth_scroll_wrap,
        scrollable_id,
        on_scroll,
    } = scroll;
    let scroller = scrollable(smooth_scroll_wrap(listing.into()))
        .id(scrollable_id)
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            TREE_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(scroll_height))
        .on_scroll(on_scroll);

    column![enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        TREE_SCROLLBAR_WIDTH,
    )]
}

fn preview_tree_listing<Message>(
    entries: &[PreviewTreeEntry],
    empty_message: &'static str,
) -> Column<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let mut listing = Column::new().spacing(3);
    if entries.is_empty() {
        return listing.push(readable_text(empty_message).size(14));
    }

    for entry in visible_preview_tree_entries(entries) {
        listing = listing.push(preview_tree_entry_row(entry));
        if let Some(message) = preview_tree_directory_status_message(entry) {
            listing = listing.push(preview_tree_status_row(entry, message));
        }
    }

    listing
}

fn preview_tree_directory_status_message(entry: &PreviewTreeEntry) -> Option<String> {
    if !entry.is_expanded {
        return None;
    }

    match entry.directory_children.as_ref()? {
        PreviewTreeDirectoryChildren::Error(error) => Some(format!("Could not load: {error}")),
        PreviewTreeDirectoryChildren::Loading
        | PreviewTreeDirectoryChildren::Pending
        | PreviewTreeDirectoryChildren::Loaded => None,
    }
}

fn visible_preview_tree_entries(entries: &[PreviewTreeEntry]) -> Vec<&PreviewTreeEntry> {
    entries
        .iter()
        .filter(|entry| preview_tree_entry_visible(entry, entries))
        .collect()
}

fn preview_tree_entry_visible(entry: &PreviewTreeEntry, entries: &[PreviewTreeEntry]) -> bool {
    let mut parent = entry.parent;
    while let Some(parent_id) = parent {
        let Some(parent_entry) = entries.get(parent_id) else {
            return false;
        };
        if !(parent_entry.is_expanded || parent_entry.toggle_rotation_progress > 0.0) {
            return false;
        }
        parent = parent_entry.parent;
    }

    true
}

fn preview_tree_entry_row<Message>(entry: &PreviewTreeEntry) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let name = format_middle_ellipsized_text(&entry.name, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        entry.depth as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let toggle: Element<'static, Message> = if entry.is_directory() {
        container(
            rotated_chevron_right_view(
                entry.toggle_rotation_progress * PREVIEW_TREE_TOGGLE_ROTATION_DEGREES,
                PREVIEW_TREE_TOGGLE_WIDTH,
            )
            .style(icon_tone_style(IconTone::Normal)),
        )
        .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .height(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_x(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_y(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .into()
    } else {
        Space::new()
            .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
            .into()
    };
    let row_content = row![
        indent,
        toggle,
        themed_icon(
            preview_entry_icon_symbol(entry.kind, &entry.name),
            IconTone::Normal,
            PREVIEW_ICON_SIZE,
        ),
        readable_text(name).size(14).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let row_container = container(row_content).padding([3, 6]).width(Length::Fill);

    if entry.is_directory() {
        mouse_area(row_container)
            .on_press(Message::from(PreviewMessage::PreviewTreeDirectoryToggled(
                entry.id,
            )))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    } else {
        row_container.into()
    }
}

fn preview_tree_status_row<Message>(
    entry: &PreviewTreeEntry,
    message: String,
) -> Element<'static, Message>
where
    Message: 'static,
{
    let message = format_middle_ellipsized_text(&message, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        (entry.depth + 1) as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let row_content = row![
        indent,
        Space::new().width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH)),
        Space::new().width(Length::Fixed(PREVIEW_ICON_SIZE)),
        localized_text(message).size(13).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(row_content)
        .padding([3, 6])
        .width(Length::Fill)
        .into()
}
