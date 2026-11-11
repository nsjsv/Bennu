//! 设置 → 传输：接收开关与原因、接收确认、PIN、端口、接收目录、设备名、收藏设备。

use iced::widget::tooltip;
use iced::Element;

use crate::app::transfer::{receiver_failure_message, TransferMessage};
use crate::app::FileBrowser;
use crate::config::{TransferAcceptMode, TRANSFER_PORT_RANGE};
use crate::model::Message;
use bennu_theme::controls::{icon_button, tooltip_bubble, SegmentedChoice};
use bennu_theme::icons::IconSymbol;

use super::settings_group::{
    choice_row, entry_row, label_row, notice_row, settings_group, switch_row, EntryRow, RowNote,
    SettingsRow,
};
use super::{themed_icon, IconTone};

const ROW_ICON_SIZE: f32 = 14.0;

/// "接收"组的行：开关 → 没开起来的原因（有才显示）→ 接收确认 → PIN → 端口 → 接收目录
/// → 设备名。
pub(super) fn transfer_receive_rows(browser: &FileBrowser) -> Vec<SettingsRow<'_>> {
    let mut rows = vec![switch_row(
        tr!("Receive files"),
        Some(RowNote::Hint(tr!(
            "When off, other devices cannot find this computer. You can still send files."
        ))),
        browser.user_config().transfer_receive_enabled,
        Message::Transfer(TransferMessage::ReceiveEnabledToggled),
    )];
    if let Some(failure) = browser.transfer_receiver_failure() {
        rows.push(notice_row(RowNote::Error(receiver_failure_message(
            failure,
        ))));
    }
    rows.push(accept_mode_row(browser));
    rows.push(switch_row(
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

/// 接收确认三档；"所有人自动收"时附注一直是红字提醒：同一网络里谁都能直接往这台电脑塞文件。
fn accept_mode_row(browser: &FileBrowser) -> SettingsRow<'static> {
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
    let note = (current == TransferAcceptMode::AcceptEveryone).then(|| {
        RowNote::Error(tr!(
            "Anyone on your local network can send you files without asking."
        ))
    });
    choice_row(tr!("Confirm incoming files"), note, choices)
}

/// PIN 行：明文输入（要念给发送方听）；保存了非空的才生效。
fn pin_row<'a>(browser: &'a FileBrowser, input: &'a str) -> SettingsRow<'a> {
    let note = if browser.transfer_pin_invalid {
        RowNote::Error(tr!("The PIN cannot be empty."))
    } else {
        RowNote::Hint(tr!(
            "Other devices must enter this PIN to send files to this computer, favorites included."
        ))
    };
    entry_row(EntryRow {
        title: tr!("PIN"),
        saved: browser
            .user_config()
            .transfer_receive_pin
            .clone()
            .unwrap_or_default(),
        draft: input,
        placeholder: tr!("PIN"),
        on_input: Box::new(|value| {
            Message::Transfer(TransferMessage::ReceivePinInputChanged(value))
        }),
        submit: Message::Transfer(TransferMessage::ReceivePinCommitted),
        note: Some(note),
        unit: None,
        trailing: Vec::new(),
    })
}

/// 端口行：附注常驻"两端端口要一致"，上次提交不合法时换成红字。
fn port_row(browser: &FileBrowser) -> SettingsRow<'_> {
    let note = if browser.transfer_port_invalid {
        RowNote::Error(tr!(
            "Port must be between %1 and %2.",
            TRANSFER_PORT_RANGE.start(),
            TRANSFER_PORT_RANGE.end()
        ))
    } else {
        RowNote::Hint(tr!(
            "If you change the port, other devices must use the same port to find each other."
        ))
    };
    entry_row(EntryRow {
        title: tr!("Port"),
        saved: browser.user_config().transfer_port.to_string(),
        draft: &browser.transfer_port_input,
        placeholder: local_send::DEFAULT_PORT.to_string(),
        on_input: Box::new(|value| Message::Transfer(TransferMessage::PortInputChanged(value))),
        submit: Message::Transfer(TransferMessage::PortCommitted),
        note: Some(note),
        unit: None,
        trailing: Vec::new(),
    })
}

/// 接收目录行：可以直接输入路径（空 = 默认 ~/Downloads，占位符显示默认路径），
/// 右边的图标按钮打开目录选择器。
fn receive_directory_row(browser: &FileBrowser) -> SettingsRow<'_> {
    let note = if browser.transfer_download_dir_invalid {
        RowNote::Error(tr!("Enter an absolute path."))
    } else {
        RowNote::Hint(tr!(
            "Files received from other devices are saved to this directory"
        ))
    };
    let choose: Element<'_, Message> = tooltip(
        icon_button(themed_icon(
            IconSymbol::FolderOpen,
            IconTone::Normal,
            ROW_ICON_SIZE,
        ))
        .on_press(Message::Transfer(
            TransferMessage::DownloadDirChooserPressed,
        )),
        tooltip_bubble(tr!("Choose directory")),
        tooltip::Position::Left,
    )
    .into();
    entry_row(EntryRow {
        title: tr!("Receive directory"),
        saved: browser.configured_transfer_download_dir(),
        draft: &browser.transfer_download_dir_input,
        placeholder: file_core::app_dirs::home_dir()
            .join("Downloads")
            .to_string_lossy()
            .into_owned(),
        on_input: Box::new(|value| {
            Message::Transfer(TransferMessage::DownloadDirInputChanged(value))
        }),
        submit: Message::Transfer(TransferMessage::DownloadDirCommitted),
        note: Some(note),
        unit: None,
        trailing: vec![choose],
    })
}

/// 设备名行：提交后重启常驻服务生效；清空保存 = 用这台电脑的主机名。
fn device_alias_row(browser: &FileBrowser) -> SettingsRow<'_> {
    entry_row(EntryRow {
        title: tr!("Device name"),
        saved: browser.effective_transfer_device_alias(),
        draft: &browser.transfer_device_alias_input,
        placeholder: tr!("Device name"),
        on_input: Box::new(|value| {
            Message::Transfer(TransferMessage::DeviceAliasInputChanged(value))
        }),
        submit: Message::Transfer(TransferMessage::DeviceAliasCommitted),
        note: None,
        unit: None,
        trailing: Vec::new(),
    })
}

/// 收藏设备组：每台一行，名字是输入行（附注是地址），右边删除。
/// 名字是对方自报或用户起的，不做本地化。
pub(super) fn favorite_devices_group(browser: &FileBrowser) -> Element<'_, Message> {
    let favorites = &browser.user_config().transfer_favorite_devices;
    if favorites.is_empty() {
        return settings_group(
            tr!("Favorite devices"),
            None,
            vec![label_row(
                tr!("No favorite devices yet"),
                Some(RowNote::Hint(tr!(
                    "In the LocalSend window, click the star next to a device to add it. Files from favorite devices are accepted automatically."
                ))),
            )],
        );
    }
    let rows = favorites
        .iter()
        .map(|device| {
            let fingerprint = device.fingerprint.clone();
            let address = match device.address {
                Some(address) => address.ip().to_string(),
                None => tr!("Not found yet"),
            };
            let remove: Element<'_, Message> = icon_button(themed_icon(
                IconSymbol::Trash,
                IconTone::Normal,
                ROW_ICON_SIZE,
            ))
            .on_press(Message::Transfer(TransferMessage::UnfavoritePressed(
                fingerprint.clone(),
            )))
            .into();
            entry_row(EntryRow {
                title: address,
                saved: device.alias.clone(),
                draft: browser
                    .favorite_alias_inputs
                    .get(&device.fingerprint)
                    .unwrap_or(&device.alias),
                placeholder: tr!("Device name"),
                on_input: Box::new({
                    let fingerprint = fingerprint.clone();
                    move |value| {
                        Message::Transfer(TransferMessage::FavoriteAliasInputChanged {
                            fingerprint: fingerprint.clone(),
                            value,
                        })
                    }
                }),
                submit: Message::Transfer(TransferMessage::FavoriteAliasCommitted(fingerprint)),
                note: None,
                unit: None,
                trailing: vec![remove],
            })
        })
        .collect();
    settings_group(
        tr!("Favorite devices"),
        Some(tr!(
            "Press Enter to save a name. Clear it and press Enter to use the device's own name again. Files from favorite devices are accepted automatically."
        )),
        rows,
    )
}
