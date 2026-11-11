//! 本机卡片里的"从手机接收"：按钮、准备中、二维码与"停止接收"、失败与超时的说明。

use iced::widget::column;
use iced::Element;

use crate::app::transfer::{LinkReceiveMessage, LinkReceivePhase, TransferMessage};
use crate::app::FileBrowser;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;

use super::local_send_window::error_text;
use super::settings_group::muted_setting_text;
use super::transfer_qr::qr_links_view;
use bennu_theme::controls::normal_button;

pub(super) fn link_receive_section(browser: &FileBrowser) -> Element<'_, Message> {
    let Some(current) = &browser.link_receiving.current else {
        return action_button(tr!("Receive from phone"), LinkReceiveMessage::StartPressed);
    };
    match &current.phase {
        LinkReceivePhase::Starting => muted_setting_text(tr!("Preparing the link..."), 11),
        LinkReceivePhase::Ready(links) => column![
            qr_links_view(
                links,
                |url| tr!(
                    "Scan the code or open the link on your phone to send files here: %1",
                    url
                ),
                |index| TransferMessage::LinkReceive(LinkReceiveMessage::AddressChosen(index)),
                TransferMessage::LinkReceive(LinkReceiveMessage::AddressListRequested),
            ),
            action_button(tr!("Stop receiving"), LinkReceiveMessage::StopPressed),
        ]
        .spacing(10)
        .into(),
        LinkReceivePhase::Failed(error) => column![
            error_text(tr!("Could not start receiving from a phone: %1", error)),
            action_button(tr!("Receive from phone"), LinkReceiveMessage::StartPressed),
        ]
        .spacing(8)
        .into(),
        LinkReceivePhase::TimedOut => column![
            muted_setting_text(
                tr!("No one used the link for 10 minutes, so receiving stopped."),
                11
            ),
            action_button(tr!("Receive from phone"), LinkReceiveMessage::StartPressed),
        ]
        .spacing(8)
        .into(),
    }
}

fn action_button(label: String, message: LinkReceiveMessage) -> Element<'static, Message> {
    normal_button(readable_text(label))
        .on_press(Message::Transfer(TransferMessage::LinkReceive(message)))
        .into()
}
