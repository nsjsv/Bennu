//! 文件区右键菜单有哪些行：视图按它画，键盘按它走，键盘执行的就是鼠标点那一行发的消息。
//! 每行带着它对应的快捷键，菜单里显示的键和实际键位是同一份。

use std::collections::BTreeMap;
use std::sync::Arc;

use iced::keyboard;

use crate::actions::service_menu::Priority;
use crate::actions::{ActionId, ServiceMenuAction};
use crate::app::actions::{ActionMessage, ActionSubject};
use crate::app::archive_creation::ArchiveCreationMessage;
use crate::app::checksum::ChecksumMessage;
use crate::app::convert::ConvertMessage;
use crate::app::transfer::TransferMessage;
use crate::model::{
    BatchRenameMessage, ContextMenuPreferences, FileAreaMenuItem, FileContextMenuExpansion,
    FileContextMenuState, FileDeleteAction, FileEntryMenuEntry, FileGroupingMode,
    FilePropertiesMessage, Message,
};
use crate::shortcuts::{ShortcutAction, ShortcutTarget};
use bennu_theme::icons::IconSymbol;

/// 菜单里的几段。段跟着每一项走，不按位置写死：用户在设置里调了顺序，
/// 同一段的项挨在一起就合成一段，被拆开了就各自成段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuSection {
    Open,
    /// 对选中的文件本身动手：复制、移动、改名，以及“工具”里的压缩、转换、校验、建链接。
    /// 工具放这段还因为已存下的顺序里它夹在移动和粘贴之间，放别段会把这段劈成两半。
    Edit,
    /// 新建、在这里开终端、发到手机。
    NewAndShare,
    Delete,
    Properties,
    /// 空白处菜单末尾的“分组方式”，管的是怎么看，不是对文件做什么。
    View,
}

#[derive(Debug, Clone)]
pub(crate) struct MenuRow {
    pub(crate) icon: Option<IconSymbol>,
    pub(crate) label: String,
    pub(crate) section: MenuSection,
    pub(crate) shortcut: Option<ShortcutTarget>,
    /// 单选项（分组方式）的当前项。
    pub(crate) checked: bool,
    pub(crate) kind: MenuRowKind,
}

#[derive(Debug, Clone)]
pub(crate) enum MenuRowKind {
    Action(Message),
    /// 有子菜单的行：悬停或键盘 → 展开 `expansion`；单点执行 `on_press`（没有自己动作的组行，
    /// `on_press` 就是展开）。
    Submenu {
        expansion: FileContextMenuExpansion,
        on_press: Message,
        members: Vec<MenuRow>,
    },
}

impl MenuRow {
    pub(crate) fn expansion(&self) -> Option<FileContextMenuExpansion> {
        match &self.kind {
            MenuRowKind::Submenu { expansion, .. } => Some(*expansion),
            MenuRowKind::Action(_) => None,
        }
    }

    pub(crate) fn members(&self) -> &[MenuRow] {
        match &self.kind {
            MenuRowKind::Submenu { members, .. } => members,
            MenuRowKind::Action(_) => &[],
        }
    }

    /// 点这一行（或键盘在这一行按 Enter）发的消息。
    pub(crate) fn press_message(&self) -> Message {
        match &self.kind {
            MenuRowKind::Action(message) => message.clone(),
            MenuRowKind::Submenu { on_press, .. } => on_press.clone(),
        }
    }
}

fn item_section(item: FileAreaMenuItem) -> MenuSection {
    match item {
        FileAreaMenuItem::Open
        | FileAreaMenuItem::OpenContainingFolder
        | FileAreaMenuItem::OpenWith
        | FileAreaMenuItem::AddToFavorites
        | FileAreaMenuItem::SmartExtractHere
        | FileAreaMenuItem::ExtractToArchiveFolder => MenuSection::Open,
        FileAreaMenuItem::Copy
        | FileAreaMenuItem::Duplicate
        | FileAreaMenuItem::CopyPath
        | FileAreaMenuItem::Move
        | FileAreaMenuItem::Paste
        | FileAreaMenuItem::Rename
        | FileAreaMenuItem::BatchRename
        | FileAreaMenuItem::Tools
        | FileAreaMenuItem::CreateArchive
        | FileAreaMenuItem::ConvertFormat
        | FileAreaMenuItem::FileChecksum
        | FileAreaMenuItem::CreateSymlink => MenuSection::Edit,
        FileAreaMenuItem::NewEntry
        | FileAreaMenuItem::NewFolderFromSelection
        | FileAreaMenuItem::OpenTerminalHere
        | FileAreaMenuItem::SendToPhone
        | FileAreaMenuItem::Actions => MenuSection::NewAndShare,
        FileAreaMenuItem::Delete => MenuSection::Delete,
        FileAreaMenuItem::Properties => MenuSection::Properties,
    }
}

fn action_row(
    item: FileAreaMenuItem,
    icon: IconSymbol,
    label: String,
    shortcut: Option<ShortcutAction>,
    message: Message,
) -> MenuRow {
    MenuRow {
        icon: Some(icon),
        label,
        section: item_section(item),
        shortcut: shortcut.map(ShortcutTarget::Builtin),
        checked: false,
        kind: MenuRowKind::Action(message),
    }
}

/// 打开菜单时按住 Alt / Shift，复制和删除两行实时换成复制路径 / 永久删除。
pub(crate) fn file_context_menu_rows(
    menu: &FileContextMenuState,
    context_menus: &ContextMenuPreferences,
    keyboard_modifiers: keyboard::Modifiers,
    file_grouping: FileGroupingMode,
) -> Vec<MenuRow> {
    let alt_held = keyboard_modifiers.alt();
    let shift_held = keyboard_modifiers.shift();
    let mut rows = Vec::new();
    for entry in menu_entries(menu, context_menus) {
        match entry {
            FileEntryMenuEntry::Item(FileAreaMenuItem::Actions) => rows.extend(action_rows(menu)),
            FileEntryMenuEntry::Item(item) => {
                if let Some(row) = item_row(item, menu, alt_held, shift_held) {
                    rows.push(row);
                }
            }
            FileEntryMenuEntry::Group { anchor, members } => {
                rows.push(group_row(anchor, &members, menu, alt_held));
            }
        }
    }
    // 空白菜单末尾的固定「分组方式」触发行:构造时已按视图模式门控,条目菜单一律不带。
    if menu.grouping_entry_visible {
        rows.push(file_grouping_row(file_grouping));
    }
    rows
}

/// 快捷操作面板的行：菜单里把选中的文件变成别的东西的那几项（解压、压缩、转换、校验、批量
/// 改名）和「动作」那一块。从同一份菜单结构里挑，设置里关掉的、这批文件用不了的，面板里也没有。
pub(crate) fn quick_action_rows(
    menu: &FileContextMenuState,
    context_menus: &ContextMenuPreferences,
) -> Vec<MenuRow> {
    let is_quick_action = |item: &FileAreaMenuItem| {
        matches!(
            item,
            FileAreaMenuItem::SmartExtractHere
                | FileAreaMenuItem::ExtractToArchiveFolder
                | FileAreaMenuItem::CreateArchive
                | FileAreaMenuItem::ConvertFormat
                | FileAreaMenuItem::FileChecksum
                | FileAreaMenuItem::BatchRename
        )
    };
    let mut rows = Vec::new();
    for entry in menu_entries(menu, context_menus) {
        match entry {
            FileEntryMenuEntry::Item(FileAreaMenuItem::Actions) => rows.extend(action_rows(menu)),
            FileEntryMenuEntry::Item(item) if is_quick_action(&item) => {
                rows.extend(item_row(item, menu, false, false));
            }
            FileEntryMenuEntry::Item(_) => {}
            FileEntryMenuEntry::Group { members, .. } => rows.extend(
                members
                    .into_iter()
                    .filter(is_quick_action)
                    .filter_map(member_row),
            ),
        }
    }
    rows
}

/// 配置后的结构列表;空白菜单没有组成员,只有「New...」组行。
fn menu_entries(
    menu: &FileContextMenuState,
    context_menus: &ContextMenuPreferences,
) -> Vec<FileEntryMenuEntry> {
    let Some(_) = &menu.target else {
        return context_menus
            .file_blank_items()
            .into_iter()
            .map(|item| {
                if item == FileAreaMenuItem::NewEntry {
                    FileEntryMenuEntry::Group {
                        anchor: item,
                        members: Vec::new(),
                    }
                } else {
                    FileEntryMenuEntry::Item(item)
                }
            })
            .collect();
    };
    if menu.inside_archive {
        // 包内只读：条目菜单切换为固定的只读子集，写操作与解压入口整体不渲染。
        return [
            FileAreaMenuItem::Open,
            FileAreaMenuItem::Copy,
            FileAreaMenuItem::CopyPath,
            FileAreaMenuItem::Properties,
        ]
        .into_iter()
        .map(FileEntryMenuEntry::Item)
        .collect();
    }
    let mut entries =
        context_menus.file_entry_menu_entries(menu.place, menu.target_kind, menu.selection);
    if menu.selection_archives.is_empty() {
        // 解压两项只属于压缩包条目；其余归档无关选中不含它们。
        let is_extract = |item: &FileAreaMenuItem| {
            matches!(
                item,
                FileAreaMenuItem::SmartExtractHere | FileAreaMenuItem::ExtractToArchiveFolder
            )
        };
        entries.retain(|entry| match entry {
            FileEntryMenuEntry::Item(item) => !is_extract(item),
            FileEntryMenuEntry::Group { members, .. } => !members.iter().any(is_extract),
        });
    }
    entries
}

fn terminal_directory(menu: &FileContextMenuState) -> std::path::PathBuf {
    if menu.target_kind.is_directory() {
        menu.target
            .clone()
            .unwrap_or_else(|| menu.paste_directory.clone())
    } else {
        menu.paste_directory.clone()
    }
}

/// 一级普通行。组成员只会出现在子菜单里，组锚点走组行；返回 None 的变体是布局子集
/// 保证不会出现的防御分支。
fn item_row(
    item: FileAreaMenuItem,
    menu: &FileContextMenuState,
    alt_held: bool,
    shift_held: bool,
) -> Option<MenuRow> {
    use ShortcutAction as A;
    let row = |icon, shortcut, message| action_row(item, icon, item.label(), shortcut, message);
    Some(match (item, &menu.target) {
        (FileAreaMenuItem::Open, Some(path)) => row(
            IconSymbol::Folder,
            Some(A::OpenSelected),
            Message::OpenPath(path.clone()),
        ),
        (FileAreaMenuItem::OpenContainingFolder, Some(path)) => row(
            item.icon(),
            None,
            Message::SearchOpenContainingDirectory(path.clone()),
        ),
        (FileAreaMenuItem::SmartExtractHere, _) => {
            row(item.icon(), None, Message::SmartExtractSelected)
        }
        (FileAreaMenuItem::ExtractToArchiveFolder, _) => {
            row(item.icon(), None, Message::ExtractSelectedToArchiveFolder)
        }
        (FileAreaMenuItem::OpenWith, Some(path)) => row(
            IconSymbol::Monitor,
            None,
            Message::OpenWithRequested(path.clone()),
        ),
        (FileAreaMenuItem::AddToFavorites, Some(path)) => row(
            item.icon(),
            Some(A::AddToFavorites),
            Message::SidebarBookmarkAddRequested(path.clone()),
        ),
        (FileAreaMenuItem::Copy, _) => copy_row(alt_held),
        (FileAreaMenuItem::Move, _) => {
            row(IconSymbol::ArrowRight, Some(A::Cut), Message::MoveSelected)
        }
        (FileAreaMenuItem::Paste, _) => {
            row(IconSymbol::Copy, Some(A::Paste), Message::PastePending)
        }
        (FileAreaMenuItem::Rename, Some(path)) => row(
            IconSymbol::Pencil,
            Some(A::RenameSelected),
            Message::BeginRename(path.clone()),
        ),
        (FileAreaMenuItem::OpenTerminalHere, _) => row(
            IconSymbol::Terminal,
            Some(A::OpenExternalTerminal),
            Message::OpenTerminalHere(terminal_directory(menu)),
        ),
        (FileAreaMenuItem::Delete, _) => delete_row(menu, shift_held),
        (FileAreaMenuItem::Properties, Some(path)) => row(
            IconSymbol::FileText,
            Some(A::FileProperties),
            Message::FileProperties(FilePropertiesMessage::Requested(path.clone())),
        ),
        (FileAreaMenuItem::SendToPhone, _) => row(
            item.icon(),
            None,
            Message::Transfer(TransferMessage::SendToPhoneRequested),
        ),
        // 组锚点不经普通行渲染;条目专属项不进空白菜单;组成员只渲染在子菜单里。
        _ => return None,
    })
}

/// "动作"那一块不超过这么多个时直接排在菜单里，多了收进"动作"子菜单（同 KIO）。
const INLINE_ACTION_LIMIT: usize = 4;

/// 自定义动作和服务菜单，顺序同 KIO 的 `addServiceActionsTo`：
/// 1. 能收起来的一块：Bennu 自己的动作，Important 的项，普通的项；
/// 2. 服务菜单自己的子菜单，每个一行（Bennu 的菜单只有一层子菜单，所以不收进"动作"）；
/// 3. TopLevel 的项，总是直接放在主菜单里。
///
/// 同一组里的服务菜单项按动作键排序，子菜单按名字排序。
fn action_rows(menu: &FileContextMenuState) -> Vec<MenuRow> {
    let subject = match &menu.target {
        Some(_) => ActionSubject::Selection,
        None => ActionSubject::Folder(menu.paste_directory.clone()),
    };
    let row = |id: ActionId, name: &str| MenuRow {
        icon: Some(IconSymbol::Play),
        label: name.to_owned(),
        section: MenuSection::NewAndShare,
        shortcut: Some(ShortcutTarget::Action(id.clone())),
        checked: false,
        kind: MenuRowKind::Action(Message::Action(ActionMessage::Invoked {
            action: id,
            subject: subject.clone(),
        })),
    };
    let service_menus = &menu.actions.service_menus;
    let loose = |priority: Priority| {
        let mut chosen = service_menus
            .iter()
            .filter(|action| action.priority == priority && action.submenu.is_none())
            .collect::<Vec<_>>();
        chosen.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        chosen
            .into_iter()
            .map(|action| row(action.id(), &action.name))
            .collect::<Vec<_>>()
    };

    let mut block = menu
        .actions
        .own
        .iter()
        .map(|definition| row(definition.id.clone(), &definition.name))
        .collect::<Vec<_>>();
    block.extend(loose(Priority::Important));
    block.extend(loose(Priority::Normal));
    let mut rows = if block.len() <= INLINE_ACTION_LIMIT {
        block
    } else {
        let expansion = FileContextMenuExpansion::Group(FileAreaMenuItem::Actions);
        vec![submenu_row(
            FileAreaMenuItem::Actions.label(),
            expansion,
            block,
        )]
    };

    let mut submenus = BTreeMap::<(Priority, &str), Vec<&Arc<ServiceMenuAction>>>::new();
    for action in service_menus {
        if let Some(name) = &action.submenu {
            submenus
                .entry((action.priority, name.as_str()))
                .or_default()
                .push(action);
        }
    }
    for (index, ((_, name), mut members)) in submenus.into_iter().enumerate() {
        members.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        let members = members
            .into_iter()
            .map(|action| row(action.id(), &action.name))
            .collect();
        rows.push(submenu_row(
            name.to_owned(),
            FileContextMenuExpansion::ServiceSubmenu(index),
            members,
        ));
    }
    rows.extend(loose(Priority::TopLevel));
    rows
}

fn submenu_row(
    label: String,
    expansion: FileContextMenuExpansion,
    members: Vec<MenuRow>,
) -> MenuRow {
    MenuRow {
        icon: Some(FileAreaMenuItem::Actions.icon()),
        label,
        section: MenuSection::NewAndShare,
        shortcut: None,
        checked: false,
        kind: MenuRowKind::Submenu {
            expansion,
            on_press: Message::FileContextMenuExpansionChanged(expansion),
            members,
        },
    }
}

/// 复制：按住 Alt 时实时切换为复制路径。
fn copy_row(alt_held: bool) -> MenuRow {
    if alt_held {
        action_row(
            FileAreaMenuItem::Copy,
            IconSymbol::List,
            FileAreaMenuItem::CopyPath.label(),
            Some(ShortcutAction::CopyPath),
            Message::CopyPathSelected,
        )
    } else {
        action_row(
            FileAreaMenuItem::Copy,
            IconSymbol::Copy,
            FileAreaMenuItem::Copy.label(),
            Some(ShortcutAction::Copy),
            Message::CopySelected,
        )
    }
}

/// 删除：纯本地选中且按住 Shift 时实时切换为永久删除（仍走确认框）；远程选中本就永久删除、
/// 混合选中本就分流，Shift 无额外效果。
fn delete_row(menu: &FileContextMenuState, shift_held: bool) -> MenuRow {
    if shift_held && menu.delete_action == FileDeleteAction::MoveToTrash {
        action_row(
            FileAreaMenuItem::Delete,
            IconSymbol::Trash,
            FileDeleteAction::DeletePermanently.label(),
            Some(ShortcutAction::DeletePermanently),
            Message::DeleteSelectedPermanently,
        )
    } else {
        action_row(
            FileAreaMenuItem::Delete,
            IconSymbol::Trash,
            menu.delete_action.label(),
            Some(ShortcutAction::Delete),
            Message::TrashSelected,
        )
    }
}

/// 组行：悬停展开；单点执行锚点自己的动作（复制、重命名），没有动作的锚点单点也是展开。
fn group_row(
    anchor: FileAreaMenuItem,
    members: &[FileAreaMenuItem],
    menu: &FileContextMenuState,
    alt_held: bool,
) -> MenuRow {
    let expansion = FileContextMenuExpansion::Group(anchor);
    let (icon, label, shortcut, on_press) = match anchor {
        FileAreaMenuItem::Copy => {
            let copy = copy_row(alt_held);
            let on_press = copy.press_message();
            (copy.icon, copy.label, copy.shortcut, on_press)
        }
        FileAreaMenuItem::Rename => (
            Some(IconSymbol::Pencil),
            FileAreaMenuItem::Rename.label(),
            Some(ShortcutAction::RenameSelected.into()),
            Message::BeginRename(
                menu.target
                    .clone()
                    .unwrap_or_else(|| menu.paste_directory.clone()),
            ),
        ),
        other => (
            Some(other.icon()),
            other.label(),
            None,
            Message::FileContextMenuExpansionChanged(expansion),
        ),
    };
    let mut member_rows = Vec::new();
    if anchor == FileAreaMenuItem::NewEntry {
        member_rows.push(action_row(
            anchor,
            IconSymbol::File,
            tr!("New File"),
            Some(ShortcutAction::NewFile),
            Message::CreateEmptyFile(menu.paste_directory.clone()),
        ));
        member_rows.push(action_row(
            anchor,
            IconSymbol::Folder,
            tr!("New Folder"),
            Some(ShortcutAction::NewFolder),
            Message::CreateDirectory(menu.paste_directory.clone()),
        ));
    }
    member_rows.extend(members.iter().copied().filter_map(member_row));
    MenuRow {
        icon,
        label,
        section: item_section(anchor),
        shortcut,
        checked: false,
        kind: MenuRowKind::Submenu {
            expansion,
            on_press,
            members: member_rows,
        },
    }
}

/// 子菜单成员行；成员只会是组表里声明的低频项，其余变体不可达。
fn member_row(member: FileAreaMenuItem) -> Option<MenuRow> {
    use ShortcutAction as A;
    let (shortcut, message) = match member {
        FileAreaMenuItem::Duplicate => (Some(A::DuplicateSelected), Message::DuplicateSelected),
        FileAreaMenuItem::CopyPath => (Some(A::CopyPath), Message::CopyPathSelected),
        FileAreaMenuItem::CreateArchive => (
            None,
            Message::ArchiveCreation(ArchiveCreationMessage::OpenSelected),
        ),
        FileAreaMenuItem::ConvertFormat => (None, Message::Convert(ConvertMessage::OpenSelected)),
        FileAreaMenuItem::FileChecksum => (None, Message::Checksum(ChecksumMessage::OpenSelected)),
        FileAreaMenuItem::BatchRename => {
            (None, Message::BatchRename(BatchRenameMessage::OpenSelected))
        }
        FileAreaMenuItem::NewFolderFromSelection => (
            Some(A::NewFolderFromSelection),
            Message::NewFolderFromSelection,
        ),
        FileAreaMenuItem::CreateSymlink => (Some(A::CreateSymlink), Message::CreateSymlinkSelected),
        _ => return None,
    };
    Some(action_row(
        member,
        member.icon(),
        member.label(),
        shortcut,
        message,
    ))
}

/// 空白菜单的「分组方式」：纯触发器，七项单选；选一项只换档，菜单留在原地便于连续切换。
fn file_grouping_row(current: FileGroupingMode) -> MenuRow {
    let expansion = FileContextMenuExpansion::FileGrouping;
    let members = [
        FileGroupingMode::None,
        FileGroupingMode::NameInitial,
        FileGroupingMode::Kind,
        FileGroupingMode::Size,
        FileGroupingMode::ModifiedTime,
        FileGroupingMode::CreatedTime,
        FileGroupingMode::AccessedTime,
    ]
    .into_iter()
    .map(|mode| MenuRow {
        icon: None,
        label: mode.label(),
        section: MenuSection::View,
        shortcut: None,
        checked: mode == current,
        kind: MenuRowKind::Action(Message::FileGroupingModeSelected(mode)),
    })
    .collect();
    MenuRow {
        icon: Some(IconSymbol::List),
        label: tr!("Group By"),
        section: MenuSection::View,
        shortcut: None,
        checked: false,
        kind: MenuRowKind::Submenu {
            expansion,
            on_press: Message::FileContextMenuExpansionChanged(expansion),
            members,
        },
    }
}
