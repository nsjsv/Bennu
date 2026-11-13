//! 文件区右键菜单（点在条目上 / 点在空白处）：一级行按用途分段，段与段之间一条细线；
//! 悬停组行时子菜单贴在旁边，第一项和触发行对齐。

use iced::widget::{button, container, mouse_area, Column, Space};
use iced::{Background, Element, Length, Theme};

use crate::app::transfer::TransferMessage;
use crate::appearance::{context_menu_item_button_style, context_menu_style};
use crate::model::{
    ContextMenuPreferences, FileAreaMenuItem, FileContextMenuExpansion, FileContextMenuState,
    FileDeleteAction, FileEntryMenuEntry, FileGroupingMode, FilePropertiesMessage, Message,
};
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;

use super::context_menu_submenu::{
    file_grouping_submenu_panel, file_grouping_trigger_row, group_submenu_panel, group_trigger_row,
    menu_label, CONTEXT_MENU_ITEM_HEIGHT, CONTEXT_MENU_ITEM_SPACING, CONTEXT_MENU_PADDING,
    CONTEXT_MENU_WIDTH,
};

/// 分隔线上下各留一点空，加上行间距，段与段比行与行明显分得开。
const SECTION_DIVIDER_GAP: f32 = 2.0;
const SECTION_DIVIDER_HEIGHT: f32 = 1.0 + SECTION_DIVIDER_GAP * 2.0;

/// 菜单里的几段。段跟着每一项走，不按位置写死：用户在设置里调了顺序，
/// 同一段的项挨在一起就合成一段，被拆开了就各自成段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuSection {
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

fn item_section(item: FileAreaMenuItem) -> MenuSection {
    match item {
        FileAreaMenuItem::Open
        | FileAreaMenuItem::OpenContainingFolder
        | FileAreaMenuItem::OpenWith
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
        | FileAreaMenuItem::SendToPhone => MenuSection::NewAndShare,
        FileAreaMenuItem::Delete => MenuSection::Delete,
        FileAreaMenuItem::Properties => MenuSection::Properties,
    }
}

/// 一级菜单从上往下排：换段时先插分隔线；同时记下每行顶边的纵坐标，
/// 子菜单要贴着触发行出来，分隔线的高度也得算进去。
struct MenuRows<'a> {
    rows: Vec<Element<'a, Message>>,
    next_top: f32,
    section: Option<MenuSection>,
}

impl<'a> MenuRows<'a> {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            next_top: CONTEXT_MENU_PADDING,
            section: None,
        }
    }

    /// 返回这一行顶边离菜单顶边多远。
    fn push(&mut self, section: MenuSection, row: Element<'a, Message>) -> f32 {
        if self.section.is_some_and(|previous| previous != section) {
            self.rows.push(section_divider());
            self.next_top += SECTION_DIVIDER_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        }
        self.section = Some(section);
        let top = self.next_top;
        self.rows.push(row);
        self.next_top += CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        top
    }

    fn into_menu(self) -> Element<'a, Message> {
        container(
            Column::with_children(self.rows)
                .spacing(CONTEXT_MENU_ITEM_SPACING)
                .padding(CONTEXT_MENU_PADDING),
        )
        .width(Length::Fixed(CONTEXT_MENU_WIDTH))
        .style(context_menu_style)
        .into()
    }
}

/// 子菜单顶边相对主菜单顶边的偏移：两边内边距一样，往上提一个内边距，
/// 子菜单第一项就和触发行在同一高度。
fn submenu_top(trigger_top: f32) -> f32 {
    trigger_top - CONTEXT_MENU_PADDING
}

fn section_divider() -> Element<'static, Message> {
    container(
        container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(ui_colors(theme).outline_variant)),
                ..container::Style::default()
            }),
    )
    .padding([SECTION_DIVIDER_GAP, 6.0])
    .width(Length::Fill)
    .into()
}

pub(super) fn file_context_menu_panel<'a>(
    menu: &'a FileContextMenuState,
    context_menus: &'a ContextMenuPreferences,
    keyboard_modifiers: iced::keyboard::Modifiers,
    file_grouping: FileGroupingMode,
) -> (Element<'a, Message>, Option<(Element<'a, Message>, f32)>) {
    let terminal_directory = if menu.target_is_directory {
        menu.target
            .clone()
            .unwrap_or_else(|| menu.paste_directory.clone())
    } else {
        menu.paste_directory.clone()
    };

    let alt_held = keyboard_modifiers.alt();
    let shift_held = keyboard_modifiers.shift();

    // 行渲染与子菜单展开共用同一份配置后的结构列表;空白菜单没有组成员,
    // 只有「New...」组行。
    let entries: Vec<FileEntryMenuEntry> = match &menu.target {
        Some(_) => {
            let mut entries = context_menus.file_entry_menu_entries(
                menu.place,
                menu.target_is_directory,
                menu.can_batch_rename,
                menu.can_create_symlink,
            );
            if menu.inside_archive {
                // 包内只读：条目菜单切换为固定的只读子集，写操作与
                // 解压入口整体不渲染（门控是防御，菜单是交互层真相）。
                entries = [
                    FileAreaMenuItem::Open,
                    FileAreaMenuItem::Copy,
                    FileAreaMenuItem::CopyPath,
                    FileAreaMenuItem::Properties,
                ]
                .into_iter()
                .map(FileEntryMenuEntry::Item)
                .collect();
            } else if menu.selection_archives.is_empty() {
                // 解压两项只属于压缩包条目；其余归档无关选中不含它们。
                entries.retain(|entry| match entry {
                    FileEntryMenuEntry::Item(item) => !matches!(
                        item,
                        FileAreaMenuItem::SmartExtractHere
                            | FileAreaMenuItem::ExtractToArchiveFolder
                    ),
                    FileEntryMenuEntry::Group { members, .. } => !members.iter().any(|item| {
                        matches!(
                            item,
                            FileAreaMenuItem::SmartExtractHere
                                | FileAreaMenuItem::ExtractToArchiveFolder
                        )
                    }),
                });
            }
            entries
        }
        None => context_menus
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
            .collect(),
    };

    let mut rows = MenuRows::new();
    let mut submenu = None;
    for entry in &entries {
        match entry {
            FileEntryMenuEntry::Item(item) => {
                if let Some(row) =
                    file_menu_action_row(*item, menu, &terminal_directory, alt_held, shift_held)
                {
                    rows.push(item_section(*item), row);
                }
            }
            FileEntryMenuEntry::Group { anchor, members } => {
                let (icon, label, on_press) = match *anchor {
                    FileAreaMenuItem::Copy => copy_menu_action(alt_held),
                    FileAreaMenuItem::Rename => rename_menu_action(menu),
                    // 本身没有动作的组行，点一下也是展开子菜单。
                    other => (
                        other.icon(),
                        other.label(),
                        Message::FileContextMenuExpansionChanged(FileContextMenuExpansion::Group(
                            other,
                        )),
                    ),
                };
                let top = rows.push(
                    item_section(*anchor),
                    group_trigger_row(*anchor, icon, label, on_press),
                );
                if menu.expansion == FileContextMenuExpansion::Group(*anchor) {
                    submenu = Some((
                        group_submenu_panel(menu, *anchor, members),
                        submenu_top(top),
                    ));
                }
            }
        }
    }

    // 空白菜单末尾的固定「分组方式」触发行:构造时已按视图模式门控,
    // 条目菜单一律不带该入口。
    if menu.grouping_entry_visible {
        let top = rows.push(MenuSection::View, file_grouping_trigger_row());
        if menu.expansion == FileContextMenuExpansion::FileGrouping {
            submenu = Some((file_grouping_submenu_panel(file_grouping), submenu_top(top)));
        }
    }

    (rows.into_menu(), submenu)
}

/// 复制的动作与文案:按住 Alt 时实时切换为复制路径。
fn copy_menu_action(alt_held: bool) -> (IconSymbol, String, Message) {
    if alt_held {
        (
            IconSymbol::List,
            FileAreaMenuItem::CopyPath.label(),
            Message::CopyPathSelected,
        )
    } else {
        (
            IconSymbol::Copy,
            FileAreaMenuItem::Copy.label(),
            Message::CopySelected,
        )
    }
}

/// 重命名的动作与文案;条目菜单必有 target。
fn rename_menu_action(menu: &FileContextMenuState) -> (IconSymbol, String, Message) {
    let path = menu
        .target
        .clone()
        .unwrap_or_else(|| menu.paste_directory.clone());
    (
        IconSymbol::Pencil,
        FileAreaMenuItem::Rename.label(),
        Message::BeginRename(path),
    )
}

/// 删除的动作与文案:纯本地选中且按住 Shift 时实时切换为永久删除(仍走确认框);
/// 远程选中本就永久删除、混合选中本就分流,Shift 无额外效果。
fn delete_menu_action(
    menu: &FileContextMenuState,
    shift_held: bool,
) -> (IconSymbol, String, Message) {
    if shift_held && menu.delete_action == FileDeleteAction::MoveToTrash {
        (
            IconSymbol::Trash,
            FileDeleteAction::DeletePermanently.label(),
            Message::DeleteSelectedPermanently,
        )
    } else {
        (
            IconSymbol::Trash,
            menu.delete_action.label(),
            Message::TrashSelected,
        )
    }
}

/// 一级普通行的动作渲染。组成员只会出现在子菜单里,组锚点走组行;
/// 返回 None 的变体是布局子集保证不会出现的防御分支。
fn file_menu_action_row(
    item: FileAreaMenuItem,
    menu: &FileContextMenuState,
    terminal_directory: &std::path::Path,
    alt_held: bool,
    shift_held: bool,
) -> Option<Element<'static, Message>> {
    let row = match (item, &menu.target) {
        (FileAreaMenuItem::Open, Some(path)) => menu_item(
            IconSymbol::Folder,
            item.label(),
            Message::OpenPath(path.clone()),
        ),
        (FileAreaMenuItem::OpenContainingFolder, Some(path)) => menu_item(
            item.icon(),
            item.label(),
            Message::SearchOpenContainingDirectory(path.clone()),
        ),
        (FileAreaMenuItem::SmartExtractHere, _) => {
            menu_item(item.icon(), item.label(), Message::SmartExtractSelected)
        }
        (FileAreaMenuItem::ExtractToArchiveFolder, _) => menu_item(
            item.icon(),
            item.label(),
            Message::ExtractSelectedToArchiveFolder,
        ),
        (FileAreaMenuItem::OpenWith, Some(path)) => menu_item(
            IconSymbol::Monitor,
            item.label(),
            Message::OpenWithRequested(path.clone()),
        ),
        (FileAreaMenuItem::Copy, _) => {
            let (icon, label, message) = copy_menu_action(alt_held);
            menu_item(icon, label, message)
        }
        (FileAreaMenuItem::Move, _) => {
            menu_item(IconSymbol::ArrowRight, item.label(), Message::MoveSelected)
        }
        (FileAreaMenuItem::Paste, _) => {
            menu_item(IconSymbol::Copy, item.label(), Message::PastePending)
        }
        (FileAreaMenuItem::Rename, Some(path)) => menu_item(
            IconSymbol::Pencil,
            item.label(),
            Message::BeginRename(path.clone()),
        ),
        (FileAreaMenuItem::OpenTerminalHere, _) => menu_item(
            IconSymbol::Terminal,
            item.label(),
            Message::OpenTerminalHere(terminal_directory.to_path_buf()),
        ),
        (FileAreaMenuItem::Delete, _) => {
            let (icon, label, message) = delete_menu_action(menu, shift_held);
            menu_item(icon, label, message)
        }
        (FileAreaMenuItem::Properties, Some(path)) => menu_item(
            IconSymbol::FileText,
            item.label(),
            Message::FileProperties(FilePropertiesMessage::Requested(path.clone())),
        ),
        (FileAreaMenuItem::SendToPhone, _) => menu_item(
            item.icon(),
            item.label(),
            Message::Transfer(TransferMessage::SendToPhoneRequested),
        ),
        // 组锚点不经普通行渲染;条目专属项不进空白菜单。
        (
            FileAreaMenuItem::NewEntry
            | FileAreaMenuItem::Tools
            | FileAreaMenuItem::Open
            | FileAreaMenuItem::OpenContainingFolder
            | FileAreaMenuItem::OpenWith
            | FileAreaMenuItem::Rename
            | FileAreaMenuItem::Properties,
            _,
        ) => return None,
        // 组成员只渲染在子菜单里。
        _ => return None,
    };
    Some(row)
}

/// 普通行：悬停时收起别的组的子菜单。
fn menu_item(icon: IconSymbol, label: String, message: Message) -> Element<'static, Message> {
    mouse_area(
        button(menu_label(icon, label))
            .on_press(message)
            .width(Length::Fill)
            .height(Length::Fixed(CONTEXT_MENU_ITEM_HEIGHT))
            .style(context_menu_item_button_style()),
    )
    .on_enter(Message::FileContextMenuExpansionChanged(
        FileContextMenuExpansion::None,
    ))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder_row() -> Element<'static, Message> {
        Space::new().into()
    }

    #[test]
    fn a_divider_goes_only_between_sections_and_shifts_the_rows_below_it() {
        let step = CONTEXT_MENU_ITEM_HEIGHT + CONTEXT_MENU_ITEM_SPACING;
        let mut rows = MenuRows::new();

        let open = rows.push(MenuSection::Open, placeholder_row());
        let open_with = rows.push(MenuSection::Open, placeholder_row());
        let copy = rows.push(MenuSection::Edit, placeholder_row());
        let move_to = rows.push(MenuSection::Edit, placeholder_row());

        // 第一行上面不画线；同段两行之间不画线；换段画一条。
        assert_eq!(rows.rows.len(), 5);
        assert_eq!(open, CONTEXT_MENU_PADDING);
        assert_eq!(open_with, open + step);
        assert_eq!(
            copy,
            open_with + step + SECTION_DIVIDER_HEIGHT + CONTEXT_MENU_ITEM_SPACING
        );
        assert_eq!(move_to, copy + step);
    }

    #[test]
    fn items_split_apart_by_the_user_form_separate_sections() {
        let mut rows = MenuRows::new();
        rows.push(MenuSection::Edit, placeholder_row());
        rows.push(MenuSection::Delete, placeholder_row());
        rows.push(MenuSection::Edit, placeholder_row());

        assert_eq!(rows.rows.len(), 5);
    }

    #[test]
    fn a_submenu_starts_level_with_the_row_that_opened_it() {
        let mut rows = MenuRows::new();
        rows.push(MenuSection::Open, placeholder_row());
        let tools = rows.push(MenuSection::Edit, placeholder_row());

        // 子菜单和主菜单内边距相同：子菜单顶边 + 内边距 = 它第一项的顶边。
        assert_eq!(submenu_top(tools) + CONTEXT_MENU_PADDING, tools);
    }
}
