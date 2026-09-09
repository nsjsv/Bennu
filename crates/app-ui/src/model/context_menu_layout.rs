//! 右键菜单的用户自定义布局:每个菜单一份有序条目列表(项 + 可见性)。
//!
//! 该模块是菜单项顺序与可见性的单一事实来源:菜单渲染(floating_panels)、
//! ContextMenuState 构造边界的空菜单判定、设置页展示与编辑都从这里取数据。

use super::context_menu_items::*;
use crate::icons::IconSymbol;
use iced::Point;
use crate::model::{ListColumnKind, SearchEntryTypePreset};
use crate::network_connections::SidebarNetworkConnectionAction;
use crate::sidebar_devices::SidebarDeviceAction;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextMenuEntry<I> {
    pub(crate) item: I,
    pub(crate) visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMenuLayout<I> {
    pub(crate) entries: Vec<ContextMenuEntry<I>>,
}

impl<I: Copy + PartialEq> ContextMenuLayout<I> {
    pub(crate) fn all_visible(items: impl IntoIterator<Item = I>) -> Self {
        Self {
            entries: items
                .into_iter()
                .map(|item| ContextMenuEntry {
                    item,
                    visible: true,
                })
                .collect(),
        }
    }

    /// 从存储字符串还原;未知 id 丢弃、缺失项按全集顺序追加且默认可见、重复 id 去重。
    pub(crate) fn normalized_from_stored(
        stored: &[(String, bool)],
        all: &[I],
        parse: impl Fn(&str) -> Option<I>,
    ) -> Self {
        let mut entries = Vec::new();
        for (id, visible) in stored {
            let Some(item) = parse(id) else {
                continue;
            };
            if entries
                .iter()
                .any(|entry: &ContextMenuEntry<I>| entry.item == item)
            {
                continue;
            }
            entries.push(ContextMenuEntry {
                item,
                visible: *visible,
            });
        }
        for item in all {
            if !entries
                .iter()
                .any(|entry: &ContextMenuEntry<I>| entry.item == *item)
            {
                entries.push(ContextMenuEntry {
                    item: *item,
                    visible: true,
                });
            }
        }
        Self { entries }
    }

    pub(crate) fn to_config_values(
        &self,
        config_value: impl Fn(I) -> &'static str,
    ) -> Vec<(String, bool)> {
        self.entries
            .iter()
            .map(|entry| (config_value(entry.item).to_owned(), entry.visible))
            .collect()
    }

    /// 配置后的有序可见项;eligible 合并运行时条件(仅文件/可用动作等)。
    pub(crate) fn ordered_visible_where(&self, eligible: impl Fn(&I) -> bool) -> Vec<I> {
        self.entries
            .iter()
            .filter(|entry| entry.visible && eligible(&entry.item))
            .map(|entry| entry.item)
            .collect()
    }

    pub(crate) fn toggle(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.visible = !entry.visible;
        }
    }

    /// 拖拽换位:把 from 位置的项移动到 to 位置(其余项顺移)。
    pub(crate) fn reordered(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
    }
}

/// 全部可配置菜单的布局集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMenuPreferences {
    pub(crate) file_entry: ContextMenuLayout<FileAreaMenuItem>,
    pub(crate) file_blank: ContextMenuLayout<FileAreaMenuItem>,
    pub(crate) trash: ContextMenuLayout<TrashMenuItem>,
    pub(crate) search: ContextMenuLayout<SearchResultMenuItem>,
    pub(crate) search_entry_types: ContextMenuLayout<SearchEntryTypePreset>,
    pub(crate) list_columns: ContextMenuLayout<ListColumnKind>,
    pub(crate) sidebar_bookmark: ContextMenuLayout<BookmarkMenuItem>,
    pub(crate) sidebar_device: ContextMenuLayout<SidebarDeviceAction>,
    pub(crate) network_connection: ContextMenuLayout<SidebarNetworkConnectionAction>,
}

/// 菜单布局的存储无关形态:9 页 × 有序 (id, visible)。由 config 层与 Stored 结构互转。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ContextMenuLayoutConfigValues {
    pub(crate) file_entry: Vec<(String, bool)>,
    pub(crate) file_blank: Vec<(String, bool)>,
    pub(crate) trash: Vec<(String, bool)>,
    pub(crate) search: Vec<(String, bool)>,
    pub(crate) search_entry_types: Vec<(String, bool)>,
    pub(crate) list_columns: Vec<(String, bool)>,
    pub(crate) sidebar_bookmark: Vec<(String, bool)>,
    pub(crate) sidebar_device: Vec<(String, bool)>,
    pub(crate) network_connection: Vec<(String, bool)>,
}

impl ContextMenuPreferences {
    pub(crate) fn defaults() -> Self {
        Self {
            file_entry: ContextMenuLayout::all_visible(FILE_ENTRY_MENU_ITEMS),
            file_blank: ContextMenuLayout::all_visible(FILE_BLANK_MENU_ITEMS),
            trash: ContextMenuLayout::all_visible(TRASH_MENU_ITEMS),
            search: ContextMenuLayout::all_visible(SEARCH_RESULT_MENU_ITEMS),
            search_entry_types: ContextMenuLayout::all_visible(SearchEntryTypePreset::MORE),
            list_columns: ContextMenuLayout::all_visible(ListColumnKind::ALL),
            sidebar_bookmark: ContextMenuLayout::all_visible(BOOKMARK_MENU_ITEMS),
            sidebar_device: ContextMenuLayout::all_visible(
                device_action_config_values::DEVICE_MENU_ITEMS,
            ),
            network_connection: ContextMenuLayout::all_visible(
                network_action_config_values::NETWORK_MENU_ITEMS,
            ),
        }
    }

    pub(crate) fn to_config_values(&self) -> ContextMenuLayoutConfigValues {
        ContextMenuLayoutConfigValues {
            file_entry: self.file_entry.to_config_values(|item| item.config_value()),
            file_blank: self.file_blank.to_config_values(|item| item.config_value()),
            trash: self.trash.to_config_values(|item| item.config_value()),
            search: self.search.to_config_values(|item| item.config_value()),
            search_entry_types: self
                .search_entry_types
                .to_config_values(search_entry_type_config_value),
            list_columns: self
                .list_columns
                .to_config_values(|kind| kind.config_value()),
            sidebar_bookmark: self
                .sidebar_bookmark
                .to_config_values(|item| item.config_value()),
            sidebar_device: self
                .sidebar_device
                .to_config_values(device_action_config_values::device_config_value),
            network_connection: self
                .network_connection
                .to_config_values(network_action_config_values::network_config_value),
        }
    }

    pub(crate) fn from_config_values(values: &ContextMenuLayoutConfigValues) -> Self {
        Self {
            file_entry: ContextMenuLayout::normalized_from_stored(
                &values.file_entry,
                &FILE_ENTRY_MENU_ITEMS,
                FileAreaMenuItem::from_config_value,
            ),
            file_blank: ContextMenuLayout::normalized_from_stored(
                &values.file_blank,
                &FILE_BLANK_MENU_ITEMS,
                FileAreaMenuItem::from_config_value,
            ),
            trash: ContextMenuLayout::normalized_from_stored(
                &values.trash,
                &TRASH_MENU_ITEMS,
                TrashMenuItem::from_config_value,
            ),
            search: ContextMenuLayout::normalized_from_stored(
                &values.search,
                &SEARCH_RESULT_MENU_ITEMS,
                SearchResultMenuItem::from_config_value,
            ),
            search_entry_types: ContextMenuLayout::normalized_from_stored(
                &values.search_entry_types,
                &SearchEntryTypePreset::MORE,
                search_entry_type_from_config_value,
            ),
            list_columns: ContextMenuLayout::normalized_from_stored(
                &values.list_columns,
                &ListColumnKind::ALL,
                ListColumnKind::from_config_value,
            ),
            sidebar_bookmark: ContextMenuLayout::normalized_from_stored(
                &values.sidebar_bookmark,
                &BOOKMARK_MENU_ITEMS,
                BookmarkMenuItem::from_config_value,
            ),
            sidebar_device: ContextMenuLayout::normalized_from_stored(
                &values.sidebar_device,
                &device_action_config_values::DEVICE_MENU_ITEMS,
                device_action_config_values::device_from_config_value,
            ),
            network_connection: ContextMenuLayout::normalized_from_stored(
                &values.network_connection,
                &network_action_config_values::NETWORK_MENU_ITEMS,
                network_action_config_values::network_from_config_value,
            ),
        }
    }

    // ---- 菜单渲染取数(也是构造边界空菜单判定的唯一来源) ----

    pub(crate) fn file_entry_items(
        &self,
        target_is_directory: bool,
        can_batch_rename: bool,
        can_create_symlink: bool,
    ) -> Vec<FileAreaMenuItem> {
        self.file_entry.ordered_visible_where(|item| match item {
            FileAreaMenuItem::FileChecksum => !target_is_directory,
            FileAreaMenuItem::BatchRename => can_batch_rename,
            FileAreaMenuItem::CreateSymlink => can_create_symlink,
            _ => true,
        })
    }

    pub(crate) fn file_blank_items(&self) -> Vec<FileAreaMenuItem> {
        self.file_blank.ordered_visible_where(|_| true)
    }

    pub(crate) fn trash_items(&self, has_target: bool) -> Vec<TrashMenuItem> {
        self.trash.ordered_visible_where(|item| match item {
            TrashMenuItem::Restore
            | TrashMenuItem::DeletePermanently
            | TrashMenuItem::Properties => has_target,
            TrashMenuItem::EmptyTrash => true,
        })
    }

    pub(crate) fn search_items(&self) -> Vec<SearchResultMenuItem> {
        self.search.ordered_visible_where(|_| true)
    }

    pub(crate) fn search_entry_type_items(&self) -> Vec<SearchEntryTypePreset> {
        self.search_entry_types.ordered_visible_where(|_| true)
    }

    /// Name 列沿用既有 Required 语义,无论配置如何恒出现在菜单里。
    pub(crate) fn list_column_items(&self) -> Vec<ListColumnKind> {
        self.list_columns
            .entries
            .iter()
            .filter(|entry| entry.visible || entry.item == ListColumnKind::Name)
            .map(|entry| entry.item)
            .collect()
    }

    pub(crate) fn sidebar_bookmark_items(&self) -> Vec<BookmarkMenuItem> {
        self.sidebar_bookmark.ordered_visible_where(|_| true)
    }

    pub(crate) fn sidebar_device_items(
        &self,
        available_actions: impl IntoIterator<Item = SidebarDeviceAction>,
    ) -> Vec<SidebarDeviceAction> {
        let available: Vec<_> = available_actions.into_iter().collect();
        self.sidebar_device
            .ordered_visible_where(|action| available.contains(action))
    }

    pub(crate) fn network_connection_items(
        &self,
        available_actions: impl IntoIterator<Item = SidebarNetworkConnectionAction>,
    ) -> Vec<SidebarNetworkConnectionAction> {
        let available: Vec<_> = available_actions.into_iter().collect();
        self.network_connection
            .ordered_visible_where(|action| available.contains(action))
    }
}

/// 设置页翻页器的页序(与翻页展示顺序一致)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMenuSettingsPage {
    FileEntry,
    FileBlank,
    Trash,
    Search,
    SearchEntryTypes,
    ListColumns,
    SidebarBookmark,
    SidebarDevice,
    NetworkConnection,
}

pub(crate) const CONTEXT_MENU_SETTINGS_PAGES: [ContextMenuSettingsPage; 9] = [
    ContextMenuSettingsPage::FileEntry,
    ContextMenuSettingsPage::FileBlank,
    ContextMenuSettingsPage::Trash,
    ContextMenuSettingsPage::Search,
    ContextMenuSettingsPage::SearchEntryTypes,
    ContextMenuSettingsPage::ListColumns,
    ContextMenuSettingsPage::SidebarBookmark,
    ContextMenuSettingsPage::SidebarDevice,
    ContextMenuSettingsPage::NetworkConnection,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMenuSettingsPageStep {
    Previous,
    Next,
}

/// 设置列表的相邻行中心距:行内容高 24 + 行间距 4×2 + 分隔线 1,
/// 与菜单预览面板的实际渲染保持一致,拖拽换位与被拖行位移都按它折算。
pub(crate) const CONTEXT_MENU_SETTINGS_ROW_PITCH: f32 = 33.0;

/// 「右键菜单」设置列表的拖拽状态。origin 在首次移动时记录
/// (设置窗口的指针位置不写共享 cursor_position)。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextMenuSettingsDragState {
    pub(crate) page: ContextMenuSettingsPage,
    /// 拖拽起始行号,换位目标由它 + 光标纵向偏移折算。
    pub(crate) source_index: usize,
    /// 被拖行当前所在行号,随每次换位更新。
    pub(crate) current_index: usize,
    pub(crate) origin: Option<Point>,
    pub(crate) latest: Option<Point>,
    pub(crate) order_changed: bool,
}

impl ContextMenuSettingsDragState {
    pub(crate) fn new(page: ContextMenuSettingsPage, source_index: usize) -> Self {
        Self {
            page,
            source_index,
            current_index: source_index,
            origin: None,
            latest: None,
            order_changed: false,
        }
    }
}

/// 设置页列表的页无关行模型:异构布局类型被封装在 ContextMenuPreferences 内部。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMenuSettingsRow {
    pub(crate) label: &'static str,
    pub(crate) icon: IconSymbol,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
}

impl ContextMenuSettingsPage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FileEntry => "File entry menu",
            Self::FileBlank => "Blank area menu",
            Self::Trash => "Trash menu",
            Self::Search => "Search results menu",
            Self::SearchEntryTypes => "Search type filters",
            Self::ListColumns => "List columns menu",
            Self::SidebarBookmark => "Favorites menu",
            Self::SidebarDevice => "Devices menu",
            Self::NetworkConnection => "Network menu",
        }
    }

    pub(crate) fn stepped(self, step: ContextMenuSettingsPageStep) -> Self {
        let position = CONTEXT_MENU_SETTINGS_PAGES
            .iter()
            .position(|page| *page == self)
            .unwrap_or(0);
        let count = CONTEXT_MENU_SETTINGS_PAGES.len();
        let next = match step {
            ContextMenuSettingsPageStep::Previous => (position + count - 1) % count,
            ContextMenuSettingsPageStep::Next => (position + 1) % count,
        };
        CONTEXT_MENU_SETTINGS_PAGES[next]
    }
}

impl ContextMenuPreferences {
    pub(crate) fn settings_rows(
        &self,
        page: ContextMenuSettingsPage,
    ) -> Vec<ContextMenuSettingsRow> {
        match page {
            ContextMenuSettingsPage::FileEntry => {
                settings_rows_for(&self.file_entry, FileAreaMenuItem::label, FileAreaMenuItem::icon)
            }
            ContextMenuSettingsPage::FileBlank => {
                settings_rows_for(&self.file_blank, FileAreaMenuItem::label, FileAreaMenuItem::icon)
            }
            ContextMenuSettingsPage::Trash => {
                settings_rows_for(&self.trash, TrashMenuItem::label, TrashMenuItem::icon)
            }
            ContextMenuSettingsPage::Search => {
                settings_rows_for(&self.search, SearchResultMenuItem::label, SearchResultMenuItem::icon)
            }
            ContextMenuSettingsPage::SearchEntryTypes => {
                // 该菜单本体是勾选行,无图标;设置页用占位图标,视图层按页渲染勾选框。
                settings_rows_for(
                    &self.search_entry_types,
                    SearchEntryTypePreset::label,
                    |_| IconSymbol::File,
                )
            },
            ContextMenuSettingsPage::ListColumns => self
                .list_columns
                .entries
                .iter()
                .map(|entry| ContextMenuSettingsRow {
                    label: entry.item.label(),
                    icon: IconSymbol::List,
                    visible: entry.visible || entry.item == ListColumnKind::Name,
                    locked: entry.item == ListColumnKind::Name,
                })
                .collect(),
            ContextMenuSettingsPage::SidebarBookmark => {
                settings_rows_for(&self.sidebar_bookmark, BookmarkMenuItem::label, BookmarkMenuItem::icon)
            }
            ContextMenuSettingsPage::SidebarDevice => self
                .sidebar_device
                .entries
                .iter()
                .map(|entry| ContextMenuSettingsRow {
                    label: device_settings_label(entry.item),
                    icon: IconSymbol::HardDrive,
                    visible: entry.visible,
                    locked: false,
                })
                .collect(),
            ContextMenuSettingsPage::NetworkConnection => self
                .network_connection
                .entries
                .iter()
                .map(|entry| ContextMenuSettingsRow {
                    label: entry.item.label(),
                    icon: network_action_config_values::network_icon(entry.item),
                    visible: entry.visible,
                    locked: false,
                })
                .collect(),
        }
    }

    pub(crate) fn toggle_settings_row(
        &mut self,
        page: ContextMenuSettingsPage,
        index: usize,
    ) {
        if page == ContextMenuSettingsPage::ListColumns {
            if self.list_columns.entries.get(index).is_some_and(|entry| {
                entry.item == ListColumnKind::Name
            }) {
                return;
            }
            self.list_columns.toggle(index);
            return;
        }
        match page {
            ContextMenuSettingsPage::FileEntry => self.file_entry.toggle(index),
            ContextMenuSettingsPage::FileBlank => self.file_blank.toggle(index),
            ContextMenuSettingsPage::Trash => self.trash.toggle(index),
            ContextMenuSettingsPage::Search => self.search.toggle(index),
            ContextMenuSettingsPage::SearchEntryTypes => self.search_entry_types.toggle(index),
            ContextMenuSettingsPage::SidebarBookmark => self.sidebar_bookmark.toggle(index),
            ContextMenuSettingsPage::SidebarDevice => self.sidebar_device.toggle(index),
            ContextMenuSettingsPage::NetworkConnection => self.network_connection.toggle(index),
            ContextMenuSettingsPage::ListColumns => unreachable!(),
        }
    }

    pub(crate) fn reorder_settings_row(
        &mut self,
        page: ContextMenuSettingsPage,
        from: usize,
        to: usize,
    ) {
        match page {
            ContextMenuSettingsPage::FileEntry => self.file_entry.reordered(from, to),
            ContextMenuSettingsPage::FileBlank => self.file_blank.reordered(from, to),
            ContextMenuSettingsPage::Trash => self.trash.reordered(from, to),
            ContextMenuSettingsPage::Search => self.search.reordered(from, to),
            ContextMenuSettingsPage::SearchEntryTypes => self.search_entry_types.reordered(from, to),
            ContextMenuSettingsPage::ListColumns => self.list_columns.reordered(from, to),
            ContextMenuSettingsPage::SidebarBookmark => self.sidebar_bookmark.reordered(from, to),
            ContextMenuSettingsPage::SidebarDevice => self.sidebar_device.reordered(from, to),
            ContextMenuSettingsPage::NetworkConnection => {
                self.network_connection.reordered(from, to)
            }
        }
    }

    pub(crate) fn reset_settings_page(&mut self, page: ContextMenuSettingsPage) {
        match page {
            ContextMenuSettingsPage::FileEntry => {
                self.file_entry = ContextMenuLayout::all_visible(FILE_ENTRY_MENU_ITEMS)
            }
            ContextMenuSettingsPage::FileBlank => {
                self.file_blank = ContextMenuLayout::all_visible(FILE_BLANK_MENU_ITEMS)
            }
            ContextMenuSettingsPage::Trash => {
                self.trash = ContextMenuLayout::all_visible(TRASH_MENU_ITEMS)
            }
            ContextMenuSettingsPage::Search => {
                self.search = ContextMenuLayout::all_visible(SEARCH_RESULT_MENU_ITEMS)
            }
            ContextMenuSettingsPage::SearchEntryTypes => {
                self.search_entry_types = ContextMenuLayout::all_visible(SearchEntryTypePreset::MORE)
            }
            ContextMenuSettingsPage::ListColumns => {
                self.list_columns = ContextMenuLayout::all_visible(ListColumnKind::ALL)
            }
            ContextMenuSettingsPage::SidebarBookmark => {
                self.sidebar_bookmark = ContextMenuLayout::all_visible(BOOKMARK_MENU_ITEMS)
            }
            ContextMenuSettingsPage::SidebarDevice => {
                self.sidebar_device =
                    ContextMenuLayout::all_visible(device_action_config_values::DEVICE_MENU_ITEMS)
            }
            ContextMenuSettingsPage::NetworkConnection => {
                self.network_connection = ContextMenuLayout::all_visible(
                    network_action_config_values::NETWORK_MENU_ITEMS,
                )
            }
        }
    }
}

fn settings_rows_for<I: Copy + PartialEq>(
    layout: &ContextMenuLayout<I>,
    label: impl Fn(I) -> &'static str,
    icon: impl Fn(I) -> IconSymbol,
) -> Vec<ContextMenuSettingsRow> {
    layout
        .entries
        .iter()
        .map(|entry| ContextMenuSettingsRow {
            label: label(entry.item),
            icon: icon(entry.item),
            visible: entry.visible,
            locked: false,
        })
        .collect()
}

fn device_settings_label(action: SidebarDeviceAction) -> &'static str {
    match action {
        SidebarDeviceAction::Mount => "Mount",
        SidebarDeviceAction::Unmount => "Unmount",
        SidebarDeviceAction::Eject => "Eject",
    }
}

