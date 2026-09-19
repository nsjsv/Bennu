use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::DirectoryEntry;
use iced::Point;

use crate::icon_grid_geometry::{
    column_count_for_width, keyboard_target_index, row_count_for_entries, row_height,
    visible_vertical_window, IconGridDirection, ICON_GRID_CONTENT_PADDING,
};
use crate::model::{
    ExpandedDirectoryStatus, IconGridExpandedDirectory, IconGridExpansionState, IconGridViewport,
};
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, TransferPlaceholder, TransferSortOptions,
};
use crate::virtual_range::vertical_scroll_delta_to_reveal;

mod interaction_geometry;
mod root_group_plan;

use interaction_geometry::IconGridEntryGeometry;
pub(crate) use root_group_plan::IconGridRootGrouping;
use root_group_plan::{split_root_cells, IconGridRowsPlan};

/// 展开子面板的占位注入表:目录 -> (占位, 排序)。由调用方(持有
/// FileBrowser)一次性派生,布局保持纯几何、不触操作队列;拖放进
/// 展开中的目录时,band 内占位与列表/多栏同一事实源。
pub(crate) type ExpandedTransferPlaceholderIndex =
    HashMap<PathBuf, (Vec<TransferPlaceholder>, TransferSortOptions)>;

/// 网格单元:真实条目或传输占位(根面板与展开子面板均可合入)。
/// 交互(键盘/命中/揭示)一律只认 Entry,占位纯展示。
#[derive(Debug, Clone)]
pub(crate) enum IconGridCell<'a> {
    Entry(IconGridEntryCell<'a>),
    Placeholder(TransferPlaceholder),
}

/// 条目格子与其在所属目录条目数组中的真实下标:分组会把根条目重排,
/// 展开锚点必须以条目数组下标为键,因此下标在格子构造期确定并随格子
/// 走,渲染与布局直接消费,不再按格子顺序隐式计数。
#[derive(Debug, Clone)]
pub(crate) struct IconGridEntryCell<'a> {
    pub(crate) entry: &'a DirectoryEntry,
    pub(crate) entry_index: usize,
    /// 同名传输占位的装饰(进度环),渲染时叠加在瓦片图标上。
    pub(crate) transfer: Option<TransferPlaceholder>,
}

impl<'a> IconGridCell<'a> {
    fn entry(&self) -> Option<&'a DirectoryEntry> {
        match self {
            Self::Entry(cell) => Some(cell.entry),
            Self::Placeholder(_) => None,
        }
    }
}

pub(crate) const ICON_GRID_STATUS_HEIGHT: f32 = 48.0;

/// 网格分组标题条高度:与列表组头行同族同值的面板 chrome,不随
/// icon_edge 缩放;两侧共用一个事实源防止视觉漂移。
pub(crate) const ICON_GRID_GROUP_HEADER_HEIGHT: f32 = crate::list_view::LIST_GROUP_HEADER_HEIGHT;

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

/// 根面板分组标题条:全宽纯展示 chrome,不是格子——不产生交互目标,
/// 不进键盘/命中/选择集合;可见性按整段与窗口相交处理(与 Band 同思路),
/// 数量随标题一起展示;index_label 供索引栏与组头同源派生。
#[derive(Debug)]
pub(crate) struct IconGridGroupHeaderLayout {
    pub(crate) title: String,
    pub(crate) index_label: String,
    pub(crate) count: usize,
    pub(crate) top: f32,
    pub(crate) height: f32,
}

#[derive(Debug)]
pub(crate) enum IconGridFlowSegment<'a> {
    Rows(IconGridRowsLayout<'a>),
    Band(IconGridBandLayout<'a>),
    GroupHeader(IconGridGroupHeaderLayout),
}

impl IconGridFlowSegment<'_> {
    pub(crate) fn top(&self) -> f32 {
        match self {
            Self::Rows(rows) => rows.top,
            Self::Band(band) => band.top,
            Self::GroupHeader(header) => header.top,
        }
    }

    pub(crate) fn height(&self) -> f32 {
        match self {
            Self::Rows(rows) => rows.height,
            Self::Band(band) => band.height,
            Self::GroupHeader(header) => header.height,
        }
    }
}

#[derive(Debug)]
pub(crate) struct IconGridPanelLayout<'a> {
    pub(crate) status: IconGridPanelStatus,
    pub(crate) height: f32,
    pub(crate) flow: Vec<IconGridFlowSegment<'a>>,
}

impl IconGridPanelLayout<'_> {
    /// 分组索引栏条目:组头条段的短标签与顶点偏移直接取自 flow 几何,
    /// 与渲染/reveal 同一事实源,不重算分组也不重算高度。
    pub(crate) fn file_group_rail_entries(&self) -> Vec<crate::model::FileGroupRailEntry> {
        self.flow
            .iter()
            .filter_map(|segment| match segment {
                IconGridFlowSegment::GroupHeader(header) => {
                    Some(crate::model::FileGroupRailEntry {
                        index_label: header.index_label.clone(),
                        top_offset: header.top,
                    })
                }
                _ => None,
            })
            .collect()
    }
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

impl<'a> IconGridLayout<'a> {
    /// 根面板与展开子面板合入传输占位:占位按各自排序插入条目流,
    /// 几何随合并后单元格数推导;交互(键盘/命中/揭示)只认真实条目。
    /// 分组开启(仅根面板)时根合并流先按目录置顶、文件段按组划分,
    /// 划分器与列表共用;子面板/band 永远平铺不分组。
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
        root_grouping: Option<IconGridRootGrouping<'_>>,
    ) -> Self {
        let expansion = expansion.filter(|state| state.context().current_dir == root_directory);
        let children_by_parent = expansion.map(index_visible_children);
        let merged = merge_entries_with_placeholders(root_entries, root_placeholders, root_sort);
        let (root_cells, root_rows_plan) = split_root_cells(merged, root_entries, root_grouping);
        // 分组开启时根单元格只含置顶目录,文件组也可能非空——两者皆空才算空态。
        let root_status = if root_cells.is_empty() && !root_rows_plan.has_any_group_cells() {
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
            root_rows_plan,
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
        interaction_geometry::collect_visible_entries(
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
            return interaction_geometry::rows_entries(&rows.cells)
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
        interaction_geometry::collect_interactive_paths(&self.root, &candidates, &mut matches);
        matches
    }

    pub(crate) fn keyboard_target(
        &self,
        current_path: Option<&Path>,
        direction: IconGridDirection,
    ) -> Option<IconGridNavigationTarget<'a>> {
        if let [IconGridFlowSegment::Rows(rows)] = self.root.flow.as_slice() {
            // 键盘导航只落在真实条目上;占位格被自然跳过。
            let entries = interaction_geometry::rows_entries(&rows.cells).collect::<Vec<_>>();
            let current_index = current_path.and_then(|current_path| {
                entries.iter().position(|entry| entry.path == current_path)
            });
            let target_index =
                keyboard_target_index(current_index, direction, entries.len(), rows.column_count)?;
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
                IconGridDirection::Up => {
                    interaction_geometry::adjacent_vertical_entry(&entries, index, false)
                }
                IconGridDirection::Down => {
                    interaction_geometry::adjacent_vertical_entry(&entries, index, true)
                }
            },
        };

        Some(IconGridNavigationTarget {
            directory: target.directory,
            entry: target.entry,
        })
    }

    pub(crate) fn scroll_delta_to_reveal(&self, viewport: IconGridViewport, path: &Path) -> f32 {
        let Some(entry) = interaction_geometry::find_interactive_entry(
            &self.root,
            0.0,
            0.0,
            self.icon_edge,
            path,
        ) else {
            return 0.0;
        };
        vertical_scroll_delta_to_reveal(
            viewport.offset_y,
            viewport.height,
            entry.top,
            entry.bottom - entry.top,
        )
    }

    /// 光标落点(滚动内容坐标)→ 可交互条目路径:与键盘/reveal 的命中
    /// 几何同源,滚动帧的 hover 补偿由此与渲染瓦片落在同一格子上;
    /// 组头/占位/空白落点返回 None。
    pub(crate) fn entry_path_at_point(&self, point: Point) -> Option<&Path> {
        interaction_geometry::find_interactive_entry_at_point(
            &self.root,
            0.0,
            0.0,
            self.icon_edge,
            point,
        )
        .map(|(_, entry)| entry.path.as_path())
    }

    fn interactive_entry_geometry(&self) -> Vec<IconGridEntryGeometry<'a>> {
        let mut entries = Vec::new();
        interaction_geometry::collect_interactive_entries(
            &self.root,
            0.0,
            0.0,
            self.icon_edge,
            &mut entries,
        );
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
    // 行区计划:Full 铺满全部单元格;GroupedFiles 只把置顶目录铺成行,
    // 文件段由本函数按组追加[组头条, 组行段](仅根面板使用)。
    rows_plan: IconGridRowsPlan<'a>,
    expanded_placeholders: Option<&ExpandedTransferPlaceholderIndex>,
) -> IconGridPanelLayout<'a> {
    let column_count = column_count_for_width(width, icon_edge);
    // 分组根面板的 cells 只含置顶目录,空态判定要连同文件组一起看;
    // 其余面板 cells 即全部内容。
    if status != IconGridPanelStatus::Loaded
        || (cells.is_empty() && !rows_plan.has_any_group_cells())
    {
        return IconGridPanelLayout {
            status,
            height: ICON_GRID_CONTENT_PADDING * 2.0 + ICON_GRID_STATUS_HEIGHT,
            flow: Vec::new(),
        };
    }

    // 条目下标 -> 单元格下标:格子自带真实条目下标(见 IconGridEntryCell),
    // 展开锚点按 path 校验出的条目下标经此映射到行号——分组重排与占位
    // 插入都不会让锚点漂移。分组根的文件段条目不在本面板 cells 里,落空
    // 时回退原下标,与既有陈旧锚点的回退一致。
    let entry_count = cells
        .iter()
        .filter_map(|cell| match cell {
            IconGridCell::Entry(cell) => Some(cell.entry_index + 1),
            IconGridCell::Placeholder(_) => None,
        })
        .max()
        .unwrap_or(0);
    let mut entry_cell_positions = vec![usize::MAX; entry_count];
    for (cell_index, cell) in cells.iter().enumerate() {
        if let IconGridCell::Entry(cell) = cell {
            entry_cell_positions[cell.entry_index] = cell_index;
        }
    }
    let anchor_cell_index = |anchor_index: usize| {
        entry_cell_positions
            .get(anchor_index)
            .copied()
            .filter(|position| *position != usize::MAX)
            .unwrap_or(anchor_index)
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
                IconGridRowsPlan::Full,
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

    match rows_plan {
        IconGridRowsPlan::Full => {
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
        }
        // 分组根的文件段:每组输出[组头条, 组行段]。组头条是纯展示
        // chrome,高度计入流高与 reveal 数学,但不产生任何交互目标;
        // 划分层保证组非空,组行段至少一行。
        IconGridRowsPlan::GroupedFiles(groups) => {
            // 置顶目录的行区先行入流(children 循环只推到 band 锚点行,
            // 无展开时行区不能因为替换了尾段而丢失)。
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
            for group in groups {
                flow.push(IconGridFlowSegment::GroupHeader(
                    IconGridGroupHeaderLayout {
                        title: group.title,
                        index_label: group.index_label,
                        count: group.count,
                        top,
                        height: ICON_GRID_GROUP_HEADER_HEIGHT,
                    },
                ));
                top += ICON_GRID_GROUP_HEADER_HEIGHT;
                let rows = row_count_for_entries(group.cells.len(), column_count);
                let height = rows as f32 * row_height(icon_edge);
                flow.push(IconGridFlowSegment::Rows(IconGridRowsLayout {
                    directory,
                    cells: group.cells,
                    start_row: 0,
                    end_row: rows,
                    column_count,
                    top,
                    height,
                }));
                top += height;
            }
        }
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
            // 子面板不分组,合并序保持条目原序,出现序即条目真实下标。
            let mut next_entry_index = 0usize;
            let cells: Vec<IconGridCell> = match expanded_placeholders
                .and_then(|index| index.get(directory_path))
                .filter(|(placeholders, _)| !placeholders.is_empty())
            {
                Some((placeholders, sort)) => merge_entries_with_placeholders(
                    &directory.contents.entries,
                    placeholders,
                    *sort,
                )
                .iter()
                .map(|item| match item {
                    crate::transfer_placeholders::MergedTransferItem::Entry { entry, transfer } => {
                        let entry_index = next_entry_index;
                        next_entry_index += 1;
                        IconGridCell::Entry(IconGridEntryCell {
                            entry,
                            entry_index,
                            transfer: transfer.clone(),
                        })
                    }
                    crate::transfer_placeholders::MergedTransferItem::Placeholder(placeholder) => {
                        IconGridCell::Placeholder(placeholder.clone())
                    }
                })
                .collect(),
                None => directory
                    .contents
                    .entries
                    .iter()
                    .map(|entry| {
                        let entry_index = next_entry_index;
                        next_entry_index += 1;
                        IconGridCell::Entry(IconGridEntryCell {
                            entry,
                            entry_index,
                            transfer: None,
                        })
                    })
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

#[cfg(test)]
#[path = "icon_grid_layout_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "icon_grid_layout_grouping_tests.rs"]
mod grouping_tests;
