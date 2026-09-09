use iced::widget::{
    button, checkbox, column, container, mouse_area, row, scrollable, Button, Column, Row, Space,
};
use iced::{Alignment, Element, Length};

use desktop_linux::FileClipboardOperation;

use crate::app::archive_creation::ArchiveCreationMessage;
use crate::app::checksum::ChecksumMessage;
use crate::app::convert::ConvertMessage;
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{
    context_menu_item_button_style, context_menu_style, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, error_notification_style, navigation_icon_button_style,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::{
    BatchRenameMessage, BrowserPaneId, ContextMenuPreferences, ContextMenuState,
    DestructiveActionConfirmation, FileAreaMenuItem, FileContextMenuExpansion,
    FileContextMenuState, FileDropPrompt, FilePropertiesMessage, ListColumnConfig, ListColumnKind,
    ListViewPreferences, Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility,
    SearchContextMenuState, SearchResultMenuItem, SearchEntryTypePreset,
    SidebarBookmarkContextMenuState, TrashMenuItem,
};
use crate::open_with::OpenWithState;
use crate::sidebar_devices::SidebarDeviceContextMenuState;
use crate::typography::{localized_text, readable_text};

use super::network_connections::network_connection_context_menu_panel;
use super::option_controls::{action_choice_row, primary_action_button, secondary_action_button};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const ERROR_NOTIFICATION_FLOAT_WIDTH: f32 = 560.0;
const ERROR_NOTIFICATION_MAX_CHARS: usize = 96;
const DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH: f32 = 460.0;
const FILE_DROP_OPERATION_PANEL_WIDTH: f32 = 460.0;
const FILE_DROP_PATH_MAX_CHARS: usize = 62;
const OPEN_WITH_PANEL_WIDTH: f32 = 420.0;
const OPEN_WITH_APPLICATION_LIST_HEIGHT: f32 = 240.0;
const OPEN_WITH_PATH_MAX_CHARS: usize = 62;
const OPEN_WITH_ERROR_MAX_CHARS: usize = 96;
const CONTEXT_MENU_WIDTH: f32 = 190.0;
const LIST_COLUMN_MENU_WIDTH: f32 = 230.0;
const CONTEXT_SUBMENU_WIDTH: f32 = 170.0;
const CONTEXT_MENU_PADDING: f32 = 8.0;
const CONTEXT_MENU_ITEM_SPACING: f32 = 4.0;
const CONTEXT_MENU_ITEM_HEIGHT: f32 = 28.0;
const LIST_COLUMN_VISIBILITY_BUTTON_HEIGHT: f32 = 28.0;
pub(super) fn error_notification_panel(error: &str, generation: u64) -> Element<'_, Message> {
    let message = crate::localization::translate_current(error);
    let message = format_middle_ellipsized_text(&message, ERROR_NOTIFICATION_MAX_CHARS);
    let close_button = button(themed_icon(IconSymbol::Close, IconTone::Normal, 13.0))
        .on_press(Message::GlobalErrorNotificationDismissed(generation))
        .padding(4)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(navigation_icon_button_style());
    let content = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text(message).size(13).width(Length::Fill),
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
        DestructiveActionConfirmation::DeleteTrashEntries { entries } => {
            let item_count = entries.len();
            let item_label = if item_count == 1 {
                "1 item".to_owned()
            } else {
                format!("{item_count} items")
            };
            (
                "Delete Permanently?",
                format!("Delete {item_label} from Trash permanently? This cannot be undone."),
                "Delete Permanently",
            )
        }
        DestructiveActionConfirmation::DeletePermanently { paths } => {
            let item_count = paths.len();
            let item_label = if item_count == 1 {
                "1 item".to_owned()
            } else {
                format!("{item_count} items")
            };
            (
                "Delete Permanently?",
                format!("Delete {item_label} permanently? This cannot be undone."),
                "Delete Permanently",
            )
        }
        DestructiveActionConfirmation::EmptyTrash => (
            "Empty Trash?",
            "Delete all items in Trash permanently? This cannot be undone.".to_owned(),
            "Empty Trash",
        ),
    };

    let title_row = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text(title).size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let actions = row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::DestructiveActionCanceled),
        primary_action_button(confirm_label, Message::DestructiveActionConfirmed),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let content = column![title_row, localized_text(body).size(13), actions]
        .spacing(12)
        .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(DESTRUCTIVE_CONFIRMATION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

pub(super) fn file_drop_operation_panel(prompt: &FileDropPrompt) -> Element<'_, Message> {
    let item_count = prompt.paths.len();
    let item_label = if item_count == 1 {
        "1 item".to_owned()
    } else {
        format!("{item_count} items")
    };
    let destination = format_middle_ellipsized_text(
        prompt.paste_directory.to_string_lossy().as_ref(),
        FILE_DROP_PATH_MAX_CHARS,
    );

    let title = row![
        themed_icon(IconSymbol::ArrowRight, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("Drop Files").size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let operation_choices = column![
        action_choice_row(
            "Move",
            "Move files into the current folder.",
            Message::FileDropOperationSelected(FileClipboardOperation::Move),
        ),
        action_choice_row(
            "Copy",
            "Copy files into the current folder.",
            Message::FileDropOperationSelected(FileClipboardOperation::Copy),
        ),
    ]
    .spacing(8);

    let actions = row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::FileDropCancelled),
    ]
    .align_y(Alignment::Center);

    let content = column![
        title,
        localized_text(format!("{item_label} will be added to:")).size(13),
        readable_text(destination).size(12).width(Length::Fill),
        operation_choices,
        actions,
    ]
    .spacing(12)
    .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(FILE_DROP_OPERATION_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

pub(super) fn open_with_panel(
    state: &OpenWithState,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'static, Message> {
    let path_label = format_middle_ellipsized_text(
        state.path().to_string_lossy().as_ref(),
        OPEN_WITH_PATH_MAX_CHARS,
    );
    let title = row![
        themed_icon(IconSymbol::Monitor, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("Open With").size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut content = column![title, readable_text(path_label).size(12)]
        .spacing(10)
        .width(Length::Fill);

    if let Some(fallback_error) = state.fallback_error() {
        content = content.push(
            localized_text(format!(
                "Default open failed: {}",
                format_middle_ellipsized_text(fallback_error, OPEN_WITH_ERROR_MAX_CHARS)
            ))
            .size(12),
        );
    }

    if let Some(mime_type) = state.mime_type() {
        content = content.push(readable_text(mime_type.to_owned()).size(12));
    }

    match state {
        OpenWithState::Loading { .. } => {
            content = content.push(readable_text("Loading applications...").size(13));
        }
        OpenWithState::Ready { .. } => {
            content = content
                .push(open_with_application_list(
                    state,
                    scrollbar_visibility,
                    scrollbar_viewport,
                ))
                .push(
                    checkbox(state.set_default_selected())
                        .label(crate::localization::translate_current(
                            "Set as default application",
                        ))
                        .on_toggle(Message::OpenWithDefaultApplicationToggled),
                );
        }
    }

    content = content.push(
        row![
            Space::new().width(Length::Fill),
            secondary_action_button("Cancel", Message::DismissFloating),
        ]
        .align_y(Alignment::Center),
    );

    container(content)
        .padding(14)
        .width(Length::Fixed(OPEN_WITH_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn open_with_application_list(
    state: &OpenWithState,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'static, Message> {
    let mut applications = Column::new().spacing(4);
    for application in state.applications() {
        let label = if application.is_default {
            format!(
                "{} {}",
                application.name,
                crate::localization::translate_current("(default)")
            )
        } else {
            application.name.clone()
        };
        applications = applications.push(
            button(
                row![
                    themed_icon(IconSymbol::Monitor, IconTone::Normal, MENU_ICON_SIZE),
                    column![
                        readable_text(label).size(13).width(Length::Fill),
                        readable_text(application.desktop_id.clone()).size(11),
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
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(OPEN_WITH_APPLICATION_LIST_HEIGHT))
        .width(Length::Fill)
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |_| {
            Message::OpenWithApplicationsScrolled
        }));

    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    )
}

fn action_label(icon: IconSymbol, label: &'static str, size: f32) -> Row<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, size),
        crate::typography::readable_text(label)
    ]
    .spacing(6)
    .align_y(Alignment::Center)
}

pub(super) fn context_menu_panel<'a>(
    menu: &'a ContextMenuState,
    is_trash_view: bool,
    active_pane_id: BrowserPaneId,
    context_menus: &'a ContextMenuPreferences,
    list_view_preferences: &'a ListViewPreferences,
    selected_search_entry_types: &'a [SearchEntryTypePreset],
) -> Element<'a, Message> {
    match menu {
        ContextMenuState::FileArea(menu) => {
            file_context_menu_panel(menu, is_trash_view, active_pane_id, context_menus)
        }
        ContextMenuState::Search(menu) => search_context_menu_panel(menu, context_menus),
        ContextMenuState::SearchEntryTypes(_) => {
            search_entry_types_menu_panel(context_menus, selected_search_entry_types)
        }
        ContextMenuState::ListColumns(_) => {
            list_column_context_menu_panel(context_menus, list_view_preferences)
        }
        ContextMenuState::SidebarBookmark(menu) => {
            sidebar_bookmark_context_menu_panel(menu, context_menus)
        }
        ContextMenuState::SidebarDevice(menu) => {
            sidebar_device_context_menu_panel(menu, context_menus)
        }
        ContextMenuState::NetworkConnection(menu) => {
            network_connection_context_menu_panel(menu, context_menus)
        }
    }
}

fn search_entry_types_menu_panel<'a>(
    context_menus: &'a ContextMenuPreferences,
    selected_entry_types: &'a [SearchEntryTypePreset],
) -> Element<'a, Message> {
    let mut content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING + 2.0)
        .padding(CONTEXT_MENU_PADDING + 2.0);
    for entry_type in context_menus.search_entry_type_items() {
        content = content.push(
            checkbox(selected_entry_types.contains(&entry_type))
                .label(crate::localization::translate_current(entry_type.label()))
                .on_toggle(move |_| Message::SearchEntryTypeToggled(entry_type))
                .size(16)
                .text_size(13)
                .spacing(8),
        );
    }

    container(content)
        .width(Length::Fixed(190.0))
        .style(context_menu_style)
        .into()
}

fn search_context_menu_panel<'a>(
    menu: &'a SearchContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    for item in context_menus.search_items() {
        let (icon, label, message) = match item {
            SearchResultMenuItem::OpenContainingFolder => (
                IconSymbol::FolderOpen,
                item.label(),
                Message::SearchOpenContainingDirectory(menu.target.clone()),
            ),
            SearchResultMenuItem::Copy => (IconSymbol::Copy, item.label(), Message::CopySelected),
            SearchResultMenuItem::Cut => (IconSymbol::ArrowRight, item.label(), Message::MoveSelected),
            SearchResultMenuItem::MoveToTrash => {
                (IconSymbol::Trash, item.label(), Message::TrashSelected)
            }
            SearchResultMenuItem::DeletePermanently => (
                IconSymbol::Trash,
                item.label(),
                Message::SearchDeletePermanentlySelected,
            ),
        };
        content = content.push(menu_item(icon, label, message));
    }

    container(content)
        .width(Length::Fixed(CONTEXT_MENU_WIDTH))
        .style(context_menu_style)
        .into()
}

fn list_column_context_menu_panel<'a>(
    context_menus: &'a ContextMenuPreferences,
    preferences: &'a ListViewPreferences,
) -> Element<'a, Message> {
    let mut menu_content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    for kind in context_menus.list_column_items() {
        if let Some(column) = preferences
            .columns()
            .iter()
            .find(|column| column.kind == kind)
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
        readable_text(column.kind.label())
            .size(13)
            .width(Length::Fill),
        list_column_visibility_button(column),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
    .into()
}

fn list_column_visibility_button(column: &ListColumnConfig) -> Button<'static, Message> {
    let label = if column.kind == ListColumnKind::Name {
        "Required"
    } else if column.visible {
        "Hide"
    } else {
        "Show"
    };
    let button = button(
        container(readable_text(label).size(12))
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

fn file_context_menu_panel<'a>(
    menu: &'a FileContextMenuState,
    is_trash_view: bool,
    _active_pane_id: BrowserPaneId,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    if is_trash_view {
        return trash_context_menu_panel(menu, context_menus);
    }

    let terminal_directory = if menu.target_is_directory {
        menu.target
            .clone()
            .unwrap_or_else(|| menu.paste_directory.clone())
    } else {
        menu.paste_directory.clone()
    };

    // 行渲染与「New...」子菜单偏移共用同一份配置后的可见项列表。
    let items = match &menu.target {
        Some(_) => context_menus.file_entry_items(
            menu.target_is_directory,
            menu.can_batch_rename,
            menu.can_create_symlink,
        ),
        None => context_menus.file_blank_items(),
    };
    let mut menu_content = iced::widget::Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    let mut new_entry_row_index = None;
    for (row_index, item) in items.iter().enumerate() {
        if *item == FileAreaMenuItem::NewEntry {
            new_entry_row_index = Some(row_index);
            menu_content = menu_content.push(new_entry_menu_trigger());
            continue;
        }
        let row = match (item, &menu.target) {
            (FileAreaMenuItem::Open, Some(path)) => menu_item(
                IconSymbol::Folder,
                item.label(),
                Message::OpenPath(path.clone()),
            ),
            (FileAreaMenuItem::OpenWith, Some(path)) => menu_item(
                IconSymbol::Monitor,
                item.label(),
                Message::OpenWithRequested(path.clone()),
            ),
            (FileAreaMenuItem::Copy, _) => {
                menu_item(IconSymbol::Copy, item.label(), Message::CopySelected)
            }
            (FileAreaMenuItem::Duplicate, _) => {
                menu_item(IconSymbol::Copy, item.label(), Message::DuplicateSelected)
            }
            (FileAreaMenuItem::NewFolderFromSelection, _) => {
                menu_item(IconSymbol::Folder, item.label(), Message::NewFolderFromSelection)
            }
            (FileAreaMenuItem::CopyPath, _) => {
                menu_item(IconSymbol::List, item.label(), Message::CopyPathSelected)
            }
            (FileAreaMenuItem::CreateSymlink, _) => {
                menu_item(IconSymbol::Link, item.label(), Message::CreateSymlinkSelected)
            }
            (FileAreaMenuItem::Move, _) => menu_item(
                IconSymbol::ArrowRight,
                item.label(),
                Message::MoveSelected,
            ),
            (FileAreaMenuItem::CreateArchive, _) => menu_item(
                IconSymbol::FileArchive,
                item.label(),
                Message::ArchiveCreation(ArchiveCreationMessage::OpenSelected),
            ),
            (FileAreaMenuItem::ConvertFormat, _) => menu_item(
                IconSymbol::FileImage,
                item.label(),
                Message::Convert(ConvertMessage::OpenSelected),
            ),
            (FileAreaMenuItem::FileChecksum, _) => menu_item(
                IconSymbol::Hash,
                item.label(),
                Message::Checksum(ChecksumMessage::OpenSelected),
            ),
            (FileAreaMenuItem::Paste, _) => {
                menu_item(IconSymbol::Copy, item.label(), Message::PastePending)
            }
            (FileAreaMenuItem::Rename, Some(path)) => menu_item(
                IconSymbol::Pencil,
                item.label(),
                Message::BeginRename(path.clone()),
            ),
            (FileAreaMenuItem::BatchRename, _) => menu_item(
                IconSymbol::Pencil,
                item.label(),
                Message::BatchRename(BatchRenameMessage::OpenSelected),
            ),
            (FileAreaMenuItem::OpenTerminalHere, _) => menu_item(
                IconSymbol::Terminal,
                item.label(),
                Message::OpenTerminalHere(terminal_directory.clone()),
            ),
            (FileAreaMenuItem::Delete, _) => menu_item(
                IconSymbol::Trash,
                menu.delete_action.label(),
                Message::TrashSelected,
            ),
            (FileAreaMenuItem::Properties, Some(path)) => menu_item(
                IconSymbol::FileText,
                item.label(),
                Message::FileProperties(FilePropertiesMessage::Requested(path.clone())),
            ),
            // 空白菜单不含条目专属项;条目菜单必有 target。
            (FileAreaMenuItem::Open | FileAreaMenuItem::OpenWith | FileAreaMenuItem::Rename | FileAreaMenuItem::Properties, None) => {
                continue
            }
            // NewEntry 已在循环开头作为子菜单触发行处理。
            (FileAreaMenuItem::NewEntry, _) => continue,
        };
        menu_content = menu_content.push(row);
    }

    let root_menu = container(menu_content)
        .width(Length::Fixed(CONTEXT_MENU_WIDTH))
        .style(context_menu_style);

    let content = match (menu.expansion, new_entry_row_index) {
        (FileContextMenuExpansion::NewEntry, Some(new_entry_row_index)) => Row::new()
            .spacing(4)
            .push(root_menu)
            .push(new_entry_submenu_slot(new_entry_row_index, menu)),
        _ => Row::new().push(root_menu),
    };

    container(content).width(Length::Shrink).into()
}

fn new_entry_submenu_slot(row_index: usize, menu: &FileContextMenuState) -> Element<'_, Message> {
    // 偏移量由「New...」在可见项列表中的实际行号决定,替换旧的硬编码行数表。
    let trigger_top = CONTEXT_MENU_PADDING
        + row_index as f32 * (CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING);
    Column::new()
        .push(Space::new().height(Length::Fixed(trigger_top)))
        .push(new_entry_submenu(menu))
        .into()
}

fn new_entry_submenu(menu: &FileContextMenuState) -> Element<'_, Message> {
    let content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING)
        .push(submenu_item(
            IconSymbol::File,
            "New File",
            Message::CreateEmptyFile(menu.paste_directory.clone()),
        ))
        .push(submenu_item(
            IconSymbol::Folder,
            "New Folder",
            Message::CreateDirectory(menu.paste_directory.clone()),
        ));

    mouse_area(
        container(content)
            .width(Length::Fixed(CONTEXT_SUBMENU_WIDTH))
            .style(context_menu_style),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::NewEntry,
    ))
    .into()
}

fn new_entry_menu_trigger() -> Element<'static, Message> {
    mouse_area(
        button(menu_label_with_chevron(IconSymbol::File, "New..."))
            .on_press(Message::FileContextMenuExpansionChanged(
                FileContextMenuExpansion::NewEntry,
            ))
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(context_menu_item_button_style()),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::NewEntry,
    ))
    .into()
}

fn menu_item(icon: IconSymbol, label: &'static str, message: Message) -> Element<'static, Message> {
    mouse_area(menu_button(icon, label, message))
        .on_enter(Message::FileContextMenuExpansionChanged(
            FileContextMenuExpansion::None,
        ))
        .into()
}

fn submenu_item(
    icon: IconSymbol,
    label: &'static str,
    message: Message,
) -> Element<'static, Message> {
    mouse_area(menu_button(icon, label, message))
        .on_enter(Message::FileContextMenuExpansionChanged(
            FileContextMenuExpansion::NewEntry,
        ))
        .into()
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
        menu_content = menu_content.push(readable_text("No device actions available").size(12));
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
    menu: &'a FileContextMenuState,
    context_menus: &'a ContextMenuPreferences,
) -> Element<'a, Message> {
    let mut menu_content = iced::widget::Column::new().spacing(4).padding(8);
    for item in context_menus.trash_items(menu.target.is_some()) {
        match item {
            TrashMenuItem::Restore => menu_content = menu_content.push(menu_button(
                IconSymbol::ArrowLeft,
                item.label(),
                Message::RestoreSelected,
            )),
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

fn menu_button(
    icon: IconSymbol,
    label: &'static str,
    message: Message,
) -> Button<'static, Message> {
    button(menu_label(icon, label))
        .on_press(message)
        .width(Length::Fill)
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .style(context_menu_item_button_style())
}

fn menu_label(icon: IconSymbol, label: &'static str) -> Row<'static, Message> {
    action_label(icon, label, MENU_ICON_SIZE).width(Length::Fill)
}

fn menu_label_with_chevron(icon: IconSymbol, label: &'static str) -> Row<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE),
        crate::typography::readable_text(label).width(Length::Fill),
        themed_icon(IconSymbol::ChevronRight, IconTone::Normal, MENU_ICON_SIZE),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
}
