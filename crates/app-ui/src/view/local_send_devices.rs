//! 设备区上半截，LocalSend 和“发送到手机”两个窗口共用：标题 + 刷新、一行提示、勾了几台
//! 时的“一起发”。下面的设备格子见 `local_send_tiles`。勾选按窗口各记一份（`DeviceListOwner`）。

use iced::widget::{column, row, Column};
use iced::{Alignment, Element, Length};

use crate::app::transfer::{DeviceListOwner, TransferMessage};
use crate::app::FileBrowser;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{icon_button, normal_button};
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::BOLD;

use super::settings_group::muted_setting_text;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

pub(super) fn devices_section_head(
    browser: &FileBrowser,
    owner: DeviceListOwner,
) -> Column<'_, Message> {
    // 手动刷新：立即清空快照并触发一轮扫描（不等 10s 周期）。
    let refresh = icon_button(themed_icon(
        IconSymbol::Refresh,
        IconTone::Normal,
        MENU_ICON_SIZE,
    ))
    .on_press(Message::Transfer(TransferMessage::RefreshDevicesRequested));
    let can_send = browser.has_outgoing(owner);
    let any_device =
        !browser.favorite_rows().is_empty() || !browser.unclaimed_nearby_devices().is_empty();
    let mut head = column![
        row![
            readable_text(tr!("Devices")).font(BOLD).width(Length::Fill),
            refresh,
        ]
        .align_y(Alignment::Center),
        muted_setting_text(if !any_device {
            tr!("Looking for nearby devices. The other device needs LocalSend open on the same network.")
        } else if can_send {
            tr!("Click a device to send. Hold Ctrl to pick several.")
        } else {
            tr!("Choose files or type some text, then click a device.")
        }),
    ]
    .spacing(10)
    .width(Length::Fill);
    if let Some(bar) = checked_bar(browser, owner, can_send) {
        head = head.push(bar);
    }
    head
}

/// 勾了设备就出：给勾着的在线设备发送（`can_send` = 有东西可发）+ 取消勾选。
fn checked_bar(
    browser: &FileBrowser,
    owner: DeviceListOwner,
    can_send: bool,
) -> Option<Element<'static, Message>> {
    if browser.checked_devices.of(owner).is_empty() {
        return None;
    }
    let count = browser.checked_online_devices(owner).len();
    let action = |label: String, message: Option<TransferMessage>| {
        normal_button(readable_text(label)).on_press_maybe(message.map(Message::Transfer))
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
