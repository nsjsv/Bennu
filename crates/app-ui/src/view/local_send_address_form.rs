//! LocalSend 窗口收藏区里展开的"按 IP 添加"表单：名字（可不填）、IP、端口。
//!
//! 连接中输入框不可改、"添加"变"正在连接…"：结果要对上用户看到的那个地址。

use iced::widget::{button, column, container, row, text_input, Space, TextInput};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{AddressFormMessage, FavoriteAddressForm, TransferMessage};
use crate::appearance::context_menu_button_style;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
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
    text_input(placeholder, value)
        .on_input_maybe(editable.then_some(move |value| form_message(on_input(value))))
        .on_submit_maybe(editable.then(|| form_message(AddressFormMessage::Submitted)))
        .padding([5, 8])
        .size(12)
}

pub(super) fn address_form(form: &FavoriteAddressForm) -> Element<'_, Message> {
    let editable = form.connecting.is_none();
    let label = |text: String| {
        readable_text(text)
            .size(12)
            .width(Length::Fixed(LABEL_WIDTH))
    };
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
            readable_text(tr!("Port")).size(12),
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
                .size(11)
                .width(Length::Fill)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).error),
                }),
        );
    }
    let action = |label: String, message: Option<AddressFormMessage>| {
        button(readable_text(label).size(12))
            .on_press_maybe(message.map(form_message))
            .padding([5, 12])
            .style(context_menu_button_style())
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
    container(content)
        .padding(10)
        .width(Length::Fill)
        .style(form_style)
        .into()
}

fn form_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(iced::Background::Color(colors.surface_container_low)),
        border: iced::Border {
            color: crate::appearance::subtle_border_color(theme),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}
