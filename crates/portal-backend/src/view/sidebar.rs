//! 选择窗口左侧栏：分节（位置/收藏/网络/设备）+ 行 + 垃圾桶 + 失败
//! 提示。数据与条目模型全部来自 `bennu-sidebar`，样式一律 bennu-theme
//! （portal 视觉词汇契约：禁本地配色）。滚动遵守接线契约：widget id
//! 按请求路径命名空间（`SessionScrollRegion::Sidebar`）。

use bennu_sidebar::entries::selected_sidebar_device;
use bennu_sidebar::{
    sidebar_icon_symbol, SidebarDeviceEntry, SidebarLocation, SidebarLocationKind,
    SidebarNetworkConnectionEntry, SIDEBAR_DEVICE_ICON_SYMBOL,
    SIDEBAR_NETWORK_CONNECTION_ICON_SYMBOL,
};
use bennu_theme::icons::{icon_tone_style, IconSymbol, IconTone};
use bennu_theme::measured_text::measured_middle_ellipsized_text;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, error_notification_style,
    hovered_sidebar_item_style, selected_sidebar_item_style, sidebar_style,
};
use desktop_linux::NetworkMountState;
use iced::widget::{column, container, mouse_area, row, scrollable, Space};
use iced::{alignment, Element, Length, Theme};

use crate::picker_session::scrollbar::{scroll_id, scrollbar_on_scroll};
use crate::picker_session::{PickerSession, SessionMessage, SessionScrollRegion, SidebarEntryId};

use super::readable_label;

/// 侧栏滚动条静态宽度（与主软件侧栏一致）。
const SIDEBAR_SCROLLBAR_WIDTH: f32 = 6.0;
/// 行图标尺寸（与主软件 MENU_ICON_SIZE 同值）。
const SIDEBAR_ICON_SIZE: f32 = 16.0;
/// 拖宽手柄宽度。
const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;

/// 标准位置的中文文案：主程序经 bennu-localization 翻译同一批键；
/// portal 无本地化层，取同一份中文（portal 既有 UI 文案即中文）。
fn location_label(location: &SidebarLocation) -> String {
    match location.kind {
        SidebarLocationKind::Home => "主目录".to_string(),
        SidebarLocationKind::Desktop => "桌面".to_string(),
        SidebarLocationKind::Documents => "文档".to_string(),
        SidebarLocationKind::Downloads => "下载".to_string(),
        SidebarLocationKind::Pictures => "图片".to_string(),
        SidebarLocationKind::Music => "音乐".to_string(),
        SidebarLocationKind::Videos => "视频".to_string(),
        // 收藏（配置/GTK 书签）保留用户自定义标签，与主程序一致。
        SidebarLocationKind::Bookmark => location.label.clone(),
    }
}

fn sidebar_row_style(selected: bool, hovered: bool) -> fn(&Theme) -> container::Style {
    if selected {
        selected_sidebar_item_style
    } else if hovered {
        hovered_sidebar_item_style
    } else {
        |_| container::Style::default()
    }
}

/// 行内容：图标 + 单行省略标签（像素测量组件，主程序侧栏同规则）。
fn sidebar_label(
    symbol: IconSymbol,
    label: String,
    tone: IconTone,
    label_size: u32,
) -> Element<'static, SessionMessage> {
    row![
        symbol.view(SIDEBAR_ICON_SIZE).style(icon_tone_style(tone)),
        measured_middle_ellipsized_text(label, label_size)
    ]
    .spacing(8)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// 设备/网络行：图标 + 主标签 + 详情副行（挂载点/容量/连接状态）。
fn sidebar_detailed_label(
    symbol: IconSymbol,
    label: String,
    detail: String,
    tone: IconTone,
) -> Element<'static, SessionMessage> {
    row![
        symbol.view(SIDEBAR_ICON_SIZE).style(icon_tone_style(tone)),
        column![
            measured_middle_ellipsized_text(label, 13),
            measured_middle_ellipsized_text(detail, 11)
        ]
        .spacing(1)
        .width(Length::Fill)
    ]
    .spacing(8)
    .align_y(alignment::Vertical::Center)
    .into()
}

fn sidebar_item(
    content: Element<'static, SessionMessage>,
    selected: bool,
    hovered: bool,
    on_press: SessionMessage,
    entry: SidebarEntryId,
) -> Element<'static, SessionMessage> {
    mouse_area(
        container(content)
            .padding([6, 8])
            .width(Length::Fill)
            .style(sidebar_row_style(selected, hovered)),
    )
    .on_press(on_press)
    .on_enter(SessionMessage::SidebarHoverChanged {
        entry: Some(entry.clone()),
    })
    .on_exit(SessionMessage::SidebarHoverChanged { entry: None })
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

fn section_label(label: &str) -> Element<'static, SessionMessage> {
    container(readable_label(label.to_string()).size(12))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}

fn location_item(
    session: &PickerSession,
    location: &SidebarLocation,
    is_favorite: bool,
) -> Element<'static, SessionMessage> {
    let id = SidebarEntryId::Location(location.path.clone());
    let selected = !session.is_trash_view() && location.path == session.directory();
    let hovered = session.sidebar().hovered.as_ref() == Some(&id);
    let tone = if selected {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    // 收藏显示自定义标签，标准位置显示本地化名（主程序同规则）。
    let label = if is_favorite {
        location.label.clone()
    } else {
        location_label(location)
    };
    sidebar_item(
        sidebar_label(sidebar_icon_symbol(location), label, tone, 16),
        selected,
        hovered,
        SessionMessage::SidebarLocationPressed {
            path: location.path.clone(),
        },
        id,
    )
}

fn trash_item(session: &PickerSession) -> Element<'static, SessionMessage> {
    let selected = session.is_trash_view();
    let tone = if selected {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    sidebar_item(
        sidebar_label(IconSymbol::Trash, "回收站".to_string(), tone, 16),
        selected,
        session.sidebar().hovered.as_ref() == Some(&SidebarEntryId::Trash),
        SessionMessage::SidebarTrashPressed,
        SidebarEntryId::Trash,
    )
}

fn device_detail(device: &SidebarDeviceEntry, mounting: bool) -> String {
    if mounting {
        "处理中...".to_string()
    } else if device.is_mounted() {
        device
            .detail
            .clone()
            .unwrap_or_else(|| "已挂载".to_string())
    } else if device.size_bytes > 0 {
        format!("未挂载 · {}", super::readable_size(device.size_bytes))
    } else {
        "未挂载".to_string()
    }
}

fn device_item(
    session: &PickerSession,
    device: &SidebarDeviceEntry,
) -> Element<'static, SessionMessage> {
    let id = SidebarEntryId::Device(device.id.clone());
    // 当前目录所属设备高亮：最长挂载点前缀命中（共享判定，与主程序同源）。
    let selected = !session.is_trash_view()
        && selected_sidebar_device(&session.sidebar().devices, session.directory())
            .is_some_and(|selected| selected.id == device.id);
    let tone = if selected {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    sidebar_item(
        sidebar_detailed_label(
            SIDEBAR_DEVICE_ICON_SYMBOL,
            device.label.clone(),
            device_detail(device, session.sidebar_device_is_mounting(&device.id)),
            tone,
        ),
        selected,
        session.sidebar().hovered.as_ref() == Some(&id),
        SessionMessage::SidebarDevicePressed {
            id: device.id.clone(),
        },
        id,
    )
}

fn connection_detail(connection: &SidebarNetworkConnectionEntry) -> String {
    match &connection.state {
        NetworkMountState::Disconnected => "未连接".to_string(),
        NetworkMountState::Connecting => "连接中...".to_string(),
        NetworkMountState::Mounted(path) => path.to_string_lossy().into_owned(),
        NetworkMountState::Error(_) => "连接错误".to_string(),
    }
}

fn connection_item(
    session: &PickerSession,
    connection: &SidebarNetworkConnectionEntry,
) -> Element<'static, SessionMessage> {
    let id = SidebarEntryId::Connection(connection.id().clone());
    let selected = !session.is_trash_view()
        && connection
            .mount_path()
            .is_some_and(|mount_path| session.directory().starts_with(mount_path));
    let tone = if selected {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let detail = if matches!(connection.state, NetworkMountState::Connecting) {
        "处理中...".to_string()
    } else {
        connection_detail(connection)
    };
    sidebar_item(
        sidebar_detailed_label(
            SIDEBAR_NETWORK_CONNECTION_ICON_SYMBOL,
            connection.label(),
            detail,
            tone,
        ),
        selected,
        session.sidebar().hovered.as_ref() == Some(&id),
        SessionMessage::SidebarConnectionPressed {
            id: connection.id().clone(),
        },
        id,
    )
}

/// 侧边栏主体（含滚动接线与拖宽手柄），由 picker_window_view 组进
/// 顶层 row 的左段。宽度当次窗口有效，不持久化。
pub(super) fn sidebar_panel(session: &PickerSession) -> Element<'static, SessionMessage> {
    let mut content = column![].spacing(6).padding(12);

    // 位置：Home + 垃圾桶（与主程序 Places 块同序）。
    for location in session
        .sidebar()
        .locations
        .iter()
        .filter(|location| !location.kind.is_user_favorite())
    {
        content = content.push(location_item(session, location, false));
    }
    content = content.push(trash_item(session));

    // 收藏。
    let favorites = session
        .sidebar()
        .locations
        .iter()
        .filter(|location| location.kind.is_user_favorite())
        .collect::<Vec<_>>();
    if !favorites.is_empty() {
        content = content.push(section_label("收藏夹"));
        for location in favorites {
            content = content.push(location_item(session, location, true));
        }
    }

    // 网络。
    if !session.sidebar().connections.is_empty() {
        content = content.push(section_label("网络"));
        for connection in &session.sidebar().connections {
            content = content.push(connection_item(session, connection));
        }
    }

    // 设备。
    if !session.sidebar().devices.is_empty() {
        content = content.push(section_label("设备"));
        for device in &session.sidebar().devices {
            content = content.push(device_item(session, device));
        }
    }

    // 失败提示（挂载失败等；下次成功操作清除）。样式与确认栏的
    // 覆盖提示同源（bennu-theme error_notification_style）。
    if let Some(notice) = session.sidebar().notice.clone() {
        content = content.push(
            container(readable_label(notice).size(12))
                .padding([4, 8])
                .width(Length::Fill)
                .style(error_notification_style),
        );
    }

    let region = SessionScrollRegion::Sidebar;
    let scrollbar_visibility = session.scrollbar_visibility_for(&region);
    let scrollbar_viewport = session.scrollbar_viewport_for(&region);
    let request_path = session.request_path().to_string();

    // 滚轮走 SmoothScrollArea → WheelScrolled{region}（main 层拦截路由），
    // widget id 按请求路径命名空间，多窗不串。
    let scroller = scrollable(super::smooth_scroll_region(
        content,
        region,
        session.scroll_shift_pressed(),
    ))
    .id(scroll_id(&request_path, region))
    .width(Length::Fill)
    .height(Length::Fill)
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        SIDEBAR_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .on_scroll(scrollbar_on_scroll(region, |_| {
        SessionMessage::ScrollbarEngaged {
            region: SessionScrollRegion::Sidebar,
        }
    }));

    let panel = container(enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        SIDEBAR_SCROLLBAR_WIDTH,
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .style(sidebar_style);

    container(
        row![panel, resize_handle()]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(session.sidebar_width()))
    .height(Length::Fill)
    .into()
}

/// 拖宽手柄：6px 窄条，左右光标语义；按下发 `SidebarResizeStarted`，
/// 增量与收尾由 main 层的指针簿记完成。
fn resize_handle() -> Element<'static, SessionMessage> {
    let handle = container(Space::new().width(Length::Fixed(SIDEBAR_RESIZE_HANDLE_WIDTH)))
        .width(Length::Fixed(SIDEBAR_RESIZE_HANDLE_WIDTH))
        .height(Length::Fill);
    mouse_area(handle)
        .on_press(SessionMessage::SidebarResizeStarted)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}
