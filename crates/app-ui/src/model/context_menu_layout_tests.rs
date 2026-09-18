use super::context_menu_items::*;
use super::context_menu_layout::*;
use crate::icons::IconSymbol;

use super::*;

#[test]
fn defaults_match_existing_menu_structure() {
    let preferences = ContextMenuPreferences::defaults();
    // 一级结构:组锚点折叠成员,其余保持全集原序;工具组插在移动和粘贴之间。
    assert_eq!(
        preferences.file_entry_menu_entries(false, true, true),
        vec![
            FileEntryMenuEntry::Item(FileAreaMenuItem::Open),
            FileEntryMenuEntry::Item(FileAreaMenuItem::SmartExtractHere),
            FileEntryMenuEntry::Item(FileAreaMenuItem::ExtractToArchiveFolder),
            FileEntryMenuEntry::Item(FileAreaMenuItem::OpenWith),
            FileEntryMenuEntry::Group {
                anchor: FileAreaMenuItem::Copy,
                members: vec![FileAreaMenuItem::Duplicate, FileAreaMenuItem::CopyPath],
            },
            FileEntryMenuEntry::Item(FileAreaMenuItem::Move),
            FileEntryMenuEntry::Group {
                anchor: FileAreaMenuItem::Tools,
                members: vec![
                    FileAreaMenuItem::CreateArchive,
                    FileAreaMenuItem::ConvertFormat,
                    FileAreaMenuItem::FileChecksum,
                    FileAreaMenuItem::CreateSymlink,
                ],
            },
            FileEntryMenuEntry::Item(FileAreaMenuItem::Paste),
            FileEntryMenuEntry::Group {
                anchor: FileAreaMenuItem::Rename,
                members: vec![FileAreaMenuItem::BatchRename],
            },
            FileEntryMenuEntry::Group {
                anchor: FileAreaMenuItem::NewEntry,
                members: vec![FileAreaMenuItem::NewFolderFromSelection],
            },
            FileEntryMenuEntry::Item(FileAreaMenuItem::OpenTerminalHere),
            FileEntryMenuEntry::Item(FileAreaMenuItem::Delete),
            FileEntryMenuEntry::Item(FileAreaMenuItem::Properties),
        ]
    );
}

#[test]
fn runtime_eligibility_gates_members_and_degrades_groups() {
    let preferences = ContextMenuPreferences::defaults();
    // 目录:工具子菜单无文件校验。
    let tools_members = |target_is_directory: bool, can_create_symlink: bool| {
        preferences
            .file_entry_menu_entries(target_is_directory, true, can_create_symlink)
            .iter()
            .find_map(|entry| match entry {
                FileEntryMenuEntry::Group {
                    anchor: FileAreaMenuItem::Tools,
                    members,
                } => Some(members.clone()),
                _ => None,
            })
            .unwrap()
    };
    assert!(!tools_members(true, true).contains(&FileAreaMenuItem::FileChecksum));
    // 远程挂载:工具子菜单无创建符号链接。
    assert!(!tools_members(false, false).contains(&FileAreaMenuItem::CreateSymlink));
    // 不可批量重命名:重命名组退化为普通行。
    let entries = preferences.file_entry_menu_entries(false, false, true);
    assert!(entries.contains(&FileEntryMenuEntry::Item(FileAreaMenuItem::Rename)));
    assert!(!entries.iter().any(|entry| matches!(
        entry,
        FileEntryMenuEntry::Group {
            anchor: FileAreaMenuItem::Rename,
            ..
        }
    )));
}

#[test]
fn selection_only_items_stay_out_of_the_blank_area_menu() {
    // 复制副本 / 收纳文件夹 / 复制路径 / 创建符号链接都作用于选中条目,
    // 空白处右键没有选中项,不提供这些条目。
    for item in FILE_BLANK_MENU_ITEMS {
        assert!(!matches!(
            item,
            FileAreaMenuItem::Duplicate
                | FileAreaMenuItem::NewFolderFromSelection
                | FileAreaMenuItem::CopyPath
                | FileAreaMenuItem::CreateSymlink
        ));
    }
}

#[test]
fn new_entry_position_tracks_visibility() {
    let mut preferences = ContextMenuPreferences::defaults();
    // 隐藏前 5 项(Open/两个解压项/OpenWith/Copy);组行随锚点一起消失。
    for index in 0..5 {
        preferences.file_entry.toggle(index);
    }
    let entries = preferences.file_entry_menu_entries(false, false, true);
    let new_entry_index = entries
        .iter()
        .position(|entry| {
            matches!(
                entry,
                FileEntryMenuEntry::Group {
                    anchor: FileAreaMenuItem::NewEntry,
                    ..
                }
            )
        })
        .unwrap();
    // 前面剩移动、工具组、粘贴、重命名组共 4 行。
    assert_eq!(new_entry_index, 4);
}

#[test]
fn hidden_group_anchor_hides_the_whole_group() {
    let mut preferences = ContextMenuPreferences::defaults();
    let copy_index = preferences
        .file_entry
        .entries
        .iter()
        .position(|entry| entry.item == FileAreaMenuItem::Copy)
        .unwrap();
    preferences.file_entry.toggle(copy_index);
    let entries = preferences.file_entry_menu_entries(false, true, true);
    assert!(!entries.iter().any(|entry| matches!(
        entry,
        FileEntryMenuEntry::Group {
            anchor: FileAreaMenuItem::Copy,
            ..
        }
    )));
    // 成员也不作为一级行出现。
    assert!(!entries.iter().any(|entry| matches!(
        entry,
        FileEntryMenuEntry::Item(FileAreaMenuItem::Duplicate)
            | FileEntryMenuEntry::Item(FileAreaMenuItem::CopyPath)
    )));
}

#[test]
fn empty_member_lists_degrade_or_omit_group_rows() {
    let mut preferences = ContextMenuPreferences::defaults();
    let toggle_item = |preferences: &mut ContextMenuPreferences, member: FileAreaMenuItem| {
        let index = preferences
            .file_entry
            .entries
            .iter()
            .position(|entry| entry.item == member)
            .unwrap();
        preferences.file_entry.toggle(index);
    };
    // 复制组成员全部隐藏 → 复制退化为普通行(单点动作仍在)。
    toggle_item(&mut preferences, FileAreaMenuItem::Duplicate);
    toggle_item(&mut preferences, FileAreaMenuItem::CopyPath);
    let entries = preferences.file_entry_menu_entries(false, true, true);
    assert!(entries.contains(&FileEntryMenuEntry::Item(FileAreaMenuItem::Copy)));
    // 工具组成员全部隐藏 → 纯触发器整行省略。
    for member in [
        FileAreaMenuItem::CreateArchive,
        FileAreaMenuItem::ConvertFormat,
        FileAreaMenuItem::FileChecksum,
        FileAreaMenuItem::CreateSymlink,
    ] {
        toggle_item(&mut preferences, member);
    }
    let entries = preferences.file_entry_menu_entries(false, true, true);
    assert!(!entries.iter().any(|entry| matches!(
        entry,
        FileEntryMenuEntry::Group {
            anchor: FileAreaMenuItem::Tools,
            ..
        }
    )));
    // 「新建...」硬编码行恒在:成员隐藏也保持组行。
    toggle_item(&mut preferences, FileAreaMenuItem::NewFolderFromSelection);
    let entries = preferences.file_entry_menu_entries(false, true, true);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        FileEntryMenuEntry::Group {
            anchor: FileAreaMenuItem::NewEntry,
            members
        } if members.is_empty()
    )));
}

#[test]
fn all_hidden_menus_report_empty() {
    let mut preferences = ContextMenuPreferences::defaults();
    let count = preferences.search.entries.len();
    for index in 0..count {
        preferences.search.toggle(index);
    }
    assert!(preferences.search_items().is_empty());
}

#[test]
fn normalized_from_stored_appends_missing_and_drops_unknown() {
    let stored = vec![
        ("copy".to_owned(), true),
        ("bogus".to_owned(), false),
        ("open".to_owned(), false),
        ("copy".to_owned(), false),
    ];
    let layout = ContextMenuLayout::<FileAreaMenuItem>::normalized_from_stored(
        &stored,
        &FILE_ENTRY_MENU_ITEMS,
        FileAreaMenuItem::from_config_value,
    );
    assert_eq!(layout.entries[0].item, FileAreaMenuItem::Copy);
    assert!(layout.entries[0].visible);
    assert_eq!(layout.entries[1].item, FileAreaMenuItem::Open);
    assert!(!layout.entries[1].visible);
    // 缺失项追加且可见;总数等于全集。
    assert_eq!(layout.entries.len(), FILE_ENTRY_MENU_ITEMS.len());
    assert!(layout
        .entries
        .iter()
        .skip(2)
        .all(|entry| entry.visible));
}

#[test]
fn stored_layout_without_tools_appends_it_visible_at_the_end() {
    // 老配置没有 tools 条目:按全集顺序追加到末尾且可见,其余保持存储顺序。
    let old_items = [
        FileAreaMenuItem::Open,
        FileAreaMenuItem::OpenWith,
        FileAreaMenuItem::Copy,
        FileAreaMenuItem::Duplicate,
        FileAreaMenuItem::Move,
        FileAreaMenuItem::CreateArchive,
        FileAreaMenuItem::ConvertFormat,
        FileAreaMenuItem::FileChecksum,
        FileAreaMenuItem::Paste,
        FileAreaMenuItem::Rename,
        FileAreaMenuItem::BatchRename,
        FileAreaMenuItem::NewEntry,
        FileAreaMenuItem::NewFolderFromSelection,
        FileAreaMenuItem::OpenTerminalHere,
        FileAreaMenuItem::CopyPath,
        FileAreaMenuItem::CreateSymlink,
        FileAreaMenuItem::Delete,
        FileAreaMenuItem::Properties,
    ];
    let stored: Vec<(String, bool)> = old_items
        .iter()
        .map(|item| (item.config_value().to_owned(), true))
        .collect();
    let layout = ContextMenuLayout::<FileAreaMenuItem>::normalized_from_stored(
        &stored,
        &FILE_ENTRY_MENU_ITEMS,
        FileAreaMenuItem::from_config_value,
    );
    assert_eq!(layout.entries.len(), FILE_ENTRY_MENU_ITEMS.len());
    let last = layout.entries.last().unwrap();
    assert_eq!(last.item, FileAreaMenuItem::Tools);
    assert!(last.visible);
}

#[test]
fn file_entry_settings_rows_project_groups_to_single_rows() {
    let preferences = ContextMenuPreferences::defaults();
    let rows = preferences.settings_rows(ContextMenuSettingsPage::FileEntry);
    // 21 项中 8 个成员被收进组面板,顶级行 = 13 行。
    assert_eq!(rows.len(), 13);
    let group_anchors: Vec<_> = rows.iter().filter_map(|row| row.group_anchor).collect();
    assert_eq!(
        group_anchors,
        vec![
            FileAreaMenuItem::Copy,
            FileAreaMenuItem::Tools,
            FileAreaMenuItem::Rename,
            FileAreaMenuItem::NewEntry,
        ]
    );
    // entry_index 指向布局真实下标。
    assert_eq!(rows[0].entry_index, 0);
    assert_eq!(rows[4].entry_index, 4);
    // 成员面板行带布局下标,按成员表顺序。
    let members = preferences.file_entry_settings_member_rows(FileAreaMenuItem::Tools);
    assert_eq!(members.len(), 4);
    assert_eq!(members[0].label, "Create Archive...");
    assert_eq!(members[3].label, "Create Symbolic Link");
}

#[test]
fn file_entry_drag_moves_the_anchor_entry_only() {
    let mut preferences = ContextMenuPreferences::defaults();
    // 复制组锚点在布局下标 4;拖到粘贴(下标 11)之后:
    // 锚点条目换位,成员条目留在原下标。
    preferences.reorder_settings_row(ContextMenuSettingsPage::FileEntry, 4, 11);
    let rows = preferences.settings_rows(ContextMenuSettingsPage::FileEntry);
    assert_eq!(rows[11].label, "Copy");
    // 锚点移走后,原下标 4 由成员 Duplicate 顶上;CopyPath 前移到 16。
    assert_eq!(
        preferences.file_entry.entries[4].item,
        FileAreaMenuItem::Duplicate
    );
    assert_eq!(
        preferences.file_entry.entries[16].item,
        FileAreaMenuItem::CopyPath
    );
}

#[test]
fn list_columns_keep_name_visible() {
    let mut preferences = ContextMenuPreferences::defaults();
    let name_index = preferences
        .list_columns
        .entries
        .iter()
        .position(|entry| entry.item == ListColumnKind::Name)
        .unwrap();
    preferences.toggle_settings_row(ContextMenuSettingsPage::ListColumns, name_index);
    assert!(preferences
        .list_column_items()
        .contains(&ListColumnKind::Name));
    assert_eq!(
        preferences
            .settings_rows(ContextMenuSettingsPage::ListColumns)[name_index],
        ContextMenuSettingsRow {
            label: "Name",
            icon: IconSymbol::List,
            visible: true,
            locked: true,
            entry_index: name_index,
            group_anchor: None,
        }
    );
}

#[test]
fn reorder_settings_row_moves_and_clamps() {
    let mut preferences = ContextMenuPreferences::defaults();
    preferences.reorder_settings_row(ContextMenuSettingsPage::Search, 1, 0);
    assert_eq!(
        preferences.search.entries[0].item,
        SearchResultMenuItem::Copy
    );
    // 同位移动与越界目标都是空操作。
    preferences.reorder_settings_row(ContextMenuSettingsPage::Search, 0, 0);
    preferences.reorder_settings_row(
        ContextMenuSettingsPage::Search,
        0,
        preferences.search.entries.len(),
    );
    assert_eq!(
        preferences.search.entries[0].item,
        SearchResultMenuItem::Copy
    );
}

#[test]
fn settings_page_steps_wrap_around() {
    assert_eq!(
        ContextMenuSettingsPage::FileEntry.stepped(ContextMenuSettingsPageStep::Previous),
        ContextMenuSettingsPage::NetworkConnection
    );
    assert_eq!(
        ContextMenuSettingsPage::NetworkConnection.stepped(ContextMenuSettingsPageStep::Next),
        ContextMenuSettingsPage::FileEntry
    );
}

#[test]
fn config_values_round_trip() {
    let mut preferences = ContextMenuPreferences::defaults();
    preferences.trash.toggle(0);
    preferences.reorder_settings_row(ContextMenuSettingsPage::Trash, 1, 0);
    let restored =
        ContextMenuPreferences::from_config_values(&preferences.to_config_values());
    assert_eq!(preferences, restored);
}
