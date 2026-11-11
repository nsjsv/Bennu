//! 接收确认 Modal：不是收藏的设备发来文件时弹出（主窗口同层）。

use iced::widget::{column, container, row, scrollable, Space};
use iced::{Alignment, Element, Length};

use crate::app::transfer::{
    incoming_entries, IncomingEntry, IncomingTransferConfirmation, TransferMessage,
};
use crate::appearance::context_menu_style;
use crate::model::Message;
use bennu_localization::format_file_size;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::floating_panels::DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::check_box;
use bennu_theme::controls::{normal_button, suggested_button};
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};

/// 接收确认 Modal：设备名 + 文件清单 + 加入收藏 + 接受/拒绝（R7）。
pub(super) fn incoming_transfer_confirmation_panel(
    incoming: &IncomingTransferConfirmation,
) -> Element<'_, Message> {
    let summary = tr!(
        "%1 wants to send %2 item(s)",
        incoming.device.alias,
        incoming.files.len()
    );
    let mut file_list = column![].spacing(4).width(Length::Fill);
    for entry in incoming_entries(&incoming.files) {
        let (icon, name, size) = match entry {
            IncomingEntry::Folder { name, files, bytes } => (
                IconSymbol::Folder,
                name,
                tr!("%1 file(s) · %2", files, format_file_size(bytes)),
            ),
            IncomingEntry::File { name, bytes } => {
                (IconSymbol::File, name, format_file_size(bytes))
            }
        };
        file_list = file_list.push(
            row![
                themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE),
                readable_text(name).width(Length::Fill),
                readable_text(size).size(CAPTION_SIZE),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    // 明文设备的身份是自报的，收藏不了。
    let add_to_favorites = incoming.device.is_certified().then(|| {
        check_box(incoming.add_to_favorites)
            .label(tr!("Add to favorites"))
            .on_toggle(|add| Message::Transfer(TransferMessage::IncomingAddToFavoritesToggled(add)))
    });
    let actions = row![
        Space::new().width(Length::Fill),
        normal_button(readable_text(tr!("Decline"))).on_press(Message::Transfer(
            TransferMessage::IncomingConfirmed { accept: false }
        )),
        suggested_button(readable_text(tr!("Accept"))).on_press(Message::Transfer(
            TransferMessage::IncomingConfirmed { accept: true }
        )),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let content = column![
        row![
            themed_icon(IconSymbol::Download, IconTone::Normal, MENU_ICON_SIZE),
            readable_text(tr!("Incoming files"))
                .size(TITLE_SIZE)
                .font(BOLD)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        readable_text(summary),
        container(scrollable(file_list))
            .height(Length::Fixed(132.0))
            .width(Length::Fill),
    ]
    .push(add_to_favorites)
    .push(actions)
    .spacing(12)
    .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}
