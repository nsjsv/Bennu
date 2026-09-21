//! 右键菜单的悬停子菜单:组触发行、子菜单槽位与成员行渲染。
//! 触发行与子菜单共用「展开锚点」这一状态,悬停普通行由 floating_panels 收起。

use iced::widget::{button, container, mouse_area, row, Column, Row};
use iced::{Alignment, Element, Length};

use crate::app::archive_creation::ArchiveCreationMessage;
use crate::app::checksum::ChecksumMessage;
use crate::app::convert::ConvertMessage;
use crate::appearance::{context_menu_item_button_style, context_menu_style};
use crate::icons::IconSymbol;
use crate::model::{
    BatchRenameMessage, FileAreaMenuItem, FileContextMenuExpansion, FileContextMenuState,
    FileGroupingMode, Message,
};

use super::{themed_icon, IconTone, MENU_ICON_SIZE};

pub(super) const CONTEXT_MENU_PADDING: f32 = 8.0;
pub(super) const CONTEXT_MENU_ITEM_SPACING: f32 = 4.0;
pub(super) const CONTEXT_MENU_ITEM_HEIGHT: f32 = 28.0;
pub(super) const CONTEXT_SUBMENU_WIDTH: f32 = 170.0;

/// 子菜单相对父浮层顶沿的纵向偏移:对齐触发行顶(行号来自结构列表,
/// 无硬编码行数);横向与安全区钳制由 floating_surface::BesideParent 负责。
pub(super) fn submenu_top(row_index: usize) -> f32 {
    CONTEXT_MENU_PADDING + row_index as f32 * (CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING)
}

/// 组触发行:动作锚点(on_press 有值)单点执行动作,纯触发器单点仅展开;悬停一律展开。
pub(super) fn group_trigger_row(
    anchor: FileAreaMenuItem,
    icon: IconSymbol,
    label: &'static str,
    on_press: Option<Message>,
) -> Element<'static, Message> {
    let mut button = button(menu_label_with_chevron(icon, label))
        .width(Length::Fill)
        .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
        .style(context_menu_item_button_style());
    if let Some(message) = on_press {
        button = button.on_press(message);
    }
    mouse_area(button)
        .on_enter(Message::FileContextMenuExpansionChanged(
            FileContextMenuExpansion::Group(anchor),
        ))
        .into()
}

/// 组的子菜单面板:「新建...」的硬编码行在最前,其余组只渲染成员行。
pub(super) fn group_submenu_panel(
    menu: &FileContextMenuState,
    anchor: FileAreaMenuItem,
    members: &[FileAreaMenuItem],
) -> Element<'static, Message> {
    let mut content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    if anchor == FileAreaMenuItem::NewEntry {
        content = content
            .push(submenu_item(
                IconSymbol::File,
                "New File",
                Message::CreateEmptyFile(menu.paste_directory.clone()),
                anchor,
            ))
            .push(submenu_item(
                IconSymbol::Folder,
                "New Folder",
                Message::CreateDirectory(menu.paste_directory.clone()),
                anchor,
            ));
    }
    for member in members {
        if let Some((icon, label, message)) = member_menu_action(*member) {
            content = content.push(submenu_item(icon, label, message, anchor));
        }
    }

    mouse_area(
        container(content)
            .width(Length::Fixed(CONTEXT_SUBMENU_WIDTH))
            .style(context_menu_style),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::Group(anchor),
    ))
    .into()
}

/// 空白菜单固定的「分组方式」触发行:纯触发器,悬停展开子菜单,
/// 单击无动作,与组锚点行同一交互。
pub(super) fn file_grouping_trigger_row() -> Element<'static, Message> {
    mouse_area(
        button(menu_label_with_chevron(IconSymbol::List, "Group By"))
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(context_menu_item_button_style()),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::FileGrouping,
    ))
    .into()
}

/// 分组方式子菜单:七项单选,当前生效项带勾选标记。悬停保持展开,
/// 单击只换档,菜单停留在原地便于连续切换。
pub(super) fn file_grouping_submenu_panel(current: FileGroupingMode) -> Element<'static, Message> {
    let modes = [
        FileGroupingMode::None,
        FileGroupingMode::NameInitial,
        FileGroupingMode::Kind,
        FileGroupingMode::Size,
        FileGroupingMode::ModifiedTime,
        FileGroupingMode::CreatedTime,
        FileGroupingMode::AccessedTime,
    ];
    let mut content = Column::new()
        .spacing(CONTEXT_MENU_ITEM_SPACING)
        .padding(CONTEXT_MENU_PADDING);
    for mode in modes {
        content = content.push(file_grouping_submenu_item(mode, mode == current));
    }

    mouse_area(
        container(content)
            .width(Length::Fixed(CONTEXT_SUBMENU_WIDTH))
            .style(context_menu_style),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::FileGrouping,
    ))
    .into()
}

/// 分组方式选项行:标签占满 + 当前项尾部勾选标记。
fn file_grouping_submenu_item(mode: FileGroupingMode, selected: bool) -> Element<'static, Message> {
    let mut label = row![crate::typography::readable_text(mode.label()).width(Length::Fill),]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if selected {
        label = label.push(themed_icon(
            IconSymbol::Check,
            IconTone::Normal,
            MENU_ICON_SIZE,
        ));
    }
    mouse_area(
        button(label)
            .on_press(Message::FileGroupingModeSelected(mode))
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(context_menu_item_button_style()),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::FileGrouping,
    ))
    .into()
}

/// 成员行的动作映射;成员只会是组表里声明的低频项,其余变体不可达。
fn member_menu_action(member: FileAreaMenuItem) -> Option<(IconSymbol, &'static str, Message)> {
    let action = match member {
        FileAreaMenuItem::Duplicate => (member.icon(), member.label(), Message::DuplicateSelected),
        FileAreaMenuItem::CopyPath => (member.icon(), member.label(), Message::CopyPathSelected),
        FileAreaMenuItem::CreateArchive => (
            member.icon(),
            member.label(),
            Message::ArchiveCreation(ArchiveCreationMessage::OpenSelected),
        ),
        FileAreaMenuItem::ConvertFormat => (
            member.icon(),
            member.label(),
            Message::Convert(ConvertMessage::OpenSelected),
        ),
        FileAreaMenuItem::FileChecksum => (
            member.icon(),
            member.label(),
            Message::Checksum(ChecksumMessage::OpenSelected),
        ),
        FileAreaMenuItem::BatchRename => (
            member.icon(),
            member.label(),
            Message::BatchRename(BatchRenameMessage::OpenSelected),
        ),
        FileAreaMenuItem::NewFolderFromSelection => (
            member.icon(),
            member.label(),
            Message::NewFolderFromSelection,
        ),
        FileAreaMenuItem::CreateSymlink => (
            member.icon(),
            member.label(),
            Message::CreateSymlinkSelected,
        ),
        _ => return None,
    };
    Some(action)
}

/// 子菜单成员行:悬停保持本组展开。
fn submenu_item(
    icon: IconSymbol,
    label: &'static str,
    message: Message,
    anchor: FileAreaMenuItem,
) -> Element<'static, Message> {
    mouse_area(
        button(menu_label(icon, label))
            .on_press(message)
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(context_menu_item_button_style()),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::Group(anchor),
    ))
    .into()
}

fn menu_label(icon: IconSymbol, label: &'static str) -> Row<'static, Message> {
    row![
        themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE),
        crate::typography::readable_text(label),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
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
