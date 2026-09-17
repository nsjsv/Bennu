use iced::widget::{button, column, container, row, text_input, Button, Space};
use iced::{Alignment, Element, Length};

use crate::appearance::{
    context_menu_button_style, context_menu_item_button_style, context_menu_style,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::{ContextMenuPreferences, Message};
use crate::network_connections::{
    NetworkConnectionEditorMode, NetworkConnectionEditorState, NetworkConnectionMessage,
    SidebarNetworkConnectionAction, SidebarNetworkConnectionContextMenuState,
};
use crate::typography::readable_text;

use super::option_controls::{
    primary_action_button, secondary_action_button, segmented_choice_row, SegmentedChoice,
};
use super::toggle_switch::switch_control;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const NETWORK_EDITOR_PANEL_WIDTH: f32 = 460.0;
const NETWORK_EDITOR_ERROR_MAX_CHARS: usize = 96;
const NETWORK_CONTEXT_MENU_WIDTH: f32 = 190.0;
const NETWORK_CONTEXT_MENU_PADDING: f32 = 8.0;
const NETWORK_CONTEXT_MENU_ITEM_SPACING: f32 = 4.0;
const NETWORK_CONTEXT_MENU_ITEM_HEIGHT: f32 = 28.0;

pub(super) fn network_connection_editor_panel(
    editor: &NetworkConnectionEditorState,
) -> Element<'_, Message> {
    let title = editor_title(editor.mode);
    let title_row = row![
        themed_icon(IconSymbol::Link, IconTone::Normal, MENU_ICON_SIZE),
        readable_text(title).size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut label_input = text_input(
        &crate::localization::translate_current("Name"),
        &editor.label,
    )
    .padding([6, 8])
    .size(14);
    let mut uri_input = text_input(&crate::localization::translate_current("URI"), &editor.uri)
        .on_submit(Message::NetworkConnection(
            NetworkConnectionMessage::EditorSaved,
        ))
        .padding([6, 8])
        .size(14);
    if editor.mode != NetworkConnectionEditorMode::Connect {
        label_input = label_input.on_input(|value| {
            Message::NetworkConnection(NetworkConnectionMessage::EditorLabelChanged(value))
        });
        uri_input = uri_input.on_input(|value| {
            Message::NetworkConnection(NetworkConnectionMessage::EditorUriChanged(value))
        });
    }
    let mut content = column![title_row].spacing(10).width(Length::Fill);
    if editor.mode != NetworkConnectionEditorMode::Connect {
        content = content.push(protocol_selector(editor.protocol));
    }
    content = content
        .push(label_input)
        .push(uri_input)
        .push(
            text_input(
                &crate::localization::translate_current("Username"),
                &editor.username,
            )
            .on_input(|value| {
                Message::NetworkConnection(NetworkConnectionMessage::EditorUsernameChanged(value))
            })
            .on_submit(Message::NetworkConnection(
                NetworkConnectionMessage::EditorSaved,
            ))
            .padding([6, 8])
            .size(14),
        )
        .push(
            text_input(
                &crate::localization::translate_current("Password"),
                &editor.password,
            )
            .secure(true)
            .on_input(|value| {
                Message::NetworkConnection(NetworkConnectionMessage::EditorPasswordChanged(value))
            })
            .on_submit(Message::NetworkConnection(
                NetworkConnectionMessage::EditorSaved,
            ))
            .padding([6, 8])
            .size(14),
        );

    if editor.mode != NetworkConnectionEditorMode::Connect {
        content = content.push(editor_auto_connect_button(editor.auto_connect));
    }

    if let Some(error) = &editor.error {
        let error = crate::localization::translate_current(error);
        content = content.push(
            readable_text(format_middle_ellipsized_text(
                &error,
                NETWORK_EDITOR_ERROR_MAX_CHARS,
            ))
            .size(12),
        );
    }

    content = content.push(
        row![
            Space::new().width(Length::Fill),
            secondary_action_button(
                "Cancel",
                Message::NetworkConnection(NetworkConnectionMessage::EditorCanceled),
            ),
            primary_action_button(
                editor_primary_action_label(editor),
                Message::NetworkConnection(NetworkConnectionMessage::EditorSaved),
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    );

    container(content)
        .padding(14)
        .width(Length::Fixed(NETWORK_EDITOR_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn editor_auto_connect_button(is_on: bool) -> Button<'static, Message> {
    let content = row![
        readable_text("Connect on startup")
            .size(12)
            .width(Length::Fill),
        switch_control(is_on),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    button(container(content).padding([5, 8]).width(Length::Fill))
        .on_press(Message::NetworkConnection(
            NetworkConnectionMessage::EditorAutoConnectToggled(!is_on),
        ))
        .width(Length::Fill)
        .style(context_menu_button_style())
}

fn editor_title(mode: NetworkConnectionEditorMode) -> &'static str {
    match mode {
        NetworkConnectionEditorMode::Add => "Add Network Connection",
        NetworkConnectionEditorMode::Edit => "Edit Network Connection",
        NetworkConnectionEditorMode::Connect => "Connect Network Location",
    }
}

fn editor_primary_action_label(editor: &NetworkConnectionEditorState) -> &'static str {
    match editor.mode {
        NetworkConnectionEditorMode::Connect => "Connect",
        NetworkConnectionEditorMode::Add | NetworkConnectionEditorMode::Edit
            if editor.password_is_filled() =>
        {
            "Save & Connect"
        }
        NetworkConnectionEditorMode::Add | NetworkConnectionEditorMode::Edit => "Save",
    }
}

fn protocol_selector(selected: desktop_linux::NetworkProtocol) -> Element<'static, Message> {
    segmented_choice_row(vec![
        SegmentedChoice {
            label: "SMB",
            selected: selected == desktop_linux::NetworkProtocol::Smb,
            message: Message::NetworkConnection(NetworkConnectionMessage::EditorProtocolSelected(
                desktop_linux::NetworkProtocol::Smb,
            )),
            tooltip: None,
        },
        SegmentedChoice {
            label: "WebDAV",
            selected: selected == desktop_linux::NetworkProtocol::WebDav,
            message: Message::NetworkConnection(NetworkConnectionMessage::EditorProtocolSelected(
                desktop_linux::NetworkProtocol::WebDav,
            )),
            tooltip: None,
        },
        SegmentedChoice {
            label: "SFTP",
            selected: selected == desktop_linux::NetworkProtocol::Sftp,
            message: Message::NetworkConnection(NetworkConnectionMessage::EditorProtocolSelected(
                desktop_linux::NetworkProtocol::Sftp,
            )),
            tooltip: None,
        },
    ])
}

pub(super) fn network_connection_context_menu_panel<'a>(
    menu: &'a SidebarNetworkConnectionContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut menu_content = iced::widget::Column::new()
        .spacing(NETWORK_CONTEXT_MENU_ITEM_SPACING)
        .padding(NETWORK_CONTEXT_MENU_PADDING);
    for action in context_menus.network_connection_items(menu.connection.available_actions()) {
        menu_content = menu_content.push(network_connection_menu_button(
            action,
            Message::NetworkConnection(NetworkConnectionMessage::ActionSelected(
                menu.connection.id().clone(),
                action,
            )),
        ));
    }

    container(menu_content)
        .width(Length::Fixed(NETWORK_CONTEXT_MENU_WIDTH))
        .style(context_menu_style)
        .into()
}

fn network_connection_menu_button(
    action: SidebarNetworkConnectionAction,
    message: Message,
) -> Button<'static, Message> {
    let icon = match action {
        SidebarNetworkConnectionAction::Connect => IconSymbol::Link,
        SidebarNetworkConnectionAction::Disconnect => IconSymbol::Close,
        SidebarNetworkConnectionAction::Edit => IconSymbol::Pencil,
        SidebarNetworkConnectionAction::Remove => IconSymbol::Trash,
    };
    button(network_connection_menu_label(icon, action.label()))
        .on_press(message)
        .width(Length::Fill)
        .height(Length::Fixed(NETWORK_CONTEXT_MENU_ITEM_HEIGHT))
        .style(context_menu_item_button_style())
}

fn network_connection_menu_label(
    icon: IconSymbol,
    label: &'static str,
) -> iced::widget::Row<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE),
        crate::typography::readable_text(label)
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
}
