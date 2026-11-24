//! 右侧停靠区和信息面板的存储形态 ↔ 设置。0.4.7 及以前只有一个右侧面板（预览在上、信息
//! 在下），没有 `right_dock` 键时由那三个旧键换算成同样子的布局。

use super::super::unrecognized_settings::UnrecognizedSettings;
use super::super::{
    normalize_right_preview_preview_ratio, StoredDockGroup, StoredInfoPanel, StoredRightDock,
    StoredUserPreferences, DEFAULT_RIGHT_DOCK_WIDTH, DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO,
};
use super::UserPreferences;
use crate::info_panel::{InfoField, InfoPanelPreferences};
use crate::right_dock::RightDockLayout;

pub(super) fn right_dock_from_stored(
    stored: &StoredUserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> RightDockLayout {
    let Some(dock) = &stored.right_dock else {
        return RightDockLayout::from_previous_panel(
            stored.right_preview_panel_open,
            stored
                .right_preview_panel_width
                .map_or(DEFAULT_RIGHT_DOCK_WIDTH, |width| width as f32),
            stored
                .right_preview_preview_ratio
                .map_or(DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO, |ratio| {
                    normalize_right_preview_preview_ratio(ratio as f32)
                }),
        );
    };
    let groups = dock.groups.iter().map(|group| {
        (
            group.panels.iter().map(String::as_str).collect(),
            group.active as usize,
            group.portion.min(u32::from(u16::MAX)) as u16,
        )
    });
    // 认不出的面板（更新的版本加的）、空格子去掉；其余照用。
    let (layout, dropped) =
        RightDockLayout::from_parts(dock.visible, dock.width as f32, dock.locked, groups);
    if dropped {
        unrecognized.note("right_dock");
    }
    layout
}

pub(super) fn right_dock_to_stored(layout: &RightDockLayout) -> StoredRightDock {
    StoredRightDock {
        visible: layout.is_visible(),
        width: f64::from(layout.width()),
        locked: layout.is_locked(),
        groups: layout
            .groups()
            .iter()
            .map(|group| StoredDockGroup {
                panels: group
                    .panels()
                    .iter()
                    .map(|panel| panel.storage_name().to_owned())
                    .collect(),
                active: group
                    .panels()
                    .iter()
                    .position(|panel| *panel == group.active_panel())
                    .unwrap_or(0) as u32,
                portion: u32::from(group.portion()),
            })
            .collect(),
    }
}

pub(super) fn info_panel_from_stored(
    stored: &StoredUserPreferences,
    default_preferences: &UserPreferences,
    unrecognized: &mut UnrecognizedSettings,
) -> InfoPanelPreferences {
    let Some(panel) = &stored.info_panel else {
        return default_preferences.info_panel.clone();
    };
    let hidden: Vec<InfoField> = panel
        .hidden_fields
        .iter()
        .filter_map(|name| InfoField::from_storage_name(name))
        .collect();
    unrecognized.note_dropped("info_panel", panel.hidden_fields.len(), hidden.len());
    InfoPanelPreferences::new(hidden)
}

pub(super) fn info_panel_to_stored(preferences: &InfoPanelPreferences) -> StoredInfoPanel {
    StoredInfoPanel {
        hidden_fields: preferences
            .hidden_fields()
            .iter()
            .map(|field| field.storage_name().to_owned())
            .collect(),
    }
}
