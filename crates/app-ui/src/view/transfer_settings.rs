//! 设置 → 传输：接收开关与原因、接收确认、PIN、端口、接收目录、设备名、收藏设备。

use iced::widget::{column, row};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{receiver_failure_message, TransferMessage};
use crate::app::FileBrowser;
use crate::config::{TransferAcceptMode, TRANSFER_PORT_RANGE};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;

use super::settings_group::toggle_setting_row;
use super::{themed_icon, IconTone};
use bennu_theme::controls::normal_button;
use bennu_theme::controls::text_field;
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};
use bennu_theme::typography::CAPTION_SIZE;

/// "接收"组的行：开关 → 没开起来的原因（有才显示）→ 接收确认 → PIN → 端口 → 接收目录
/// → 设备名。
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
    rows.push(accept_mode_row(browser));
    rows.push(toggle_setting_row(
        tr!("Require PIN to receive"),
        None,
        browser.transfer_pin_input.is_some(),
        Message::Transfer(TransferMessage::ReceivePinToggled),
    ));
    if let Some(input) = &browser.transfer_pin_input {
        rows.push(pin_row(browser, input));
    }
    rows.push(port_row(browser));
    rows.push(receive_directory_row(browser));
    rows.push(device_alias_row(browser));
    rows
}

/// 红字错误行：与批量重命名的错误状态同一取色。
fn error_line(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 接收确认三档；"所有人自动收"时一直红字提醒（同官方 Quick Save 的提示）。
fn accept_mode_row(browser: &FileBrowser) -> Element<'_, Message> {
    let current = browser.user_config().transfer_accept_mode;
    let choices = TransferAcceptMode::ALL
        .into_iter()
        .map(|mode| SegmentedChoice {
            label: match mode {
                TransferAcceptMode::AskEveryone => tr!("Always ask"),
                TransferAcceptMode::AcceptFavorites => tr!("Accept from favorites"),
                TransferAcceptMode::AcceptEveryone => tr!("Accept from everyone"),
            },
            selected: mode == current,
            message: Some(Message::Transfer(TransferMessage::AcceptModeSelected(mode))),
            tooltip: None,
        })
        .collect();
    let mut content = column![
        readable_text(tr!("Confirm incoming files")),
        segmented_choice_row(choices),
    ]
    .spacing(6);
    if current == TransferAcceptMode::AcceptEveryone {
        content = content.push(error_line(tr!(
            "Anyone on your local network can send you files without asking."
        )));
    }
    content.into()
}

/// PIN 行：明文输入（要念给别人听，同官方）+ 保存；保存了非空的才生效。
fn pin_row<'a>(browser: &'a FileBrowser, input: &'a str) -> Element<'a, Message> {
    let field = text_field(&tr!("PIN"), input)
        .on_input(|value| Message::Transfer(TransferMessage::ReceivePinInputChanged(value)))
        .on_submit(Message::Transfer(TransferMessage::ReceivePinCommitted))
        .width(Length::Fill);
    let save = normal_button(readable_text(tr!("Save")))
        .on_press(Message::Transfer(TransferMessage::ReceivePinCommitted));
    let mut content = column![
        row![readable_text(tr!("PIN")), field, save]
            .spacing(8)
            .align_y(Alignment::Center),
        readable_text(tr!(
            "Other devices must enter this PIN to send files to this computer, favorites included."
        ))
        .size(CAPTION_SIZE),
    ]
    .spacing(3);
    if browser.transfer_pin_invalid {
        content = content.push(error_line(tr!("The PIN cannot be empty.")));
    } else if browser.user_config().transfer_receive_pin.as_deref() != Some(input) {
        content =
            content.push(readable_text(tr!("Takes effect after you save.")).size(CAPTION_SIZE));
    }
    content.into()
}

/// 端口行：输入 + Save；下方常驻"两端端口要一致"的提示，上次提交不合法时再加红字。
fn port_row(browser: &FileBrowser) -> Element<'_, Message> {
    let input = text_field(
        &local_send::DEFAULT_PORT.to_string(),
        &browser.transfer_port_input,
    )
    .on_input(|value| Message::Transfer(TransferMessage::PortInputChanged(value)))
    .on_submit(Message::Transfer(TransferMessage::PortCommitted))
    .width(Length::Fill);
    let save = normal_button(readable_text(tr!("Save")))
        .on_press(Message::Transfer(TransferMessage::PortCommitted));
    let mut content = column![
        row![readable_text(tr!("Port")).width(Length::Fill), input, save,]
            .spacing(8)
            .align_y(Alignment::Center),
        readable_text(tr!(
            "If you change the port, other devices must use the same port to find each other."
        ))
        .size(CAPTION_SIZE),
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
    let choose = normal_button(
        row![
            themed_icon(IconSymbol::Folder, IconTone::Normal, 14.0),
            readable_text(tr!("Choose...")),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Message::Transfer(
        TransferMessage::DownloadDirChooserPressed,
    ));
    column![
        row![
            readable_text(tr!("Receive directory")).width(Length::Fill),
            readable_text(current),
            choose,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        readable_text(tr!(
            "Files received from other devices are saved to this directory"
        ))
        .size(CAPTION_SIZE),
    ]
    .spacing(3)
    .into()
}

/// 设备名行：输入 + Save（提交后重启常驻服务生效）。
fn device_alias_row(browser: &FileBrowser) -> Element<'_, Message> {
    let input = text_field(&tr!("Device name"), &browser.transfer_device_alias_input)
        .on_input(|value| Message::Transfer(TransferMessage::DeviceAliasInputChanged(value)))
        .on_submit(Message::Transfer(TransferMessage::DeviceAliasCommitted))
        .width(Length::Fill);
    let save = normal_button(readable_text(tr!("Save")))
        .on_press(Message::Transfer(TransferMessage::DeviceAliasCommitted));
    row![
        readable_text(tr!("Device name")).width(Length::Fill),
        input,
        save,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// 收藏设备组：每行 名字输入框（回车保存，清空回车 = 恢复对方的名字）+ 地址 + 删除。
pub(super) fn favorite_devices_section(browser: &FileBrowser) -> Element<'_, Message> {
    let favorites = &browser.user_config().transfer_favorite_devices;
    if favorites.is_empty() {
        return column![
            readable_text(tr!("No favorite devices yet")),
            readable_text(tr!(
                "In the LocalSend window, click the star next to a device to add it. Files from favorite devices are accepted automatically."
            ))
            .size(CAPTION_SIZE),
        ]
        .spacing(3)
        .into();
    }
    let mut rows = Vec::with_capacity(favorites.len() + 1);
    for device in favorites {
        let fingerprint = device.fingerprint.clone();
        // 名字是对方自报或用户起的，不做本地化。
        let value = browser
            .favorite_alias_inputs
            .get(&device.fingerprint)
            .unwrap_or(&device.alias);
        let name = text_field(&tr!("Device name"), value)
            .on_input({
                let fingerprint = fingerprint.clone();
                move |value| {
                    Message::Transfer(TransferMessage::FavoriteAliasInputChanged {
                        fingerprint: fingerprint.clone(),
                        value,
                    })
                }
            })
            .on_submit(Message::Transfer(TransferMessage::FavoriteAliasCommitted(
                fingerprint.clone(),
            )))
            .width(Length::Fill);
        let address = match device.address {
            Some(address) => address.ip().to_string(),
            None => tr!("Not found yet"),
        };
        let remove = normal_button(readable_text(tr!("Delete")).size(CAPTION_SIZE)).on_press(
            Message::Transfer(TransferMessage::UnfavoritePressed(fingerprint)),
        );
        rows.push(
            row![name, readable_text(address), remove]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    rows.push(
        readable_text(tr!(
            "Press Enter to save a name. Clear it and press Enter to use the device's own name again. Files from favorite devices are accepted automatically."
        ))
        .size(CAPTION_SIZE)
        .into(),
    );
    column(rows).spacing(6).width(Length::Fill).into()
}
