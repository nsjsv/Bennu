//! LocalSend 设备行："发送到手机"窗口与 LocalSend 窗口共用。
//!
//! 两块：收藏（全部收藏设备，不在线的灰着）与附近（没被收藏认领的设备）。每行：
//! 勾选框 + 类型图标 + 名字 + 小字 + 这台设备最近一次直推的状态 + ☆/★ + 发送。
//! 勾选按窗口各记一份（`DeviceListOwner`）。

use iced::widget::{button, checkbox, column, container, row, tooltip, Column};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{DeviceListOwner, FavoriteRow, TransferMessage};
use crate::app::FileBrowser;
use crate::appearance::{context_menu_button_style, muted_text_color};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use local_send::DeviceInfo;

use super::local_send_send_status::direct_send_lines;
use super::option_controls::tooltip_panel_style;
use super::settings_group::muted_setting_text;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const DEVICE_ICON_SIZE: f32 = 20.0;

/// 设备行共用的上下文：哪个窗口的勾选、点"发送"发什么。
pub(super) struct DeviceRows<'a, F: Fn(&DeviceInfo) -> Option<Message>> {
    pub(super) browser: &'a FileBrowser,
    pub(super) owner: DeviceListOwner,
    /// 这台设备的发送消息；没有可发的文件时返回 None。
    pub(super) send: F,
}

/// 勾了设备就出：给勾着的在线设备发送（`can_send` = 有文件可发）+ 取消勾选。
pub(super) fn checked_bar(
    browser: &FileBrowser,
    owner: DeviceListOwner,
    can_send: bool,
) -> Option<Element<'static, Message>> {
    if browser.checked_devices.of(owner).is_empty() {
        return None;
    }
    let count = browser.checked_online_devices(owner).len();
    let action = |label: String, message: Option<TransferMessage>| {
        button(readable_text(label).size(12))
            .on_press_maybe(message.map(Message::Transfer))
            .padding([5, 12])
            .style(context_menu_button_style())
    };
    Some(
        row![
            action(
                tr!("Send to %1 selected", count),
                (can_send && count > 0).then_some(TransferMessage::SendToCheckedPressed(owner)),
            ),
            action(
                tr!("Clear selection"),
                Some(TransferMessage::CheckedCleared(owner)),
            ),
        ]
        .spacing(8)
        .into(),
    )
}

/// 标题 + 手动刷新：立即清空快照并触发一轮扫描（不等 10s 周期）。
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

pub(super) fn favorites_title<'a>() -> iced::widget::Text<'a, Theme> {
    readable_text(tr!("Favorite devices")).size(13)
}

/// 收藏行。`send` 给出这台设备的发送消息；没有可发的文件时返回 None。不在线的
/// 收藏发不了：后台按记住的地址一直在找，找到了就变成在线。
pub(super) fn favorite_rows<'a>(
    rows_of: &DeviceRows<'a, impl Fn(&DeviceInfo) -> Option<Message>>,
) -> Column<'a, Message> {
    let browser = rows_of.browser;
    let mut rows = column![].spacing(8).width(Length::Fill);
    for FavoriteRow { favorite, online } in browser.favorite_rows() {
        let unfavor = star_button(
            IconSymbol::StarSolid,
            Some(TransferMessage::UnfavoritePressed(
                favorite.fingerprint.clone(),
            )),
        );
        // 名字是对方自报或用户起的，不做本地化。
        let name = readable_text(favorite.alias.clone()).size(12);
        rows = rows.push(match online {
            Some(device) => online_row(rows_of, device, name.into(), unfavor),
            None => {
                let last_seen = match favorite.address {
                    Some(address) => tr!("Offline · last seen at %1", address.ip()),
                    None => tr!("Offline · not found yet"),
                };
                let details = column![
                    name.style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(muted_text_color(theme)),
                    }),
                    muted_setting_text(last_seen, 11),
                ]
                .spacing(2);
                // 不在线的勾不了：发不出去。
                device_row(
                    checkbox(false).into(),
                    IconSymbol::Monitor,
                    details,
                    unfavor,
                    None,
                )
            }
        });
    }
    rows
}

/// 附近行：没被收藏认领的设备。明文设备收藏不了，☆ 灰着并说明原因。
pub(super) fn nearby_rows<'a>(
    rows_of: &DeviceRows<'a, impl Fn(&DeviceInfo) -> Option<Message>>,
) -> Column<'a, Message> {
    let browser = rows_of.browser;
    let devices = browser.unclaimed_nearby_devices();
    if devices.is_empty() {
        let any_favorite_online = browser
            .favorite_rows()
            .iter()
            .any(|row| row.online.is_some());
        let empty = if any_favorite_online {
            tr!("No other devices found")
        } else {
            tr!("No nearby devices found")
        };
        return column![readable_text(empty).size(12)];
    }
    let mut rows = column![].spacing(8).width(Length::Fill);
    for device in devices {
        let star = if device.is_certified() {
            star_button(
                IconSymbol::Star,
                Some(TransferMessage::FavoritePressed(device.clone())),
            )
        } else {
            tooltip(
                star_button(IconSymbol::Star, None),
                container(
                    readable_text(tr!(
                        "This device has encryption turned off, so it cannot be a favorite."
                    ))
                    .size(11),
                )
                .padding([5, 7])
                .style(tooltip_panel_style),
                tooltip::Position::Left,
            )
            .into()
        };
        let name = readable_text(device.alias.clone()).size(12).into();
        rows = rows.push(online_row(rows_of, device, name, star));
    }
    rows
}

/// 在线设备的一行：小字"型号 · IP"、直推状态、发送（正在发给这台时不可点：同一台
/// 设备同时只有一次直推）。
fn online_row<'a>(
    rows_of: &DeviceRows<'a, impl Fn(&DeviceInfo) -> Option<Message>>,
    device: &'a DeviceInfo,
    name: Element<'a, Message>,
    star: Element<'a, Message>,
) -> Element<'a, Message> {
    let browser = rows_of.browser;
    let mut details = column![name].spacing(2);
    let subtitle = device_subtitle(device);
    if !subtitle.is_empty() {
        details = details.push(muted_setting_text(subtitle, 11));
    }
    if let Some(state) = browser.direct_send_to(device) {
        details = details.push(direct_send_lines(browser, &device.fingerprint, state));
    }
    let owner = rows_of.owner;
    let fingerprint = device.fingerprint.clone();
    let check = checkbox(
        browser
            .checked_devices
            .of(owner)
            .contains(&device.fingerprint),
    )
    .on_toggle(move |checked| {
        Message::Transfer(TransferMessage::DeviceChecked {
            owner,
            fingerprint: fingerprint.clone(),
            checked,
        })
    });
    let message = (rows_of.send)(device).filter(|_| browser.can_send_to(device));
    device_row(check.into(), device_icon(device), details, star, message)
}

fn device_row<'a>(
    check: Element<'a, Message>,
    icon: IconSymbol,
    details: Column<'a, Message>,
    star: Element<'a, Message>,
    send: Option<Message>,
) -> Element<'a, Message> {
    let send_button = button(readable_text(tr!("Send")).size(12))
        .on_press_maybe(send)
        .padding([5, 12])
        .style(context_menu_button_style());
    row![
        check,
        themed_icon(icon, IconTone::Normal, DEVICE_ICON_SIZE),
        details.width(Length::Fill),
        star,
        send_button,
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn star_button(symbol: IconSymbol, message: Option<TransferMessage>) -> Element<'static, Message> {
    button(themed_icon(symbol, IconTone::Normal, MENU_ICON_SIZE))
        .on_press_maybe(message.map(Message::Transfer))
        .padding([4, 6])
        .style(context_menu_button_style())
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
