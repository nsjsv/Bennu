use iced::widget::{button, column, container, mouse_area, row, scrollable, Button, Column, Row};
use iced::{Alignment, Element, Length};

use desktop_linux::FileClipboardOperation;

use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{
    context_menu_item_button_style, context_menu_style, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, error_notification_style,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::formatting::item_count_label;
use crate::model::{
    ContextMenuPreferences, ContextMenuState, DestructiveActionConfirmation, FileDropPrompt,
    FileGroupingMode, FilePropertiesMessage, ListColumnConfig, ListColumnKind, ListViewPreferences,
    Message, ScrollbarRegion, SidebarBookmarkContextMenuState, TrashContextMenuState,
    TrashMenuItem,
};
use crate::open_with::OpenWithState;
use crate::sidebar_devices::SidebarDeviceContextMenuState;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};

use super::context_menu_submenu::{
    CONTEXT_MENU_ITEM_HEIGHT, CONTEXT_MENU_ITEM_SPACING, CONTEXT_MENU_PADDING,
};
use super::dialog::{self, Dialog};
use super::file_context_menu::file_context_menu_panel;
use super::network_connections::network_connection_context_menu_panel;
use super::option_controls::action_choice_row;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use bennu_theme::controls::check_box;
use bennu_theme::controls::icon_button_style;
use bennu_theme::controls::{destructive_button, normal_button};
use bennu_theme::typography::CAPTION_SIZE;

const ERROR_NOTIFICATION_FLOAT_WIDTH: f32 = 560.0;
const ERROR_NOTIFICATION_MAX_CHARS: usize = 96;
const OPEN_WITH_PANEL_WIDTH: f32 = 420.0;
const OPEN_WITH_APPLICATION_LIST_HEIGHT: f32 = 240.0;
const OPEN_WITH_PATH_MAX_CHARS: usize = 62;
const OPEN_WITH_ERROR_MAX_CHARS: usize = 96;
const LIST_COLUMN_MENU_WIDTH: f32 = 230.0;
const LIST_COLUMN_VISIBILITY_BUTTON_HEIGHT: f32 = 28.0;
pub(super) fn error_notification_panel(error: &str, generation: u64) -> Element<'_, Message> {
    let message = format_middle_ellipsized_text(error, ERROR_NOTIFICATION_MAX_CHARS);
    let close_button = button(themed_icon(IconSymbol::Close, IconTone::Normal, 13.0))
        .on_press(Message::GlobalErrorNotificationDismissed(generation))
        .padding(4)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(icon_button_style);
    let content = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text(message).width(Length::Fill),
        close_button,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    mouse_area(
        container(content)
            .padding([10, 12])
            .width(Length::Fixed(ERROR_NOTIFICATION_FLOAT_WIDTH))
            .style(error_notification_style),
    )
    .on_enter(Message::GlobalErrorNotificationPointerEntered(generation))
    .on_exit(Message::GlobalErrorNotificationPointerExited(generation))
    .on_press(Message::GlobalErrorNotificationDismissed(generation))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

pub(super) fn destructive_action_confirmation_panel(
    confirmation: &DestructiveActionConfirmation,
) -> Element<'_, Message> {
    let (title, body, confirm_label) = match confirmation {
        DestructiveActionConfirmation::DeleteTrashEntries { entries } => (
            tr!("Delete Permanently?"),
            tr!(
                "Delete %1 from Trash permanently? This cannot be undone.",
                item_count_label(entries.len())
            ),
            tr!("Delete Permanently"),
        ),
        DestructiveActionConfirmation::DeletePermanently { paths } => (
            tr!("Delete Permanently?"),
            tr!(
                "Delete %1 permanently? This cannot be undone.",
                item_count_label(paths.len())
            ),
            tr!("Delete Permanently"),
        ),
        DestructiveActionConfirmation::DeleteUntrashable { paths } => (
            tr!("Can't Move to Trash"),
            tr!(
                "%1 can't be moved to Trash here. Delete permanently? This cannot be undone.",
                item_count_label(paths.len())
            ),
            tr!("Delete Permanently"),
        ),
        DestructiveActionConfirmation::EmptyTrash => (
            tr!("Empty Trash?"),
            tr!("Delete all items in Trash permanently? This cannot be undone."),
            tr!("Empty Trash"),
        ),
    };

    // 删了就回不来：确认按钮用红色，不用强调色。
    Dialog::new(title, readable_text(body).width(Length::Fill))
        .actions(vec![
            normal_button(readable_text(tr!("Cancel")))
                .on_press(Message::DestructiveActionCanceled)
                .into(),
            destructive_button(readable_text(confirm_label))
                .on_press(Message::DestructiveActionConfirmed)
                .into(),
        ])
        .into()
}

pub(super) fn file_drop_operation_panel(prompt: &FileDropPrompt) -> Element<'_, Message> {
    let operation_choices = column![
        action_choice_row(
            tr!("Move"),
            tr!("Move files into the current folder."),
            Message::FileDropOperationSelected(FileClipboardOperation::Move),
        ),
        action_choice_row(
            tr!("Copy"),
            tr!("Copy files into the current folder."),
            Message::FileDropOperationSelected(FileClipboardOperation::Copy),
        ),
    ]
    .spacing(8);
    Dialog::new(tr!("Drop Files"), operation_choices)
        .subtitle(dialog::into_subtitle(
            &item_count_label(prompt.paths.len()),
            &prompt.paste_directory,
        ))
        .actions(vec![normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::FileDropCancelled)
            .into()])
        .into()
}

pub(super) fn open_with_panel(state: &OpenWithState) -> Element<'static, Message> {
    let mut subtitle =
        format_middle_ellipsized_text(&dialog::path_name(state.path()), OPEN_WITH_PATH_MAX_CHARS);
    if let Some(mime_type) = state.mime_type() {
        subtitle = format!("{subtitle} · {mime_type}");
    }
    let mut body = dialog::fields(Vec::new());
    if let Some(fallback_error) = state.fallback_error() {
        body = body.push(dialog::error(tr!(
            "Default open failed: %1",
            format_middle_ellipsized_text(fallback_error, OPEN_WITH_ERROR_MAX_CHARS)
        )));
    }
    match state {
        OpenWithState::Loading { .. } => {
            body = body.push(dialog::hint(tr!("Loading applications...")));
        }
        OpenWithState::Ready { .. } => {
            body = body.push(open_with_application_list(state)).push(
                check_box(state.set_default_selected())
                    .label(tr!("Set as default application"))
                    .on_toggle(Message::OpenWithDefaultApplicationToggled),
            );
        }
    }
    Dialog::new(tr!("Open With"), body)
        .subtitle(subtitle)
        .actions(vec![normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::DismissFloating)
            .into()])
        .width(OPEN_WITH_PANEL_WIDTH)
        .into()
}

fn open_with_application_list(state: &OpenWithState) -> Element<'static, Message> {
    let mut applications = Column::new().spacing(4);
    for application in state.applications() {
        let label = if application.is_default {
            tr!("%1 (default)", application.name)
        } else {
            application.name.clone()
        };
        applications = applications.push(
            button(
                row![
                    themed_icon(IconSymbol::Monitor, IconTone::Normal, MENU_ICON_SIZE),
                    column![
                        readable_text(label).width(Length::Fill),
                        readable_text(application.desktop_id.clone()).size(CAPTION_SIZE),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .on_press(Message::OpenWithApplicationSelected(
                application.desktop_id.clone(),
            ))
            .padding([7, 8])
            .width(Length::Fill)
            .style(context_menu_item_button_style()),
        );
    }

    let scroll_region = ScrollbarRegion::OpenWithApplications;
    let scroller = scrollable(smooth_scroll_content(applications, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction())
        .style(enhanced_scrollbar_style)
        .height(Length::Fixed(OPEN_WITH_APPLICATION_LIST_HEIGHT))
        .width(Length::Fill);

    enhanced_scrollbar(scroller, ScrollbarAxis::Vertical)
}

fn action_label(icon: IconSymbol, label: String, size: f32) -> Row<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, size),
        readable_text(label)
    ]
    .spacing(6)
    .align_y(Alignment::Center)
}

/// 右键菜单浮层内容:主菜单面板 + 悬停展开的子菜单(仅文件区菜单)。
/// 子菜单的纵向偏移 top 相对父浮层顶沿,由 floating_surface 独立定位与
/// 钳制,展开与否不影响主菜单位置。
pub(super) struct ContextMenuPanels<'a> {
    pub(super) root: Element<'a, Message>,
    pub(super) submenu: Option<(Element<'a, Message>, f32)>,
}

pub(super) fn context_menu_panel<'a>(
    menu: &'a ContextMenuState,
    keyboard_modifiers: iced::keyboard::Modifiers,
    context_menus: &'a ContextMenuPreferences,
    list_view_preferences: &'a ListViewPreferences,
    file_grouping: FileGroupingMode,
) -> ContextMenuPanels<'a> {
    match menu {
        ContextMenuState::Trash(menu) => ContextMenuPanels {
            root: trash_context_menu_panel(menu, context_menus),
            submenu: None,
        },
        ContextMenuState::FileArea(menu) => {
            let (root, submenu) =
                file_context_menu_panel(menu, context_menus, keyboard_modifiers, file_grouping);
            ContextMenuPanels { root, submenu }
        }
        ContextMenuState::ListColumns(menu) => ContextMenuPanels {
            root: list_column_context_menu_panel(&menu.columns, list_view_preferences),
            submenu: None,
        },
        ContextMenuState::SidebarBookmark(menu) => ContextMenuPanels {
            root: sidebar_bookmark_context_menu_panel(menu, context_menus),
            submenu: None,
        },
        ContextMenuState::SidebarDevice(menu) => ContextMenuPanels {
            root: sidebar_device_context_menu_panel(menu, context_menus),
            submenu: None,
        },
        ContextMenuState::NetworkConnection(menu) => ContextMenuPanels {
            root: network_connection_context_menu_panel(menu, context_menus),
            submenu: None,
        },
    }
}

fn list_column_context_menu_panel<'a>(
    columns: &[ListColumnKind],
    preferences: &'a ListViewPreferences,
) -> Element<'a, Message> {
    let mut menu_content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    for kind in columns {
        if let Some(column) = preferences
            .columns()
            .iter()
            .find(|column| column.kind == *kind)
        {
            menu_content = menu_content.push(list_column_menu_row(column));
        }
    }

    container(menu_content)
        .width(Length::Fixed(LIST_COLUMN_MENU_WIDTH))
        .style(context_menu_style)
        .into()
}

fn list_column_menu_row(column: &ListColumnConfig) -> Element<'static, Message> {
    row![
        themed_icon(IconSymbol::List, IconTone::Normal, MENU_ICON_SIZE),
        readable_text(column.kind.label()).width(Length::Fill),
        list_column_visibility_button(column),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
    .into()
}

fn list_column_visibility_button(column: &ListColumnConfig) -> Button<'static, Message> {
    let label = if column.kind == ListColumnKind::Name {
        tr!("Required")
    } else if column.visible {
        tr!("Hide")
    } else {
        tr!("Show")
    };
    let button = button(
        container(readable_text(label))
            .center_x(Length::Fixed(62.0))
            .center_y(Length::Fixed(LIST_COLUMN_VISIBILITY_BUTTON_HEIGHT)),
    )
    .width(Length::Fixed(62.0))
    .height(Length::Fixed(LIST_COLUMN_VISIBILITY_BUTTON_HEIGHT))
    .style(context_menu_item_button_style());

    if column.kind == ListColumnKind::Name {
        button
    } else {
        button.on_press(Message::ListColumnVisibilityToggled(column.kind))
    }
}

fn sidebar_bookmark_context_menu_panel<'a>(
    menu: &'a SidebarBookmarkContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut menu_content = iced::widget::Column::new().spacing(4).padding(8);
    for item in context_menus.sidebar_bookmark_items() {
        menu_content = menu_content.push(menu_button(
            IconSymbol::Trash,
            item.label(),
            Message::SidebarBookmarkDeleteRequested(menu.path.clone()),
        ));
    }

    container(menu_content)
        .width(Length::Fixed(190.0))
        .style(context_menu_style)
        .into()
}

fn sidebar_device_context_menu_panel<'a>(
    menu: &'a SidebarDeviceContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut menu_content = iced::widget::Column::new().spacing(4).padding(8);
    let actions = menu.device.available_actions();
    if actions.is_empty() {
        menu_content = menu_content.push(readable_text(tr!("No device actions available")));
    } else {
        for action in context_menus.sidebar_device_items(actions) {
            menu_content = menu_content.push(menu_button(
                IconSymbol::HardDrive,
                action.label(&menu.device),
                Message::SidebarDeviceActionSelected(menu.device.id.clone(), action),
            ));
        }
    }

    container(menu_content)
        .width(Length::Fixed(190.0))
        .style(context_menu_style)
        .into()
}

fn trash_context_menu_panel<'a>(
    menu: &'a TrashContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut menu_content = iced::widget::Column::new().spacing(4).padding(8);
    for item in context_menus.trash_items(menu.target.is_some()) {
        match item {
            TrashMenuItem::Restore => {
                menu_content = menu_content.push(menu_button(
                    IconSymbol::ArrowLeft,
                    item.label(),
                    Message::RestoreSelected,
                ))
            }
            TrashMenuItem::DeletePermanently => {
                menu_content = menu_content.push(menu_button(
                    IconSymbol::Trash,
                    item.label(),
                    Message::TrashSelected,
                ))
            }
            TrashMenuItem::Properties => {
                // eligibility 已保证 Properties 仅在 has_target 时出现。
                if let Some(path) = &menu.target {
                    menu_content = menu_content.push(menu_button(
                        IconSymbol::FileText,
                        item.label(),
                        Message::FileProperties(FilePropertiesMessage::Requested(path.clone())),
                    ));
                }
            }
            TrashMenuItem::EmptyTrash => {
                menu_content = menu_content.push(menu_button(
                    IconSymbol::Trash,
                    item.label(),
                    Message::EmptyTrashRequested,
                ))
            }
        }
    }

    container(menu_content)
        .width(Length::Fixed(190.0))
        .style(context_menu_style)
        .into()
}

fn menu_button(icon: IconSymbol, label: String, message: Message) -> Button<'static, Message> {
    button(menu_label(icon, label))
        .on_press(message)
        .width(Length::Fill)
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .style(context_menu_item_button_style())
}

fn menu_label(icon: IconSymbol, label: String) -> Row<'static, Message> {
    action_label(icon, label, MENU_ICON_SIZE).width(Length::Fill)
}
