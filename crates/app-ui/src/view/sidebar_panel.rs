use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, Column, Row, Space,
};
use iced::{Alignment, Element, Length, Padding};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::FileBrowser;
use crate::appearance::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, hovered_sidebar_item_style,
    navigation_icon_button_style, selected_sidebar_item_style, sidebar_bookmark_drop_slot_style,
    sidebar_style,
};
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::formatting::format_file_size;
use crate::icons::IconSymbol;
use crate::measured_middle_ellipsized_text::measured_middle_ellipsized_text;
use crate::model::{
    trash_location_path, Message, ScrollbarRegion, SidebarBookmarkDropSlot, SidebarLocation,
    TRASH_LOCATION_LABEL,
};
use crate::network_connections::{NetworkConnectionMessage, SidebarNetworkConnectionEntry};
use crate::sidebar::sidebar_icon_symbol;
use crate::sidebar_devices::SidebarDeviceEntry;
use crate::typography::readable_text;

use super::{tab_motion, themed_icon, IconTone, MENU_ICON_SIZE};

// 设备/网络行图标符号来自共享 crate，与 portal 侧栏保持一致。
use bennu_sidebar::{SIDEBAR_DEVICE_ICON_SYMBOL, SIDEBAR_NETWORK_CONNECTION_ICON_SYMBOL};

const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;
// 卡片悬浮感靠留白 + 圆角 + 投影:窗口侧三边(上/左/下)等宽 7
// (离窗沿近,与 portal 选择窗口一致),右侧 4 是与内容区的间隙。卡片
// 仍盖在终端抽屉左段上,分隔线从卡片后穿过。
const SIDEBAR_FLOATING_MARGIN_LEFT: f32 = 7.0;
const SIDEBAR_FLOATING_MARGIN_RIGHT: f32 = 4.0;
const SIDEBAR_FLOATING_MARGIN_VERTICAL: f32 = 7.0;
const SIDEBAR_BOOKMARK_DROP_SLOT_HEIGHT: f32 = 3.0;

pub(crate) fn sidebar_view(browser: &FileBrowser) -> Element<'_, Message> {
    let sidebar_header = row![
        readable_text("Places").size(16).width(Length::Fill),
        button(themed_icon(
            IconSymbol::Settings,
            IconTone::Normal,
            MENU_ICON_SIZE
        ))
        .on_press(Message::SettingsOpened)
        .padding([4, 6])
        .style(navigation_icon_button_style()),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut sidebar = column![sidebar_header].spacing(6).padding(12);
    for location in browser
        .sidebar_locations
        .iter()
        .filter(|location| !location.kind.is_user_favorite())
    {
        sidebar = sidebar.push(sidebar_location_item(browser, location));
    }
    sidebar = sidebar.push(sidebar_trash_item(browser));

    let favorite_locations = browser
        .sidebar_locations
        .iter()
        .filter(|location| location.kind.is_user_favorite())
        .collect::<Vec<_>>();
    let can_drop_bookmark = browser.can_drop_sidebar_bookmark();
    if can_drop_bookmark || !favorite_locations.is_empty() {
        let favorites_label = sidebar_section_label("Favorites");
        sidebar = sidebar.push(if can_drop_bookmark && favorite_locations.is_empty() {
            track_file_drag_hit_test_marker(
                favorites_label,
                FileDragHitTestMarker::EmptySidebarBookmarks,
            )
        } else {
            favorites_label
        });

        for (index, location) in favorite_locations.iter().enumerate() {
            sidebar = sidebar.push(sidebar_location_item_with_index(browser, location, index));
        }
    }

    sidebar = append_sidebar_network_connections(sidebar, browser);
    sidebar = append_sidebar_devices(sidebar, browser);

    let scrollbar_region = ScrollbarRegion::Sidebar;
    let scrollbar_visibility = browser.scrollbar_visibility_for(&scrollbar_region);
    let sidebar_scroller = scrollable(smooth_scroll_content(sidebar, scrollbar_region.clone()))
        .id(smooth_scroll_id(&scrollbar_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fill)
        .on_scroll(scrollbar_on_scroll(scrollbar_region.clone(), |_| {
            Message::SidebarScrolled
        }));
    let sidebar_scroller = enhanced_scrollbar(
        sidebar_scroller,
        scrollbar_visibility,
        browser.scrollbar_viewport_for(&scrollbar_region),
        ScrollbarAxis::Vertical,
        6.0,
    );
    let sidebar_content_panel = container(sidebar_scroller)
        .width(Length::Fill)
        .height(Length::Fill);

    let sidebar_content: Element<'_, Message> = if can_drop_bookmark {
        mouse_area(sidebar_content_panel)
            .on_move(Message::SidebarPointerMoved)
            .on_exit(Message::SidebarPointerExited)
            .into()
    } else {
        sidebar_content_panel.into()
    };

    let sidebar_panel = container(sidebar_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(sidebar_style);

    let sidebar_panel = container(sidebar_panel)
        .padding(sidebar_floating_panel_margin())
        .width(Length::Fill)
        .height(Length::Fill);

    container(
        row![sidebar_panel, sidebar_resize_handle()]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(browser.sidebar_width))
    .height(Length::Fill)
    .into()
}

pub(super) fn sidebar_floating_panel_margin() -> Padding {
    Padding {
        top: SIDEBAR_FLOATING_MARGIN_VERTICAL,
        right: SIDEBAR_FLOATING_MARGIN_RIGHT,
        bottom: SIDEBAR_FLOATING_MARGIN_VERTICAL,
        left: SIDEBAR_FLOATING_MARGIN_LEFT,
    }
}

fn append_sidebar_devices<'a>(
    mut sidebar: Column<'a, Message>,
    browser: &'a FileBrowser,
) -> Column<'a, Message> {
    if browser.sidebar_devices.devices.is_empty()
        && browser.sidebar_devices.provider_failures.is_empty()
        && !browser.sidebar_devices.is_loading
    {
        return sidebar;
    }

    sidebar = sidebar.push(sidebar_section_label("Devices"));
    for device in &browser.sidebar_devices.devices {
        sidebar = sidebar.push(sidebar_device_item(browser, device));
    }

    if let Some(message) =
        sidebar_device_provider_failure_message(&browser.sidebar_devices.provider_failures)
    {
        sidebar = sidebar.push(sidebar_message_row(message));
    } else if browser.sidebar_devices.devices.is_empty() && browser.sidebar_devices.is_loading {
        sidebar = sidebar.push(sidebar_message_row("Loading devices..."));
    }

    sidebar
}

fn append_sidebar_network_connections<'a>(
    mut sidebar: Column<'a, Message>,
    browser: &'a FileBrowser,
) -> Column<'a, Message> {
    sidebar = sidebar.push(sidebar_network_section_header());
    for connection in &browser.network_connections.entries {
        sidebar = sidebar.push(sidebar_network_connection_item(browser, connection));
    }

    if browser.network_connections.entries.is_empty() {
        sidebar = sidebar.push(sidebar_message_row("No saved connections"));
    } else if browser.network_connections.unavailable.is_some() {
        sidebar = sidebar.push(sidebar_message_row("Network status unavailable"));
    }

    sidebar
}

fn sidebar_network_section_header() -> Element<'static, Message> {
    row![
        container(readable_text("Network").size(12))
            .padding([4, 8])
            .width(Length::Fill),
        button(themed_icon(
            IconSymbol::Plus,
            IconTone::Normal,
            MENU_ICON_SIZE
        ))
        .on_press(Message::NetworkConnection(
            NetworkConnectionMessage::AddRequested
        ))
        .padding([2, 5])
        .style(navigation_icon_button_style()),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn sidebar_resize_handle() -> Element<'static, Message> {
    let handle = container(Space::new().width(Length::Fixed(SIDEBAR_RESIZE_HANDLE_WIDTH)))
        .width(Length::Fixed(SIDEBAR_RESIZE_HANDLE_WIDTH))
        .height(Length::Fill);

    mouse_area(handle)
        .on_press(Message::SidebarResizeStarted)
        .on_release(Message::DragSelectionFinished)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}

fn sidebar_location_item<'a>(
    browser: &'a FileBrowser,
    location: &'a SidebarLocation,
) -> Element<'a, Message> {
    sidebar_location_item_content(browser, location, None)
}

fn sidebar_location_item_with_index<'a>(
    browser: &'a FileBrowser,
    location: &'a SidebarLocation,
    favorite_index: usize,
) -> Element<'a, Message> {
    sidebar_location_item_content(browser, location, Some(favorite_index))
}

fn sidebar_location_item_content<'a>(
    browser: &'a FileBrowser,
    location: &'a SidebarLocation,
    favorite_index: Option<usize>,
) -> Element<'a, Message> {
    let presentation = sidebar_presentation(browser, location);
    let tone = if presentation.is_selected() {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let is_favorite = favorite_index.is_some();
    let label = if is_favorite {
        location.label.clone()
    } else {
        crate::localization::translate_current(&location.label)
    };

    let item_container = container(sidebar_label(sidebar_icon_symbol(location), &label, tone))
        .padding([6, 8])
        .width(Length::Fill);
    let item_container = match presentation {
        SidebarPresentation::Selected => item_container.style(selected_sidebar_item_style),
        SidebarPresentation::Hovered => item_container.style(hovered_sidebar_item_style),
        SidebarPresentation::Normal => item_container,
    };

    let item_container = if let Some(favorite_index) = favorite_index {
        sidebar_bookmark_drop_overlay(browser, item_container, favorite_index)
    } else {
        item_container.into()
    };
    let item_content: Element<'a, Message> = if is_favorite {
        tab_motion::translated(
            item_container,
            0.0,
            browser.sidebar_bookmark_motion_offset(location.path.as_path()),
        )
    } else {
        item_container.into()
    };

    let item = mouse_area(item_content)
        .on_enter(if is_favorite {
            Message::SidebarBookmarkEntered(location.path.clone())
        } else {
            Message::SidebarHovered(location.path.clone())
        })
        .on_exit(Message::SidebarHoverCleared(location.path.clone()))
        .on_middle_press(Message::OpenDirectoryFromMiddleClick(
            browser.active_pane_id(),
            location.path.clone(),
        ))
        .on_press(if is_favorite {
            Message::SidebarBookmarkPressed(location.path.clone())
        } else {
            Message::NavigateTo(location.path.clone())
        })
        .on_release(if is_favorite && browser.file_drag.is_none() {
            Message::SidebarBookmarkReleased
        } else {
            Message::DragSelectionFinished
        })
        .interaction(iced::mouse::Interaction::Pointer);
    let item = if is_favorite {
        item.on_right_press(Message::SidebarBookmarkRightClicked(location.path.clone()))
    } else {
        item
    };

    track_file_drag_hit_test_marker(
        item,
        FileDragHitTestMarker::SidebarDirectory {
            directory: location.path.clone(),
            favorite_index,
        },
    )
}

fn sidebar_bookmark_drop_overlay<'a>(
    browser: &FileBrowser,
    item_container: iced::widget::Container<'a, Message>,
    favorite_index: usize,
) -> Element<'a, Message> {
    let Some(slot) = browser.sidebar_bookmark_drop_slot else {
        return item_container.into();
    };

    let SidebarBookmarkDropSlot::Insert { index } = slot;
    let line_alignment = if index == 0 && favorite_index == 0 {
        SidebarBookmarkDropLineAlignment::Top
    } else if index > 0 && index == favorite_index + 1 {
        SidebarBookmarkDropLineAlignment::Bottom
    } else {
        return item_container.into();
    };

    let line = container(Space::new().height(Length::Fixed(SIDEBAR_BOOKMARK_DROP_SLOT_HEIGHT)))
        .height(Length::Fixed(SIDEBAR_BOOKMARK_DROP_SLOT_HEIGHT))
        .width(Length::Fill)
        .style(sidebar_bookmark_drop_slot_style);
    let aligned_line = match line_alignment {
        SidebarBookmarkDropLineAlignment::Top => container(line).align_top(Length::Fill),
        SidebarBookmarkDropLineAlignment::Bottom => container(line).align_bottom(Length::Fill),
    };

    stack([item_container.into(), aligned_line.into()])
        .width(Length::Fill)
        .into()
}

enum SidebarBookmarkDropLineAlignment {
    Top,
    Bottom,
}

fn sidebar_device_provider_failure_message(
    failures: &[desktop_linux::StorageDeviceProviderFailure],
) -> Option<&'static str> {
    match failures {
        [] => None,
        [failure] => Some(match failure.provider {
            desktop_linux::StorageDeviceProvider::Udisks => "UDisks devices unavailable",
            desktop_linux::StorageDeviceProvider::Gvfs => "GVfs devices unavailable",
        }),
        _ => Some("Some device providers unavailable"),
    }
}

fn sidebar_device_item<'a>(
    browser: &'a FileBrowser,
    device: &'a SidebarDeviceEntry,
) -> Element<'a, Message> {
    let presentation = sidebar_device_presentation(browser, device);
    let tone = if presentation.is_selected() {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let pending = browser.sidebar_devices.is_action_pending(&device.id);

    let item_container = container(sidebar_device_label(device, pending, tone))
        .padding([6, 8])
        .width(Length::Fill);
    let item_container = match presentation {
        SidebarPresentation::Selected => item_container.style(selected_sidebar_item_style),
        SidebarPresentation::Hovered => item_container.style(hovered_sidebar_item_style),
        SidebarPresentation::Normal => item_container,
    };

    mouse_area(item_container)
        .on_enter(Message::SidebarDeviceHovered(device.id.clone()))
        .on_exit(Message::SidebarDeviceHoverCleared(device.id.clone()))
        .on_middle_press(Message::SidebarDeviceMiddlePressed(
            browser.active_pane_id(),
            device.id.clone(),
        ))
        .on_press(Message::SidebarDevicePressed(device.id.clone()))
        .on_right_press(Message::SidebarDeviceRightClicked(device.id.clone()))
        .on_release(Message::DragSelectionFinished)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn sidebar_network_connection_item<'a>(
    browser: &'a FileBrowser,
    connection: &'a SidebarNetworkConnectionEntry,
) -> Element<'a, Message> {
    let presentation = sidebar_network_connection_presentation(browser, connection);
    let tone = if presentation.is_selected() {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let pending = browser.network_connections.is_pending(connection.id());

    let item_container = container(sidebar_network_connection_label(connection, pending, tone))
        .padding([6, 8])
        .width(Length::Fill);
    let item_container = match presentation {
        SidebarPresentation::Selected => item_container.style(selected_sidebar_item_style),
        SidebarPresentation::Hovered => item_container.style(hovered_sidebar_item_style),
        SidebarPresentation::Normal => item_container,
    };

    mouse_area(item_container)
        .on_enter(Message::NetworkConnection(
            NetworkConnectionMessage::Hovered(connection.id().clone()),
        ))
        .on_exit(Message::NetworkConnection(
            NetworkConnectionMessage::HoverCleared(connection.id().clone()),
        ))
        .on_middle_press(Message::NetworkConnection(
            NetworkConnectionMessage::MiddlePressed(
                browser.active_pane_id(),
                connection.id().clone(),
            ),
        ))
        .on_press(Message::NetworkConnection(
            NetworkConnectionMessage::Pressed(connection.id().clone()),
        ))
        .on_right_press(Message::NetworkConnection(
            NetworkConnectionMessage::RightClicked(connection.id().clone()),
        ))
        .on_release(Message::DragSelectionFinished)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn sidebar_trash_item(browser: &FileBrowser) -> Element<'_, Message> {
    let trash_path = trash_location_path();
    let trash_presentation = if browser.is_trash_view {
        SidebarPresentation::Selected
    } else if browser.hovered_sidebar.as_ref() == Some(&trash_path) {
        SidebarPresentation::Hovered
    } else {
        SidebarPresentation::Normal
    };
    let trash_tone = if trash_presentation.is_selected() {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let trash_label = crate::localization::translate_current(TRASH_LOCATION_LABEL);
    let trash_container = container(sidebar_label(IconSymbol::Trash, &trash_label, trash_tone))
        .padding([6, 8])
        .width(Length::Fill);
    let trash_container = match trash_presentation {
        SidebarPresentation::Selected => trash_container.style(selected_sidebar_item_style),
        SidebarPresentation::Hovered => trash_container.style(hovered_sidebar_item_style),
        SidebarPresentation::Normal => trash_container,
    };
    let trash_hover_path = trash_path.clone();
    mouse_area(trash_container)
        .on_enter(Message::SidebarHovered(trash_hover_path.clone()))
        .on_exit(Message::SidebarHoverCleared(trash_hover_path))
        .on_press(Message::TrashOpened)
        .on_middle_press(Message::OpenTrashInNewTab(browser.active_pane_id()))
        .on_release(Message::DragSelectionFinished)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn sidebar_section_label(label: &'static str) -> Element<'static, Message> {
    container(readable_text(label).size(12))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}

fn sidebar_message_row(message: &'static str) -> Element<'static, Message> {
    container(readable_text(message).size(12))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}

fn sidebar_presentation(browser: &FileBrowser, location: &SidebarLocation) -> SidebarPresentation {
    if !browser.is_trash_view && location.path == browser.current_dir {
        SidebarPresentation::Selected
    } else if browser.hovered_sidebar.as_ref() == Some(&location.path) {
        SidebarPresentation::Hovered
    } else {
        SidebarPresentation::Normal
    }
}

fn sidebar_device_presentation(
    browser: &FileBrowser,
    device: &SidebarDeviceEntry,
) -> SidebarPresentation {
    if browser.sidebar_device_is_selected(&device.id) {
        SidebarPresentation::Selected
    } else if browser.hovered_sidebar_device.as_ref() == Some(&device.id) {
        SidebarPresentation::Hovered
    } else {
        SidebarPresentation::Normal
    }
}

fn sidebar_network_connection_presentation(
    browser: &FileBrowser,
    connection: &SidebarNetworkConnectionEntry,
) -> SidebarPresentation {
    if browser.network_connection_is_selected(connection.id()) {
        SidebarPresentation::Selected
    } else if browser.hovered_network_connection.as_ref() == Some(connection.id()) {
        SidebarPresentation::Hovered
    } else {
        SidebarPresentation::Normal
    }
}

#[derive(Debug, Clone, Copy)]
enum SidebarPresentation {
    Normal,
    Hovered,
    Selected,
}

impl SidebarPresentation {
    fn is_selected(self) -> bool {
        matches!(self, Self::Selected)
    }
}

fn sidebar_label(icon: IconSymbol, label: &str, tone: IconTone) -> Row<'static, Message> {
    row![
        themed_icon(icon, tone, MENU_ICON_SIZE),
        measured_middle_ellipsized_text(label.to_owned(), 16)
    ]
    .spacing(8)
    .align_y(Alignment::Center)
}

fn sidebar_device_label(
    device: &SidebarDeviceEntry,
    pending: bool,
    tone: IconTone,
) -> Row<'static, Message> {
    let detail = sidebar_device_detail(device, pending);
    row![
        themed_icon(SIDEBAR_DEVICE_ICON_SYMBOL, tone, MENU_ICON_SIZE),
        column![
            measured_middle_ellipsized_text(device.label.clone(), 13),
            measured_middle_ellipsized_text(detail, 11)
        ]
        .spacing(1)
        .width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center)
}

fn sidebar_network_connection_label(
    connection: &SidebarNetworkConnectionEntry,
    pending: bool,
    tone: IconTone,
) -> Row<'static, Message> {
    let detail = sidebar_network_connection_detail(connection, pending);
    row![
        themed_icon(SIDEBAR_NETWORK_CONNECTION_ICON_SYMBOL, tone, MENU_ICON_SIZE),
        column![
            measured_middle_ellipsized_text(connection.label(), 13),
            measured_middle_ellipsized_text(detail, 11)
        ]
        .spacing(1)
        .width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center)
}

fn sidebar_network_connection_detail(
    connection: &SidebarNetworkConnectionEntry,
    pending: bool,
) -> String {
    if pending {
        crate::localization::translate_current("Working...")
    } else {
        match &connection.state {
            desktop_linux::NetworkMountState::Disconnected => {
                crate::localization::translate_current("Not connected")
            }
            desktop_linux::NetworkMountState::Connecting => {
                crate::localization::translate_current("Connecting...")
            }
            desktop_linux::NetworkMountState::Mounted(path) => path.to_string_lossy().into_owned(),
            desktop_linux::NetworkMountState::Error(_) => {
                crate::localization::translate_current("Connection error")
            }
        }
    }
}

fn sidebar_device_detail(device: &SidebarDeviceEntry, pending: bool) -> String {
    if pending {
        crate::localization::translate_current("Working...")
    } else if device.is_mounted() {
        device
            .detail
            .clone()
            .unwrap_or_else(|| crate::localization::translate_current("Mounted"))
    } else if device.size_bytes > 0 {
        if crate::localization::current_language_is_chinese() {
            format!("未挂载 · {}", format_file_size(device.size_bytes))
        } else {
            format!("Not mounted · {}", format_file_size(device.size_bytes))
        }
    } else {
        crate::localization::translate_current("Not mounted")
    }
}
