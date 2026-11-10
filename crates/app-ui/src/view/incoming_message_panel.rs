//! 收到的文字：主窗口中间的弹窗，一次一条（队首）；后面还有就显示"还有 N 条"。

use iced::widget::{column, container, row, scrollable, Space};
use iced::{Alignment, Element, Length};

use crate::app::transfer::{message_link, IncomingMessage, MessageTitle, TransferMessage};
use crate::appearance::context_menu_style;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;

use super::floating_panels::DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH;
use super::option_controls::{primary_action_button, secondary_action_button};
use super::settings_group::muted_setting_text;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const MESSAGE_MAX_HEIGHT: f32 = 240.0;

pub(super) fn incoming_message_panel(
    message: &IncomingMessage,
    remaining: usize,
) -> Element<'_, Message> {
    let action = |message: TransferMessage| Message::Transfer(message);
    let copy_label = if message.copied {
        tr!("Copied")
    } else {
        tr!("Copy")
    };
    let mut actions = row![
        Space::new().width(Length::Fill),
        secondary_action_button(copy_label, action(TransferMessage::IncomingMessageCopied)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if message_link(&message.text).is_some() {
        actions = actions.push(secondary_action_button(
            tr!("Open link"),
            action(TransferMessage::IncomingMessageLinkOpened),
        ));
    }
    actions = actions.push(primary_action_button(
        tr!("Close"),
        action(TransferMessage::IncomingMessageClosed),
    ));

    // 文字是对方发来的用户内容，不做本地化；放进可滚动的区域，长文不撑破弹窗。
    let body = scrollable(
        readable_text(message.text.clone())
            .size(13)
            .width(Length::Fill),
    )
    .height(Length::Shrink);
    let mut content = column![
        row![
            themed_icon(IconSymbol::Send, IconTone::Normal, MENU_ICON_SIZE),
            readable_text(match &message.title {
                MessageTitle::Received(alias) => tr!("%1 sent a message", alias),
                MessageTitle::Sent(alias) => tr!("Message to %1", alias),
            })
            .size(16)
            .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        container(body)
            .max_height(MESSAGE_MAX_HEIGHT)
            .width(Length::Fill),
    ]
    .spacing(12)
    .width(Length::Fill);
    if remaining > 0 {
        content = content.push(muted_setting_text(tr!("%1 more", remaining), 11));
    }
    container(content.push(actions))
        .padding(14)
        .width(Length::Fixed(DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}
