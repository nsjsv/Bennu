use super::context_menu_items::*;
use super::context_menu_layout::*;
use crate::icons::IconSymbol;

use super::*;

#[test]
fn defaults_match_existing_menu_order() {
    let preferences = ContextMenuPreferences::defaults();
    assert_eq!(
        preferences.file_entry_items(false, true, true),
        vec![
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
        ]
    );
    // 目录:无 FileChecksum。
    assert!(!preferences
        .file_entry_items(true, true, true)
        .contains(&FileAreaMenuItem::FileChecksum));
    // 不可批量重命名:无 BatchRename。
    assert!(!preferences
        .file_entry_items(false, false, false)
        .contains(&FileAreaMenuItem::BatchRename));
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
fn symlink_item_gates_on_local_mount_clearance() {
    let preferences = ContextMenuPreferences::defaults();
    // 全本地时文件与目录都提供创建符号链接;远程挂载选中时隐藏。
    assert!(preferences
        .file_entry_items(false, true, true)
        .contains(&FileAreaMenuItem::CreateSymlink));
    assert!(preferences
        .file_entry_items(true, true, true)
        .contains(&FileAreaMenuItem::CreateSymlink));
    assert!(!preferences
        .file_entry_items(false, true, false)
        .contains(&FileAreaMenuItem::CreateSymlink));
    assert!(!preferences
        .file_entry_items(true, true, false)
        .contains(&FileAreaMenuItem::CreateSymlink));
}

#[test]
fn new_entry_position_tracks_visibility() {
    let mut preferences = ContextMenuPreferences::defaults();
    for index in 0..3 {
        preferences.file_entry.toggle(index);
    }
    let items = preferences.file_entry_items(false, false, false);
    assert_eq!(items[0], FileAreaMenuItem::Duplicate);
    // 隐藏 Open/OpenWith/Copy 后,NewEntry 前剩 Duplicate、Move、CreateArchive、
    // ConvertFormat、FileChecksum、Paste、Rename 共 7 行。
    assert_eq!(
        items.iter().position(|item| *item == FileAreaMenuItem::NewEntry),
        Some(7)
    );
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
