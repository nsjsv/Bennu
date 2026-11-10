//! LocalSend 设备行："发送到手机"窗口与 LocalSend 窗口共用。
//!
//! 每行：类型图标 + 名字 + 小字"型号 · IP" + 这台设备最近一次直推的状态 + 发送。

use iced::widget::{button, column, row, Column};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{DirectSendState, TransferMessage};
use crate::app::FileBrowser;
use crate::appearance::context_menu_button_style;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;
use local_send::DeviceInfo;

use super::settings_group::muted_setting_text;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const DEVICE_ICON_SIZE: f32 = 20.0;
const STATUS_MAX_CHARS: usize = 72;

/// 标题 + 手动刷新：立即清空快照并触发一轮子网扫描（不等 10s 周期）。
pub(super) fn nearby_devices_header() -> Element<'static, Message> {
    row![
        readable_text(tr!("Nearby LocalSend devices"))
            .size(13)
            .width(Length::Fill),
        button(themed_icon(
            IconSymbol::Refresh,
            IconTone::Normal,
            MENU_ICON_SIZE
        ))
        .on_press(Message::Transfer(TransferMessage::RefreshDevicesRequested))
        .padding([4, 8])
        .style(context_menu_button_style()),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

pub(super) fn nearby_devices(browser: &FileBrowser) -> &[DeviceInfo] {
    browser
        .transfer_runtime
        .as_ref()
        .map(|runtime| runtime.devices.as_slice())
        .unwrap_or(&[])
}

/// 设备行列表。`send` 给出这台设备的发送消息；没有可发的文件时返回 None。
/// 正在发给这台设备时按钮一律不可点：同一台设备同时只有一次直推。
pub(super) fn device_rows<'a>(
    browser: &'a FileBrowser,
    send: impl Fn(&DeviceInfo) -> Option<Message>,
) -> Column<'a, Message> {
    let devices = nearby_devices(browser);
    if devices.is_empty() {
        return column![readable_text(tr!("No nearby devices found")).size(12)];
    }
    let mut rows = column![].spacing(8).width(Length::Fill);
    for device in devices {
        let message = send(device).filter(|_| browser.can_send_to(device));
        rows = rows.push(device_row(device, browser.direct_send_to(device), message));
    }
    rows
}

fn device_row<'a>(
    device: &'a DeviceInfo,
    state: Option<&'a DirectSendState>,
    send: Option<Message>,
) -> Element<'a, Message> {
    // 设备名、型号是对端自报的用户内容，不做本地化。
    let mut details = column![readable_text(device.alias.clone()).size(12)].spacing(2);
    let subtitle = device_subtitle(device);
    if !subtitle.is_empty() {
        details = details.push(muted_setting_text(subtitle, 11));
    }
    if let Some(state) = state {
        details = details.push(direct_send_status(state));
    }
    let send_button = button(readable_text(tr!("Send")).size(12))
        .on_press_maybe(send)
        .padding([5, 12])
        .style(context_menu_button_style());
    row![
        themed_icon(device_icon(device), IconTone::Normal, DEVICE_ICON_SIZE),
        details.width(Length::Fill),
        send_button,
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// "型号 · IP"：同名设备靠它分辨。
fn device_subtitle(device: &DeviceInfo) -> String {
    let address = device.address.map(|address| address.to_string());
    [device.device_model.clone(), address]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 按官方设备类型词表取图标；官方不认识的类型按 desktop 处理，这里同样落到显示器。
fn device_icon(device: &DeviceInfo) -> IconSymbol {
    match device.device_type.as_deref() {
        Some("mobile") => IconSymbol::Smartphone,
        Some("headless" | "server") => IconSymbol::HardDrive,
        _ => IconSymbol::Monitor,
    }
}

fn direct_send_status(state: &DirectSendState) -> Element<'static, Message> {
    match &state.outcome {
        Some(Ok(())) => muted_setting_text(tr!("Sent"), 11),
        Some(Err(error)) => {
            let label =
                format_middle_ellipsized_text(&tr!("Send failed: %1", error), STATUS_MAX_CHARS);
            readable_text(label)
                .size(11)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).error),
                })
                .into()
        }
        None if state.total_files > 0 => muted_setting_text(
            tr!(
                "Sending %1/%2 · %3%",
                // 协议层的序号从 0 开始。
                state.file_index + 1,
                state.total_files,
                file_percent(state)
            ),
            11,
        ),
        None => muted_setting_text(tr!("Sending..."), 11),
    }
}

/// 当前文件的进度百分比；大小为 0 的文件直接算完成。
fn file_percent(state: &DirectSendState) -> u64 {
    if state.file_size == 0 {
        return 100;
    }
    (state.bytes_sent.min(state.file_size) * 100) / state.file_size
}
