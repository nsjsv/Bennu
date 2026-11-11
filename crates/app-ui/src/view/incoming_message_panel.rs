//! 收到的文字：主窗口中间的弹窗，一次一条（队首）；后面还有就显示"还有 N 条"。

use iced::widget::{container, scrollable};
use iced::{Element, Length};

use crate::app::transfer::{message_link, IncomingMessage, MessageTitle, TransferMessage};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;

use super::dialog::{self, Dialog};
use bennu_theme::controls::{normal_button, suggested_button};

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
    let mut actions: Vec<Element<'_, Message>> = vec![normal_button(readable_text(copy_label))
        .on_press(action(TransferMessage::IncomingMessageCopied))
        .into()];
    if message_link(&message.text).is_some() {
        actions.push(
            normal_button(readable_text(tr!("Open link")))
                .on_press(action(TransferMessage::IncomingMessageLinkOpened))
                .into(),
        );
    }
    actions.push(
        suggested_button(readable_text(tr!("Close")))
            .on_press(action(TransferMessage::IncomingMessageClosed))
            .into(),
    );

    // 文字是对方发来的用户内容，不做本地化；放进可滚动的区域，长文不撑破弹窗。
    let text =
        scrollable(readable_text(message.text.clone()).width(Length::Fill)).height(Length::Shrink);
    let mut body = dialog::fields(vec![container(text)
        .max_height(MESSAGE_MAX_HEIGHT)
        .width(Length::Fill)
        .into()]);
    if remaining > 0 {
        body = body.push(dialog::hint(tr!("%1 more", remaining)));
    }
    Dialog::new(
        match &message.title {
            MessageTitle::Received(alias) => tr!("%1 sent a message", alias),
            MessageTitle::Sent(alias) => tr!("Message to %1", alias),
        },
        body,
    )
    .actions(actions)
    .into()
}
