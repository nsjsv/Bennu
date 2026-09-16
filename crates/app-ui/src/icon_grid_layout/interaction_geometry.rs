//! 面板流段的交互几何遍历:可见条目收集(缩略图调度)、交互条目几何
//! (键盘/命中/揭示)、路径匹配全部只认 Entry 格子;Band 透传子面板,
//! GroupHeader 是纯展示 chrome,不产生任何交互事实。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::Point;

use super::{IconGridCell, IconGridFlowSegment, IconGridPanelLayout};
use crate::icon_grid_geometry::{
    grid_gap, row_height, tile_visual_height, tile_width, ICON_GRID_CONTENT_PADDING,
};

/// 单个交互条目的命中几何:键盘导航与 reveal 数学共用同一来源,
/// 保证落点与渲染一致。
#[derive(Debug, Clone, Copy)]
pub(super) struct IconGridEntryGeometry<'a> {
    pub(super) directory: &'a Path,
    pub(super) entry: &'a DirectoryEntry,
    pub(super) center_x: f32,
    pub(super) center_y: f32,
    pub(super) top: f32,
    pub(super) bottom: f32,
}

pub(super) fn collect_visible_entries<'a>(
    panel: &IconGridPanelLayout<'a>,
    panel_top: f32,
    clip_bottom: f32,
    visible_top: f32,
    visible_bottom: f32,
    icon_edge: u32,
    collected: &mut Vec<super::IconGridVisibleEntry<'a>>,
) {
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                let Some(visible_rows) = rows.visible_rows_in_window(
                    panel_top,
                    clip_bottom,
                    visible_top,
                    visible_bottom,
                    icon_edge,
                ) else {
                    continue;
                };
                for row in visible_rows.start_row..visible_rows.end_row {
                    let start = row.saturating_mul(rows.column_count).min(rows.cells.len());
                    let end = start
                        .saturating_add(rows.column_count)
                        .min(rows.cells.len());
                    for cell in &rows.cells[start..end] {
                        if let Some(entry) = cell.entry() {
                            collected.push(super::IconGridVisibleEntry { entry });
                        }
                    }
                }
            }
            IconGridFlowSegment::Band(band) if band.interactive => {
                let band_top = panel_top + band.top;
                let band_bottom = (band_top + band.height).min(clip_bottom);
                if band_top >= band_bottom
                    || band_top >= visible_bottom
                    || band_bottom <= visible_top
                {
                    continue;
                }
                collect_visible_entries(
                    &band.panel,
                    band_top,
                    band_bottom,
                    visible_top,
                    visible_bottom,
                    icon_edge,
                    collected,
                );
            }
            // 组头条没有条目可收集;折叠中/非交互 Band 不可见。
            IconGridFlowSegment::GroupHeader(_) | IconGridFlowSegment::Band(_) => {}
        }
    }
}

pub(super) fn collect_interactive_entries<'a>(
    panel: &IconGridPanelLayout<'a>,
    panel_top: f32,
    panel_left: f32,
    icon_edge: u32,
    collected: &mut Vec<IconGridEntryGeometry<'a>>,
) {
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                let rows_top = panel_top + rows.top;
                for row in rows.start_row..rows.end_row {
                    let start = row.saturating_mul(rows.column_count).min(rows.cells.len());
                    let end = start
                        .saturating_add(rows.column_count)
                        .min(rows.cells.len());
                    let top = rows_top
                        + row.saturating_sub(rows.start_row) as f32 * row_height(icon_edge);
                    // 交互几何按单元格列位计算;占位格不产生交互事实。
                    for (column, cell) in rows.cells[start..end].iter().enumerate() {
                        if let Some(entry) = cell.entry() {
                            collected.push(IconGridEntryGeometry {
                                directory: rows.directory,
                                entry,
                                center_x: panel_left
                                    + ICON_GRID_CONTENT_PADDING
                                    + column as f32
                                        * (tile_width(icon_edge) + grid_gap(icon_edge))
                                    + tile_width(icon_edge) / 2.0,
                                center_y: top + tile_visual_height(icon_edge) / 2.0,
                                top,
                                bottom: top + tile_visual_height(icon_edge),
                            });
                        }
                    }
                }
            }
            IconGridFlowSegment::Band(band) if band.interactive => {
                collect_interactive_entries(
                    &band.panel,
                    panel_top + band.top,
                    panel_left,
                    icon_edge,
                    collected,
                );
            }
            // 组头条不是格子,键盘导航/框选天然跳过。
            IconGridFlowSegment::GroupHeader(_) | IconGridFlowSegment::Band(_) => {}
        }
    }
}

pub(super) fn rows_entries<'cells, 'a>(
    cells: &'cells [IconGridCell<'a>],
) -> impl Iterator<Item = &'a DirectoryEntry> + 'cells {
    cells.iter().filter_map(IconGridCell::entry)
}

pub(super) fn collect_interactive_paths(
    panel: &IconGridPanelLayout<'_>,
    candidates: &HashSet<&Path>,
    matches: &mut HashSet<PathBuf>,
) {
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                for entry in rows_entries(&rows.cells) {
                    if candidates.contains(entry.path.as_path()) {
                        matches.insert(entry.path.clone());
                    }
                }
            }
            IconGridFlowSegment::Band(band) if band.interactive => {
                collect_interactive_paths(&band.panel, candidates, matches);
            }
            IconGridFlowSegment::GroupHeader(_) | IconGridFlowSegment::Band(_) => {}
        }
    }
}

pub(super) fn find_interactive_entry<'a>(
    panel: &IconGridPanelLayout<'a>,
    panel_top: f32,
    panel_left: f32,
    icon_edge: u32,
    path: &Path,
) -> Option<IconGridEntryGeometry<'a>> {
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                let start = rows
                    .start_row
                    .saturating_mul(rows.column_count)
                    .min(rows.cells.len());
                let end = rows
                    .end_row
                    .saturating_mul(rows.column_count)
                    .min(rows.cells.len());
                let Some(relative_index) = rows.cells[start..end]
                    .iter()
                    .position(|cell| cell.entry().is_some_and(|entry| entry.path == path))
                else {
                    continue;
                };
                let index = relative_index + start;
                let Some(entry) = rows.cells[index].entry() else {
                    continue;
                };
                let row = index / rows.column_count;
                let column = index % rows.column_count;
                let top = panel_top
                    + rows.top
                    + row.saturating_sub(rows.start_row) as f32 * row_height(icon_edge);
                return Some(IconGridEntryGeometry {
                    directory: rows.directory,
                    entry,
                    center_x: panel_left
                        + ICON_GRID_CONTENT_PADDING
                        + column as f32 * (tile_width(icon_edge) + grid_gap(icon_edge))
                        + tile_width(icon_edge) / 2.0,
                    center_y: top + tile_visual_height(icon_edge) / 2.0,
                    top,
                    bottom: top + tile_visual_height(icon_edge),
                });
            }
            IconGridFlowSegment::Band(band) if band.interactive => {
                if let Some(entry) = find_interactive_entry(
                    &band.panel,
                    panel_top + band.top,
                    panel_left,
                    icon_edge,
                    path,
                ) {
                    return Some(entry);
                }
            }
            // 组头条不可被 reveal 定位;流段高度已计入行段 top,reveal
            // 数学与渲染同口径无需分支修正。
            IconGridFlowSegment::GroupHeader(_) | IconGridFlowSegment::Band(_) => {}
        }
    }
    None
}

/// 光标落点(面板内容坐标)→ 交互条目:列位换算与
/// `collect_interactive_entries` 的命中几何同源(tile 宽 + 间隙的
/// 等宽槽位、行内顶端对齐),滚动帧的 hover 补偿与 mouse_area 的
/// bounds 门控因此落在同一格子上。占位格、组头条、列间隙与 tile
/// 上下方的行内空白都不产生命中。
pub(super) fn find_interactive_entry_at_point<'a>(
    panel: &IconGridPanelLayout<'a>,
    panel_top: f32,
    panel_left: f32,
    icon_edge: u32,
    point: Point,
) -> Option<(&'a Path, &'a DirectoryEntry)> {
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                let rows_top = panel_top + rows.top;
                let local_y = point.y - rows_top;
                if local_y < 0.0 || local_y >= rows.height {
                    continue;
                }
                let local_row = (local_y / row_height(icon_edge)).floor().max(0.0) as usize;
                let row = rows.start_row + local_row;
                let slot = point.x - panel_left - ICON_GRID_CONTENT_PADDING;
                // 落在内容内边距左侧:整段无命中。
                if slot < 0.0 {
                    continue;
                }
                let column_slot = tile_width(icon_edge) + grid_gap(icon_edge);
                let column = (slot / column_slot) as usize;
                // 列间隙与最后一列之后的空白不命中;末行残缺的越界格子
                // 同样不命中。
                if column >= rows.column_count
                    || slot % column_slot >= tile_width(icon_edge)
                    || point.y >= rows_top
                        + local_row as f32 * row_height(icon_edge)
                        + tile_visual_height(icon_edge)
                {
                    continue;
                }
                let cell_index = row.saturating_mul(rows.column_count) + column;
                if let Some(entry) = rows.cells.get(cell_index).and_then(IconGridCell::entry) {
                    return Some((rows.directory, entry));
                }
            }
            IconGridFlowSegment::Band(band) if band.interactive => {
                if let Some(found) = find_interactive_entry_at_point(
                    &band.panel,
                    panel_top + band.top,
                    panel_left,
                    icon_edge,
                    point,
                ) {
                    return Some(found);
                }
            }
            // 组头条不是格子,hover 补偿天然落空。
            IconGridFlowSegment::GroupHeader(_) | IconGridFlowSegment::Band(_) => {}
        }
    }
    None
}

/// 上/下键的跨行落点:先找与目标行同 y 的候选,再取 x 距离最近者。
pub(super) fn adjacent_vertical_entry<'a>(
    entries: &[IconGridEntryGeometry<'a>],
    current_index: usize,
    move_down: bool,
) -> IconGridEntryGeometry<'a> {
    let current = entries[current_index];
    let target_y = entries
        .iter()
        .filter(|entry| {
            if move_down {
                entry.center_y > current.center_y + f32::EPSILON
            } else {
                entry.center_y < current.center_y - f32::EPSILON
            }
        })
        .map(|entry| entry.center_y)
        .reduce(|candidate, y| {
            if move_down {
                candidate.min(y)
            } else {
                candidate.max(y)
            }
        });
    let Some(target_y) = target_y else {
        return current;
    };

    entries
        .iter()
        .filter(|entry| (entry.center_y - target_y).abs() <= f32::EPSILON)
        .min_by(|left, right| {
            (left.center_x - current.center_x)
                .abs()
                .total_cmp(&(right.center_x - current.center_x).abs())
                .then_with(|| left.center_x.total_cmp(&right.center_x))
        })
        .copied()
        .unwrap_or(current)
}
