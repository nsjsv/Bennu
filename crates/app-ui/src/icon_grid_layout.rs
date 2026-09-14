use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::DirectoryEntry;

use crate::icon_grid_geometry::{
    column_count_for_width, grid_gap, keyboard_target_index, row_count_for_entries, row_height,
    tile_visual_height, tile_width, visible_vertical_window, IconGridDirection,
    ICON_GRID_CONTENT_PADDING,
};
use crate::model::{
    ExpandedDirectoryStatus, IconGridExpandedDirectory, IconGridExpansionState, IconGridViewport,
};
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, TransferPlaceholder, TransferSortOptions,
};
use crate::virtual_range::vertical_scroll_delta_to_reveal;

/// 展开子面板的占位注入表:目录 -> (占位, 排序)。由调用方(持有
/// FileBrowser)一次性派生,布局保持纯几何、不触操作队列;拖放进
/// 展开中的目录时,band 内占位与列表/多栏同一事实源。
pub(crate) type ExpandedTransferPlaceholderIndex =
    HashMap<PathBuf, (Vec<TransferPlaceholder>, TransferSortOptions)>;

/// 网格单元:真实条目或传输占位(根面板与展开子面板均可合入)。
/// 交互(键盘/命中/揭示)一律只认 Entry,占位纯展示。
#[derive(Debug, Clone)]
pub(crate) enum IconGridCell<'a> {
    Entry(&'a DirectoryEntry),
    Placeholder(TransferPlaceholder),
}

impl<'a> IconGridCell<'a> {
    fn entry(&self) -> Option<&'a DirectoryEntry> {
        match self {
            Self::Entry(entry) => Some(entry),
            Self::Placeholder(_) => None,
        }
    }
}

pub(crate) const ICON_GRID_STATUS_HEIGHT: f32 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconGridPanelStatus {
    Loaded,
    Loading,
    Empty,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IconGridVisibleRows {
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) before_height: f32,
    pub(crate) after_height: f32,
}

#[derive(Debug)]
pub(crate) struct IconGridRowsLayout<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) cells: Arc<[IconGridCell<'a>]>,
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) column_count: usize,
    pub(crate) top: f32,
    pub(crate) height: f32,
}

impl IconGridRowsLayout<'_> {
    pub(crate) fn visible_rows(
        &self,
        panel_top: f32,
        clip_bottom: f32,
        viewport: IconGridViewport,
        icon_edge: u32,
        height_bound: f32,
    ) -> Option<IconGridVisibleRows> {
        let (visible_top, visible_bottom) =
            visible_vertical_window(viewport, icon_edge, height_bound);
        self.visible_rows_in_window(
            panel_top,
            clip_bottom,
            visible_top,
            visible_bottom,
            icon_edge,
        )
    }

    fn visible_rows_in_window(
        &self,
        panel_top: f32,
        clip_bottom: f32,
        visible_top: f32,
        visible_bottom: f32,
        icon_edge: u32,
    ) -> Option<IconGridVisibleRows> {
        let rows_top = panel_top + self.top;
        let rows_bottom = (rows_top + self.height).min(clip_bottom);
        let intersection_top = rows_top.max(visible_top);
        let intersection_bottom = rows_bottom.min(visible_bottom);
        if intersection_top >= intersection_bottom {
            return None;
        }

        let row_height = row_height(icon_edge);
        let local_start = ((intersection_top - rows_top) / row_height)
            .floor()
            .max(0.0) as usize;
        let local_end = ((intersection_bottom - rows_top) / row_height)
            .ceil()
            .max(0.0) as usize;
        let start_row = self.start_row.saturating_add(local_start).min(self.end_row);
        let end_row = self.start_row.saturating_add(local_end).min(self.end_row);
        Some(IconGridVisibleRows {
            start_row,
            end_row,
            before_height: start_row.saturating_sub(self.start_row) as f32 * row_height,
            after_height: self.end_row.saturating_sub(end_row) as f32 * row_height,
        })
    }
}

#[derive(Debug)]
pub(crate) struct IconGridBandLayout<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) anchor_column: usize,
    pub(crate) top: f32,
    pub(crate) height: f32,
    pub(crate) natural_height: f32,
    pub(crate) interactive: bool,
    pub(crate) panel: Box<IconGridPanelLayout<'a>>,
}

#[derive(Debug)]
pub(crate) enum IconGridFlowSegment<'a> {
    Rows(IconGridRowsLayout<'a>),
    Band(IconGridBandLayout<'a>),
}

impl IconGridFlowSegment<'_> {
    pub(crate) fn top(&self) -> f32 {
        match self {
            Self::Rows(rows) => rows.top,
            Self::Band(band) => band.top,
        }
    }

    pub(crate) fn height(&self) -> f32 {
        match self {
            Self::Rows(rows) => rows.height,
            Self::Band(band) => band.height,
        }
    }
}

#[derive(Debug)]
pub(crate) struct IconGridPanelLayout<'a> {
    pub(crate) status: IconGridPanelStatus,
    pub(crate) height: f32,
    pub(crate) flow: Vec<IconGridFlowSegment<'a>>,
}

#[derive(Debug)]
pub(crate) struct IconGridLayout<'a> {
    icon_edge: u32,
    height_bound: f32,
    root: IconGridPanelLayout<'a>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IconGridVisibleEntry<'a> {
    pub(crate) entry: &'a DirectoryEntry,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IconGridNavigationTarget<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) entry: &'a DirectoryEntry,
}

#[derive(Debug, Clone, Copy)]
struct IconGridEntryGeometry<'a> {
    directory: &'a Path,
    entry: &'a DirectoryEntry,
    center_x: f32,
    center_y: f32,
    top: f32,
    bottom: f32,
}

impl<'a> IconGridLayout<'a> {
    /// 根面板与展开子面板合入传输占位:占位按各自排序插入条目流,
    /// 几何随合并后单元格数推导;交互(键盘/命中/揭示)只认真实条目。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_root_transfer_placeholders(
        root_directory: &'a Path,
        root_entries: &'a [DirectoryEntry],
        root_placeholders: &[TransferPlaceholder],
        root_sort: TransferSortOptions,
        expanded_placeholders: &ExpandedTransferPlaceholderIndex,
        viewport_width: f32,
        height_bound: f32,
        icon_edge: u32,
        expansion: Option<&'a IconGridExpansionState>,
    ) -> Self {
        let expansion = expansion.filter(|state| state.context().current_dir == root_directory);
        let children_by_parent = expansion.map(index_visible_children);
        let root_cells: Arc<[IconGridCell<'a>]> =
            merge_entries_with_placeholders(root_entries, root_placeholders, root_sort)
                .into_iter()
                .map(|item| match item {
                    crate::transfer_placeholders::MergedTransferItem::Entry(entry) => {
                        IconGridCell::Entry(entry)
                    }
                    crate::transfer_placeholders::MergedTransferItem::Placeholder(
                        placeholder,
                    ) => IconGridCell::Placeholder(placeholder),
                })
                .collect();
        // 条目索引 -> 单元格索引:展开锚点仍以条目索引为键,布局阶段
        // 换算成行号,占位插入不会让锚点漂移。
        let root_entry_cell_positions = root_cells
            .iter()
            .enumerate()
            .filter_map(|(cell_index, cell)| cell.entry().map(|_| cell_index))
            .collect::<Vec<_>>();
        let root_status = if root_cells.is_empty() {
            IconGridPanelStatus::Empty
        } else {
            IconGridPanelStatus::Loaded
        };
        let root = build_panel(
            root_directory,
            root_cells,
            root_status,
            viewport_width.max(0.0),
            icon_edge,
            children_by_parent.as_ref(),
            Some(&root_entry_cell_positions),
            Some(expanded_placeholders),
        );
        Self {
            icon_edge,
            height_bound,
            root,
        }
    }

    pub(crate) fn root(&self) -> &IconGridPanelLayout<'a> {
        &self.root
    }

    pub(crate) fn total_height(&self) -> f32 {
        self.root.height
    }

    pub(crate) fn visible_entries(
        &self,
        viewport: IconGridViewport,
    ) -> Vec<IconGridVisibleEntry<'a>> {
        let (visible_top, visible_bottom) =
            visible_vertical_window(viewport, self.icon_edge, self.height_bound);
        let mut entries = Vec::new();
        collect_visible_entries(
            &self.root,
            0.0,
            self.root.height,
            visible_top,
            visible_bottom,
            self.icon_edge,
            &mut entries,
        );
        entries
    }

    pub(crate) fn interactive_entry_paths(&self) -> Vec<PathBuf> {
        if let [IconGridFlowSegment::Rows(rows)] = self.root.flow.as_slice() {
            return rows_entries(&rows.cells)
                .map(|entry| entry.path.clone())
                .collect();
        }
        self.interactive_entry_geometry()
            .into_iter()
            .map(|entry| entry.entry.path.clone())
            .collect()
    }

    pub(crate) fn interactive_paths_matching(&self, paths: &[PathBuf]) -> HashSet<PathBuf> {
        let candidates = paths
            .iter()
            .map(|path| path.as_path())
            .collect::<HashSet<_>>();
        let mut matches = HashSet::new();
        collect_interactive_paths(&self.root, &candidates, &mut matches);
        matches
    }

    pub(crate) fn keyboard_target(
        &self,
        current_path: Option<&Path>,
        direction: IconGridDirection,
    ) -> Option<IconGridNavigationTarget<'a>> {
        if let [IconGridFlowSegment::Rows(rows)] = self.root.flow.as_slice() {
            // 键盘导航只落在真实条目上;占位格被自然跳过。
            let entries = rows_entries(&rows.cells).collect::<Vec<_>>();
            let current_index = current_path
                .and_then(|current_path| entries.iter().position(|entry| entry.path == current_path));
            let target_index = keyboard_target_index(
                current_index,
                direction,
                entries.len(),
                rows.column_count,
            )?;
            return Some(IconGridNavigationTarget {
                directory: rows.directory,
                entry: entries[target_index],
            });
        }

        let entries = self.interactive_entry_geometry();
        if entries.is_empty() {
            return None;
        }

        let current_index = current_path.and_then(|current_path| {
            entries
                .iter()
                .position(|entry| entry.entry.path == current_path)
        });
        let target = match current_index {
            None => match direction {
                IconGridDirection::Up | IconGridDirection::Left => *entries.last()?,
                IconGridDirection::Down | IconGridDirection::Right => *entries.first()?,
            },
            Some(index) => match direction {
                IconGridDirection::Left => entries[index.saturating_sub(1)],
                IconGridDirection::Right => entries[(index + 1).min(entries.len() - 1)],
                IconGridDirection::Up => adjacent_vertical_entry(&entries, index, false),
                IconGridDirection::Down => adjacent_vertical_entry(&entries, index, true),
            },
        };

        Some(IconGridNavigationTarget {
            directory: target.directory,
            entry: target.entry,
        })
    }

    pub(crate) fn scroll_delta_to_reveal(&self, viewport: IconGridViewport, path: &Path) -> f32 {
        let Some(entry) = find_interactive_entry(&self.root, 0.0, 0.0, self.icon_edge, path) else {
            return 0.0;
        };
        vertical_scroll_delta_to_reveal(
            viewport.offset_y,
            viewport.height,
            entry.top,
            entry.bottom - entry.top,
        )
    }

    fn interactive_entry_geometry(&self) -> Vec<IconGridEntryGeometry<'a>> {
        let mut entries = Vec::new();
        collect_interactive_entries(&self.root, 0.0, 0.0, self.icon_edge, &mut entries);
        entries
    }
}

fn build_panel<'a>(
    directory: &'a Path,
    cells: Arc<[IconGridCell<'a>]>,
    status: IconGridPanelStatus,
    width: f32,
    icon_edge: u32,
    children_by_parent: Option<&IconGridChildrenByParent<'a>>,
    // 本面板子锚点(条目索引 -> 单元格索引)的换算表;根面板携带,
    // 子面板的条目与单元格一一对应,用恒等映射。
    entry_cell_positions: Option<&[usize]>,
    expanded_placeholders: Option<&ExpandedTransferPlaceholderIndex>,
) -> IconGridPanelLayout<'a> {
    let column_count = column_count_for_width(width, icon_edge);
    if status != IconGridPanelStatus::Loaded || cells.is_empty() {
        return IconGridPanelLayout {
            status,
            height: ICON_GRID_CONTENT_PADDING * 2.0 + ICON_GRID_STATUS_HEIGHT,
            flow: Vec::new(),
        };
    }

    let anchor_cell_index = |anchor_index: usize| match entry_cell_positions {
        Some(positions) => positions.get(anchor_index).copied().unwrap_or(anchor_index),
        None => anchor_index,
    };
    let total_rows = row_count_for_entries(cells.len(), column_count);
    let children = children_by_parent
        .and_then(|children| children.get(directory))
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut flow = Vec::with_capacity(children.len().saturating_mul(2).saturating_add(1));
    let mut next_row = 0;
    let mut top = ICON_GRID_CONTENT_PADDING;
    let mut child_cursor = 0;
    while child_cursor < children.len() {
        let anchor_row = anchor_cell_index(children[child_cursor].1.anchor_index) / column_count;
        let rows_end = anchor_row.saturating_add(1).min(total_rows);
        if next_row < rows_end {
            let height = (rows_end - next_row) as f32 * row_height(icon_edge);
            flow.push(IconGridFlowSegment::Rows(IconGridRowsLayout {
                directory,
                cells: Arc::clone(&cells),
                start_row: next_row,
                end_row: rows_end,
                column_count,
                top,
                height,
            }));
            top += height;
            next_row = rows_end;
        }

        while child_cursor < children.len()
            && anchor_cell_index(children[child_cursor].1.anchor_index) / column_count == anchor_row
        {
            let (child_path, child) = children[child_cursor];
            let (child_cells, child_status) =
                expanded_panel_content(child_path, child, expanded_placeholders);
            let child_panel = build_panel(
                child_path,
                child_cells,
                child_status,
                width,
                icon_edge,
                children_by_parent,
                None,
                expanded_placeholders,
            );
            let natural_height = child_panel.height;
            let animation_progress = child.contents.animation_progress.clamp(0.0, 1.0);
            let height = natural_height * animation_progress;
            flow.push(IconGridFlowSegment::Band(IconGridBandLayout {
                directory: child_path,
                anchor_column: anchor_cell_index(child.anchor_index) % column_count,
                top,
                height,
                natural_height,
                interactive: child.is_interactive(),
                panel: Box::new(child_panel),
            }));
            top += height;
            child_cursor += 1;
        }
    }

    if next_row < total_rows {
        let height = (total_rows - next_row) as f32 * row_height(icon_edge);
        flow.push(IconGridFlowSegment::Rows(IconGridRowsLayout {
            directory,
            cells,
            start_row: next_row,
            end_row: total_rows,
            column_count,
            top,
            height,
        }));
        top += height;
    }

    IconGridPanelLayout {
        status,
        height: top + ICON_GRID_CONTENT_PADDING,
        flow,
    }
}

type IconGridChildrenByParent<'a> =
    HashMap<&'a Path, Vec<(&'a Path, &'a IconGridExpandedDirectory)>>;

fn index_visible_children(expansion: &IconGridExpansionState) -> IconGridChildrenByParent<'_> {
    let mut children_by_parent = IconGridChildrenByParent::new();
    for (path, directory) in expansion
        .directories()
        .filter(|(_, directory)| directory.is_visible())
    {
        children_by_parent
            .entry(directory.parent_directory.as_path())
            .or_default()
            .push((path, directory));
    }
    for children in children_by_parent.values_mut() {
        children.sort_by(|(left_path, left), (right_path, right)| {
            left.anchor_index
                .cmp(&right.anchor_index)
                .then_with(|| left_path.cmp(right_path))
        });
    }
    children_by_parent
}

fn expanded_panel_content<'a>(
    directory_path: &Path,
    directory: &'a IconGridExpandedDirectory,
    expanded_placeholders: Option<&ExpandedTransferPlaceholderIndex>,
) -> (Arc<[IconGridCell<'a>]>, IconGridPanelStatus) {
    match &directory.contents.status {
        ExpandedDirectoryStatus::Loading => (Arc::from(Vec::new()), IconGridPanelStatus::Loading),
        ExpandedDirectoryStatus::Error => (Arc::from(Vec::new()), IconGridPanelStatus::Error),
        ExpandedDirectoryStatus::Loaded => {
            // 展开子面板与根面板同一规则:目标为该目录的传入占位按该
            // 目录的排序合入;空目录但有占位时按有内容渲染,顶掉空态。
            let cells: Vec<IconGridCell> = match expanded_placeholders
                .and_then(|index| index.get(directory_path))
                .filter(|(placeholders, _)| !placeholders.is_empty())
            {
                Some((placeholders, sort)) => merge_entries_with_placeholders(
                    &directory.contents.entries,
                    placeholders,
                    *sort,
                )
                .into_iter()
                .map(|item| match item {
                    crate::transfer_placeholders::MergedTransferItem::Entry(entry) => {
                        IconGridCell::Entry(entry)
                    }
                    crate::transfer_placeholders::MergedTransferItem::Placeholder(
                        placeholder,
                    ) => IconGridCell::Placeholder(placeholder),
                })
                .collect(),
                None => directory
                    .contents
                    .entries
                    .iter()
                    .map(IconGridCell::Entry)
                    .collect(),
            };
            let status = if cells.is_empty() {
                IconGridPanelStatus::Empty
            } else {
                IconGridPanelStatus::Loaded
            };
            (cells.into(), status)
        }
    }
}

fn collect_visible_entries<'a>(
    panel: &IconGridPanelLayout<'a>,
    panel_top: f32,
    clip_bottom: f32,
    visible_top: f32,
    visible_bottom: f32,
    icon_edge: u32,
    collected: &mut Vec<IconGridVisibleEntry<'a>>,
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
                            collected.push(IconGridVisibleEntry { entry });
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
            IconGridFlowSegment::Band(_) => {}
        }
    }
}

fn collect_interactive_entries<'a>(
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
            IconGridFlowSegment::Band(_) => {}
        }
    }
}

fn rows_entries<'cells, 'a>(
    cells: &'cells [IconGridCell<'a>],
) -> impl Iterator<Item = &'a DirectoryEntry> + 'cells {
    cells.iter().filter_map(IconGridCell::entry)
}

fn collect_interactive_paths(
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
            IconGridFlowSegment::Band(_) => {}
        }
    }
}

fn find_interactive_entry<'a>(
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
            IconGridFlowSegment::Band(_) => {}
        }
    }
    None
}

fn adjacent_vertical_entry<'a>(
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

#[cfg(test)]
#[path = "icon_grid_layout_tests.rs"]
mod tests;
