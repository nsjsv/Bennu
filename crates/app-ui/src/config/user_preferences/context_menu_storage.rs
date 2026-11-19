//! 右键菜单布局和存储形态之间的转换。

use super::super::unrecognized_settings::UnrecognizedSettings;
use super::super::{
    StoredContextMenuItemEntry, StoredContextMenuLayout, StoredContextMenuLayouts,
    StoredUserPreferences,
};
use super::UserPreferences;
use crate::model::{ContextMenuLayoutConfigValues, ContextMenuPreferences};

pub(super) fn stored_context_menu_layouts(
    preferences: &ContextMenuPreferences,
) -> StoredContextMenuLayouts {
    let values = preferences.to_config_values();
    let layout = |items: &Vec<(String, bool)>| StoredContextMenuLayout {
        items: items
            .iter()
            .map(|(id, visible)| StoredContextMenuItemEntry {
                id: id.clone(),
                visible: *visible,
            })
            .collect(),
    };
    StoredContextMenuLayouts {
        file_entry: layout(&values.file_entry),
        file_blank: layout(&values.file_blank),
        trash: layout(&values.trash),
        search_entry_types: layout(&values.search_entry_types),
        list_columns: layout(&values.list_columns),
        sidebar_bookmark: layout(&values.sidebar_bookmark),
        sidebar_device: layout(&values.sidebar_device),
        network_connection: layout(&values.network_connection),
    }
}

pub(super) fn context_menu_preferences_from_stored(
    stored: &StoredUserPreferences,
    default: &UserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> ContextMenuPreferences {
    let Some(layouts) = &stored.context_menu_layouts else {
        return default.context_menus.clone();
    };
    let config_values = |layout: &StoredContextMenuLayout| {
        layout
            .items
            .iter()
            .map(|entry| (entry.id.clone(), entry.visible))
            .collect::<Vec<(String, bool)>>()
    };
    let stored_values = ContextMenuLayoutConfigValues {
        file_entry: config_values(&layouts.file_entry),
        file_blank: config_values(&layouts.file_blank),
        trash: config_values(&layouts.trash),
        search_entry_types: config_values(&layouts.search_entry_types),
        list_columns: config_values(&layouts.list_columns),
        sidebar_bookmark: config_values(&layouts.sidebar_bookmark),
        sidebar_device: config_values(&layouts.sidebar_device),
        network_connection: config_values(&layouts.network_connection),
    };
    let preferences = ContextMenuPreferences::from_config_values(&stored_values);
    // 解码会丢掉不认识的菜单项；转回配置值后，原来有、现在没有的 id 就是被丢掉的。
    let decoded_values = preferences.to_config_values();
    let dropped_unknown_item = context_menu_layout_lists(&stored_values)
        .into_iter()
        .zip(context_menu_layout_lists(&decoded_values))
        .any(|(stored_items, decoded_items)| {
            stored_items.iter().any(|(stored_id, _)| {
                !decoded_items
                    .iter()
                    .any(|(decoded_id, _)| decoded_id == stored_id)
            })
        });
    if dropped_unknown_item {
        unrecognized.note("context_menu_layouts");
    }
    preferences
}

fn context_menu_layout_lists(values: &ContextMenuLayoutConfigValues) -> [&Vec<(String, bool)>; 8] {
    [
        &values.file_entry,
        &values.file_blank,
        &values.trash,
        &values.search_entry_types,
        &values.list_columns,
        &values.sidebar_bookmark,
        &values.sidebar_device,
        &values.network_connection,
    ]
}
