//! LocalSend 窗口里点“按 IP 添加”后展开的表单：名字（可不填）、IP、端口。
//!
//! 连接中输入框不可改、"添加"变"正在连接…"：结果要对上用户看到的那个地址。

use iced::widget::{column, row, Space, TextInput};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{AddressFormMessage, FavoriteAddressForm, TransferMessage};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::normal_button;
use bennu_theme::controls::text_field;
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

const LABEL_WIDTH: f32 = 40.0;
const PORT_INPUT_WIDTH: f32 = 72.0;

fn form_message(message: AddressFormMessage) -> Message {
    Message::Transfer(TransferMessage::AddressForm(message))
}

/// 回车即提交；连接中不可改。
fn form_input<'a>(
    placeholder: &str,
    value: &'a str,
    editable: bool,
    on_input: fn(String) -> AddressFormMessage,
) -> TextInput<'a, Message> {
    text_field(placeholder, value)
        .on_input_maybe(editable.then_some(move |value| form_message(on_input(value))))
        .on_submit_maybe(editable.then(|| form_message(AddressFormMessage::Submitted)))
}

pub(super) fn address_form(form: &FavoriteAddressForm) -> Element<'_, Message> {
    let editable = form.connecting.is_none();
    let label = |text: String| readable_text(text).width(Length::Fixed(LABEL_WIDTH));
    let mut content = column![
        row![
            label(tr!("Name")),
            form_input(
                &tr!("Optional. Uses the device's own name."),
                &form.name,
                editable,
                AddressFormMessage::NameChanged
            )
            .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            label(tr!("IP")),
            form_input(
                "192.168.1.20",
                &form.ip,
                editable,
                AddressFormMessage::IpChanged
            )
            .width(Length::Fill),
            readable_text(tr!("Port")),
            form_input(
                &local_send::DEFAULT_PORT.to_string(),
                &form.port,
                editable,
                AddressFormMessage::PortChanged
            )
            .width(Length::Fixed(PORT_INPUT_WIDTH)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(8);
    if let Some(error) = &form.error {
        content = content.push(
            readable_text(error.clone())
                .size(CAPTION_SIZE)
                .width(Length::Fill)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).error),
                }),
        );
    }
    let action = |label: String, message: Option<AddressFormMessage>| {
        normal_button(readable_text(label)).on_press_maybe(message.map(form_message))
    };
    let add = if editable {
        action(tr!("Add"), Some(AddressFormMessage::Submitted))
    } else {
        action(tr!("Connecting..."), None)
    };
    content = content.push(
        row![
            Space::new().width(Length::Fill),
            action(tr!("Cancel"), Some(AddressFormMessage::Canceled)),
            add,
        ]
        .spacing(8),
    );
    content.width(Length::Fill).into()
}
