//! 设置 → 传输：接收开关与原因、端口、接收目录、设备名、信任设备。

use iced::widget::{button, column, container, row, text_input};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{receiver_failure_message, TransferMessage};
use crate::app::FileBrowser;
use crate::appearance::context_menu_button_style;
use crate::config::TRANSFER_PORT_RANGE;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;

use super::settings_group::toggle_setting_row;
use super::{themed_icon, IconTone};

/// "接收"组的行：开关 → 没开起来的原因（有才显示）→ 端口 → 接收目录 → 设备名。
pub(super) fn transfer_receive_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let mut rows = vec![toggle_setting_row(
        tr!("Receive files"),
        Some(tr!(
            "When off, other devices cannot find this computer. You can still send files."
        )),
        browser.user_config().transfer_receive_enabled,
        Message::Transfer(TransferMessage::ReceiveEnabledToggled),
    )];
    if let Some(failure) = browser.transfer_receiver_failure() {
        rows.push(error_line(receiver_failure_message(failure)));
    }
    rows.push(port_row(browser));
    rows.push(receive_directory_row(browser));
    rows.push(device_alias_row(browser));
    rows
}

/// 红字错误行：与批量重命名的错误状态同一取色。
fn error_line(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(11)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 端口行：输入 + Save；下方常驻"两端端口要一致"的提示，上次提交不合法时再加红字。
fn port_row(browser: &FileBrowser) -> Element<'_, Message> {
    let input = text_input(
        &local_send::DEFAULT_PORT.to_string(),
        &browser.transfer_port_input,
    )
    .on_input(|value| Message::Transfer(TransferMessage::PortInputChanged(value)))
    .on_submit(Message::Transfer(TransferMessage::PortCommitted))
    .padding([6, 8])
    .size(12)
    .width(Length::Fill);
    let save = button(container(readable_text(tr!("Save")).size(12)).padding([6, 10]))
        .on_press(Message::Transfer(TransferMessage::PortCommitted))
        .style(context_menu_button_style());
    let mut content = column![
        row![
            readable_text(tr!("Port")).size(12).width(Length::Fill),
            input,
            save,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        readable_text(tr!(
            "If you change the port, other devices must use the same port to find each other."
        ))
        .size(11),
    ]
    .spacing(3);
    if browser.transfer_port_invalid {
        content = content.push(error_line(tr!(
            "Port must be between %1 and %2.",
            TRANSFER_PORT_RANGE.start(),
            TRANSFER_PORT_RANGE.end()
        )));
    }
    content.into()
}

/// 接收目录行：当前生效目录 + 选择按钮；缺省显示 ~/Downloads。
fn receive_directory_row(browser: &FileBrowser) -> Element<'_, Message> {
    let configured = browser
        .user_config()
        .transfer_download_dir
        .clone()
        .map(|dir| dir.to_string_lossy().into_owned());
    let current = configured.unwrap_or_else(|| tr!("Default: ~/Downloads"));
    let choose = button(
        row![
            themed_icon(IconSymbol::Folder, IconTone::Normal, 14.0),
            readable_text(tr!("Choose...")).size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Message::Transfer(
        TransferMessage::DownloadDirChooserPressed,
    ))
    .padding([6, 10])
    .style(context_menu_button_style());
    column![
        row![
            readable_text(tr!("Receive directory"))
                .size(12)
                .width(Length::Fill),
            readable_text(current).size(12),
            choose,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        readable_text(tr!(
            "Files received from other devices are saved to this directory"
        ))
        .size(11),
    ]
    .spacing(3)
    .into()
}

/// 设备名行：输入 + Save（提交后重启常驻服务生效）。
fn device_alias_row(browser: &FileBrowser) -> Element<'_, Message> {
    let input = text_input(&tr!("Device name"), &browser.transfer_device_alias_input)
        .on_input(|value| Message::Transfer(TransferMessage::DeviceAliasInputChanged(value)))
        .on_submit(Message::Transfer(TransferMessage::DeviceAliasCommitted))
        .padding([6, 8])
        .size(12)
        .width(Length::Fill);
    let save = button(container(readable_text(tr!("Save")).size(12)).padding([6, 10]))
        .on_press(Message::Transfer(TransferMessage::DeviceAliasCommitted))
        .style(context_menu_button_style());
    row![
        readable_text(tr!("Device name"))
            .size(12)
            .width(Length::Fill),
        input,
        save,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// 信任设备组：逐行展示别名 + 移除按钮；空表给说明文案。
pub(super) fn trusted_devices_section(browser: &FileBrowser) -> Element<'_, Message> {
    let trusted = &browser.user_config().transfer_trusted_devices;
    if trusted.is_empty() {
        return column![
            readable_text(tr!("No trusted devices")).size(12),
            readable_text(tr!(
                "Files received from trusted devices are accepted automatically"
            ))
            .size(11),
        ]
        .spacing(3)
        .into();
    }
    let mut rows = Vec::with_capacity(trusted.len());
    for device in trusted {
        let fingerprint = device.fingerprint.clone();
        let remove = button(readable_text(tr!("Remove")).size(11))
            .on_press(Message::Transfer(TransferMessage::TrustedDeviceRemoved(
                fingerprint,
            )))
            .padding([4, 8])
            .style(context_menu_button_style());
        rows.push(
            row![
                readable_text(device.alias.clone())
                    .size(12)
                    .width(Length::Fill),
                remove,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    column(rows).spacing(6).width(Length::Fill).into()
}
