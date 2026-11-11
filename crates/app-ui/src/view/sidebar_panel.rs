use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, Column, Row, Space,
};
use iced::{Alignment, Element, Length, Padding};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::transfer::TransferMessage;
use crate::app::FileBrowser;
use crate::app::SidebarFixedEntry;
use crate::appearance::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, hovered_sidebar_item_style,
    selected_sidebar_item_style, sidebar_bookmark_drop_slot_style,
};
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::formatting::format_file_size;
use crate::model::{
    trash_location_path, FileDropTarget, Message, ScrollbarRegion, SidebarBookmarkDropSlot,
    SidebarLocation,
};
use crate::network_connections::{NetworkConnectionMessage, SidebarNetworkConnectionEntry};
use crate::sidebar_devices::SidebarDeviceEntry;
use bennu_preview::panel_text::readable_text;
use bennu_sidebar::SidebarLocationKind;
use bennu_theme::icons::IconSymbol;
use bennu_theme::measured_text::measured_middle_ellipsized_text;
use iced::alignment::Vertical;

use super::{tab_motion, themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::{icon_button, icon_button_style};
use bennu_theme::typography::{BODY_SIZE, BOLD, CAPTION_SIZE};

/// 位置行图标。侧边栏数据 crate 不依赖图标词汇，映射由两个宿主的视图各持一份
/// （portal 侧栏同一张表）：没有同时能看到两种类型的共享 crate，为一张 8 行的表不值得再建一个。
fn location_icon_symbol(location: &SidebarLocation) -> IconSymbol {
    match location.kind {
        SidebarLocationKind::Home => IconSymbol::House,
        SidebarLocationKind::Desktop => IconSymbol::Monitor,
        SidebarLocationKind::Documents => IconSymbol::FileText,
        SidebarLocationKind::Downloads => IconSymbol::Download,
        SidebarLocationKind::Pictures => IconSymbol::FileImage,
        SidebarLocationKind::Music => IconSymbol::Music,
        SidebarLocationKind::Videos => IconSymbol::Video,
        SidebarLocationKind::Bookmark => IconSymbol::Bookmark,
    }
}

const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;
const SIDEBAR_BOOKMARK_DROP_SLOT_HEIGHT: f32 = 3.0;
const SIDEBAR_ROW_PADDING_X: f32 = 8.0;

// 侧栏列表的几何：界面按这些常量排，收藏夹拖放落点（app/sidebar_bookmarks）按同样的
// 常量算。行高必须固定——跟着字号变的行高曾让落点整体偏移（2026-11-10）。
pub(crate) const SIDEBAR_ROW_HEIGHT: f32 = 32.0;
pub(crate) const SIDEBAR_ROW_SPACING: f32 = 2.0;
pub(crate) const SIDEBAR_SECTION_LABEL_HEIGHT: f32 = 30.0;
pub(crate) const SIDEBAR_LIST_PADDING_TOP: f32 = 4.0;
const SIDEBAR_LIST_PADDING_X: f32 = 8.0;
const SIDEBAR_LIST_PADDING_BOTTOM: f32 = 12.0;

/// 侧栏顶栏右边：设置按钮（不写标题，同 Nautilus）。
pub(crate) fn sidebar_settings_button() -> Element<'static, Message> {
    icon_button(themed_icon(
        IconSymbol::Settings,
        IconTone::Normal,
        MENU_ICON_SIZE,
    ))
    .on_press(Message::SettingsOpened)
    .into()
}

/// 侧栏顶栏下面的部分：可滚动的位置列表，右边缘是拖宽手柄。
pub(crate) fn sidebar_body(browser: &FileBrowser) -> Element<'_, Message> {
    let mut sidebar = Column::new().spacing(SIDEBAR_ROW_SPACING).padding(Padding {
        top: SIDEBAR_LIST_PADDING_TOP,
        right: SIDEBAR_LIST_PADDING_X,
        bottom: SIDEBAR_LIST_PADDING_BOTTOM,
        left: SIDEBAR_LIST_PADDING_X,
    });
    for entry in browser.sidebar_fixed_entries() {
        sidebar = sidebar.push(match entry {
            SidebarFixedEntry::Location(location) => sidebar_location_item(browser, location),
            SidebarFixedEntry::Trash => sidebar_trash_item(browser),
            SidebarFixedEntry::LocalSend => sidebar_local_send_item(browser),
        });
    }

    let favorite_locations = browser
        .sidebar_locations
        .iter()
        .filter(|location| location.kind.is_user_favorite())
        .collect::<Vec<_>>();
    let can_drop_bookmark = browser.can_drop_sidebar_bookmark();
    if can_drop_bookmark || !favorite_locations.is_empty() {
        let favorites_label = sidebar_section_label(tr!("Favorites"));
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

    row![sidebar_content, sidebar_resize_handle()]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
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

    sidebar = sidebar.push(sidebar_section_label(tr!("Devices")));
    for device in &browser.sidebar_devices.devices {
        sidebar = sidebar.push(sidebar_device_item(browser, device));
    }

    if let Some(message) =
        sidebar_device_provider_failure_message(&browser.sidebar_devices.provider_failures)
    {
        sidebar = sidebar.push(sidebar_message_row(message));
    } else if browser.sidebar_devices.devices.is_empty() && browser.sidebar_devices.is_loading {
        sidebar = sidebar.push(sidebar_message_row(tr!("Loading devices...")));
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
        sidebar = sidebar.push(sidebar_message_row(tr!("No saved connections")));
    } else if browser.network_connections.unavailable.is_some() {
        sidebar = sidebar.push(sidebar_message_row(tr!("Network status unavailable")));
    }

    sidebar
}

fn sidebar_network_section_header() -> Element<'static, Message> {
    row![
        container(section_label_text(tr!("Network")))
            .padding([0.0, SIDEBAR_ROW_PADDING_X])
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
        .style(icon_button_style),
    ]
    .height(Length::Fixed(SIDEBAR_SECTION_LABEL_HEIGHT))
    .align_y(Alignment::End)
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
    let label = location.display_label();

    let item_container = container(sidebar_label(location_icon_symbol(location), &label, tone))
        .padding([0.0, SIDEBAR_ROW_PADDING_X])
        .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
        .align_y(Vertical::Center)
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
) -> Option<String> {
    match failures {
        [] => None,
        [failure] => Some(match failure.provider {
            desktop_linux::StorageDeviceProvider::Udisks => tr!("UDisks devices unavailable"),
            desktop_linux::StorageDeviceProvider::Gvfs => tr!("GVfs devices unavailable"),
        }),
        _ => Some(tr!("Some device providers unavailable")),
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
        .padding([0.0, SIDEBAR_ROW_PADDING_X])
        .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
        .align_y(Vertical::Center)
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
        .padding([0.0, SIDEBAR_ROW_PADDING_X])
        .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
        .align_y(Vertical::Center)
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
    let trash_presentation = if browser.tab().is_trash_view {
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
    let trash_label = tr!("Trash");
    let trash_container = container(sidebar_label(IconSymbol::Trash, &trash_label, trash_tone))
        .padding([0.0, SIDEBAR_ROW_PADDING_X])
        .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
        .align_y(Vertical::Center)
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

/// LocalSend 入口：点开窗口；拖上来的文件进待发列表。它不是路径，悬停用按钮自身
/// 的状态出样式，不往按路径记的 `hovered_sidebar` 里塞假路径。
fn sidebar_local_send_item(browser: &FileBrowser) -> Element<'_, Message> {
    let drop_hovered = browser
        .file_drop_session
        .as_ref()
        .is_some_and(|session| session.hovered_target == Some(FileDropTarget::LocalSend));
    let item = button(
        container(sidebar_label(
            IconSymbol::Send,
            &tr!("LocalSend"),
            IconTone::Normal,
        ))
        .height(Length::Fill)
        .align_y(Vertical::Center),
    )
    .on_press(Message::Transfer(TransferMessage::LocalSendWindowRequested))
    .padding([0.0, SIDEBAR_ROW_PADDING_X])
    .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
    .width(Length::Fill)
    .style(move |theme, status| {
        let highlighted =
            drop_hovered || matches!(status, button::Status::Hovered | button::Status::Pressed);
        let surface = if highlighted {
            hovered_sidebar_item_style(theme)
        } else {
            container::Style::default()
        };
        button::Style {
            background: surface.background,
            text_color: surface
                .text_color
                .unwrap_or_else(|| theme.extended_palette().background.base.text),
            border: surface.border,
            shadow: surface.shadow,
            snap: surface.snap,
        }
    });
    // 应用内拖拽在这里松手时要结束拖拽（同其他侧栏行）；按钮只在自己被按下时
    // 才吃掉松开事件，拖拽经过时不会。
    track_file_drag_hit_test_marker(
        mouse_area(item).on_release(Message::DragSelectionFinished),
        FileDragHitTestMarker::SidebarLocalSend,
    )
}

/// 分组标题（收藏夹、网络、设备）：小号粗体灰字，固定高度——收藏夹拖放落点按它算。
fn sidebar_section_label(label: String) -> Element<'static, Message> {
    container(section_label_text(label))
        .padding([0.0, SIDEBAR_ROW_PADDING_X])
        .height(Length::Fixed(SIDEBAR_SECTION_LABEL_HEIGHT))
        .align_y(Vertical::Bottom)
        .width(Length::Fill)
        .into()
}

fn section_label_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .font(BOLD)
        .style(|theme: &iced::Theme| iced::widget::text::Style {
            color: Some(crate::appearance::muted_text_color(theme)),
        })
        .into()
}

fn sidebar_message_row(message: String) -> Element<'static, Message> {
    container(readable_text(message))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}

fn sidebar_presentation(browser: &FileBrowser, location: &SidebarLocation) -> SidebarPresentation {
    if !browser.tab().is_trash_view && location.path == browser.tab().current_dir {
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
        measured_middle_ellipsized_text(label.to_owned(), BODY_SIZE)
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
        themed_icon(IconSymbol::HardDrive, tone, MENU_ICON_SIZE),
        column![
            measured_middle_ellipsized_text(device.label.clone(), BODY_SIZE),
            measured_middle_ellipsized_text(detail, CAPTION_SIZE)
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
        themed_icon(IconSymbol::Link, tone, MENU_ICON_SIZE),
        column![
            measured_middle_ellipsized_text(connection.label(), BODY_SIZE),
            measured_middle_ellipsized_text(detail, CAPTION_SIZE)
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
        tr!("Working...")
    } else {
        match &connection.state {
            desktop_linux::NetworkMountState::Disconnected => tr!("Not connected"),
            desktop_linux::NetworkMountState::Connecting => tr!("Connecting..."),
            desktop_linux::NetworkMountState::Mounted(path) => path.to_string_lossy().into_owned(),
            desktop_linux::NetworkMountState::Error(_) => tr!("Connection error"),
        }
    }
}

fn sidebar_device_detail(device: &SidebarDeviceEntry, pending: bool) -> String {
    if pending {
        tr!("Working...")
    } else if device.is_mounted() {
        device.detail.clone().unwrap_or_else(|| tr!("Mounted"))
    } else if device.size_bytes > 0 {
        tr!("Not mounted · %1", format_file_size(device.size_bytes))
    } else {
        tr!("Not mounted")
    }
}
