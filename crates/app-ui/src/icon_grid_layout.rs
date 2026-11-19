use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use file_core::DirectoryEntry;
use iced::Point;

use crate::icon_grid_geometry::{
    column_count_for_width, keyboard_target_index, row_count_for_entries, row_height,
    visible_vertical_window, IconGridDirection, ICON_GRID_CONTENT_PADDING,
};
use crate::model::{IconGridExpandedDirectory, IconGridExpansionState, IconGridViewport};
use crate::transfer_placeholders::{TransferPlaceholder, TransferSortOptions};
use crate::virtual_range::vertical_scroll_delta_to_reveal;

mod cell_plan;
mod interaction_geometry;
mod root_group_plan;

pub(crate) use cell_plan::{icon_grid_cell_plan, IconGridCellCache, IconGridCellInputs};
use cell_plan::{IconGridCellContent, IconGridCellPlan, IconGridCellSpan, IconGridCells};
use interaction_geometry::IconGridEntryGeometry;
use root_group_plan::{IconGridRootFileGroup, IconGridRowsPlan};

/// 展开子面板的占位注入表:目录 -> (占位, 排序)。由调用方(持有
/// FileBrowser)一次性派生,布局保持纯几何、不触操作队列;拖放进
/// 展开中的目录时,band 内占位与列表/多栏同一事实源。
pub(crate) type ExpandedTransferPlaceholderIndex =
    HashMap<PathBuf, (Vec<TransferPlaceholder>, TransferSortOptions)>;

/// 网格单元:真实条目或传输占位(根面板与展开子面板均可合入)。
/// 交互(键盘/命中/揭示)一律只认 Entry,占位纯展示。
#[derive(Debug, Clone, Copy)]
pub(crate) enum IconGridCell<'a> {
    Entry(IconGridEntryCell<'a>),
    Placeholder(&'a TransferPlaceholder),
}

/// 条目格子与其在所属目录条目数组中的真实下标:分组会把根条目重排,
/// 展开锚点必须以条目数组下标为键,因此下标在排格子时确定并随格子
/// 走,渲染与布局直接消费,不再按格子顺序隐式计数。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IconGridEntryCell<'a> {
    pub(crate) entry: &'a DirectoryEntry,
    pub(crate) entry_index: usize,
    /// 同名传输占位的装饰(进度环),渲染时叠加在瓦片图标上。
    pub(crate) transfer: Option<&'a TransferPlaceholder>,
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
    /// 这些格子所在的目录；根面板显示的位置不是目录（回收站）时没有。
    pub(crate) directory: Option<&'a Path>,
    /// 格子里的条目下标指向这里（这个面板目录的条目）。
    entries: &'a [DirectoryEntry],
    cells: Rc<IconGridCells>,
    /// 这一段排的是 `cells` 的哪一截（分组的根面板每组一截）。
    span: IconGridCellSpan,
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) column_count: usize,
    pub(crate) top: f32,
    pub(crate) height: f32,
}

impl<'a> IconGridRowsLayout<'a> {
    /// 这一段的格子数。
    pub(crate) fn cell_count(&self) -> usize {
        self.span.len()
    }

    /// 段内第 `index` 个格子。
    pub(crate) fn cell(&self, index: usize) -> Option<IconGridCell<'_>> {
        if index >= self.span.len() {
            return None;
        }
        Some(match self.cells.content(self.span.start + index)? {
            IconGridCellContent::Entry {
                entry_index,
                transfer,
            } => IconGridCell::Entry(IconGridEntryCell {
                entry: self.entries.get(entry_index)?,
                entry_index,
                transfer,
            }),
            IconGridCellContent::Placeholder(placeholder) => IconGridCell::Placeholder(placeholder),
        })
    }

    /// 段内第 `index` 个格子的条目;占位格没有。
    pub(crate) fn entry(&self, index: usize) -> Option<&'a DirectoryEntry> {
        if index >= self.span.len() {
            return None;
        }
        self.entries
            .get(self.cells.entry_index(self.span.start + index)?)
    }

    /// 段内全部条目(按格子顺序,跳过占位)。
    pub(crate) fn entries(&self) -> impl Iterator<Item = &'a DirectoryEntry> + '_ {
        (0..self.cell_count()).filter_map(|index| self.entry(index))
    }

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

/// 不按格子一步步走的键盘移动：翻一屏（带视口高度）、到第一项、到最后一项。
#[derive(Debug, Clone, Copy)]
pub(crate) enum IconGridJump {
    PageUp(f32),
    PageDown(f32),
    First,
    Last,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IconGridNavigationTarget<'a> {
    pub(crate) directory: Option<&'a Path>,
    pub(crate) entry: &'a DirectoryEntry,
}

impl<'a> IconGridLayout<'a> {
    /// 测试用:不经窗格缓存,当场排格子再排几何。
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_root_transfer_placeholders(
        root_directory: Option<&'a Path>,
        root_entries: &'a [DirectoryEntry],
        root_placeholders: &[TransferPlaceholder],
        root_sort: TransferSortOptions,
        expanded_placeholders: &ExpandedTransferPlaceholderIndex,
        viewport_width: f32,
        height_bound: f32,
        icon_edge: u32,
        expansion: Option<&'a IconGridExpansionState>,
        root_grouping: Option<&crate::transfer_placeholder_view::root_grouping::RootGrouping>,
    ) -> Self {
        let expansion =
            expansion.filter(|state| Some(state.context().current_dir.as_path()) == root_directory);
        let plan = IconGridCellInputs {
            root: root_entries,
            root_placeholders,
            root_sort,
            grouping: root_grouping,
            children: visible_expansion_directories(expansion),
            expanded_placeholders,
        }
        .build(0);
        Self::from_plan(
            root_directory,
            root_entries,
            &plan,
            viewport_width,
            height_bound,
            icon_edge,
            expansion,
        )
    }

    /// 根面板与展开子面板的格子(传输占位已按各自排序合入、根面板已分组)取自 `plan`,
    /// 这里只排几何:行段、展开带、组头,按格子数推导;交互(键盘/命中/揭示)只认真实条目。
    pub(crate) fn from_plan(
        root_directory: Option<&'a Path>,
        root_entries: &'a [DirectoryEntry],
        plan: &IconGridCellPlan,
        viewport_width: f32,
        height_bound: f32,
        icon_edge: u32,
        expansion: Option<&'a IconGridExpansionState>,
    ) -> Self {
        let expansion =
            expansion.filter(|state| Some(state.context().current_dir.as_path()) == root_directory);
        let children_by_parent = expansion.map(index_visible_children);
        let builder = PanelBuilder {
            width: viewport_width.max(0.0),
            icon_edge,
            children_by_parent: children_by_parent.as_ref(),
            plan,
        };
        let (lead, groups) = match &plan.root_rows {
            IconGridRowsPlan::Full => (0..plan.root.len(), &[][..]),
            IconGridRowsPlan::GroupedFiles {
                directories,
                groups,
            } => (directories.clone(), groups.as_slice()),
        };
        let root = builder.panel(
            root_directory,
            root_entries,
            &plan.root,
            plan.root_status,
            lead,
            groups,
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
            return rows
                .entries()
                .map(|entry| entry.path().to_path_buf())
                .collect();
        }
        self.interactive_entry_geometry()
            .into_iter()
            .map(|entry| entry.entry.path().to_path_buf())
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
            let entries = rows.entries().collect::<Vec<_>>();
            let current_index = current_path.and_then(|current_path| {
                entries
                    .iter()
                    .position(|entry| entry.path() == current_path)
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
                .position(|entry| entry.entry.path() == current_path)
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

    /// 翻页落到同一列、大约一屏之外的那一行（少翻一行，上一屏的最后一行还在屏上）；
    /// 已经没有更远的行就到第一项 / 最后一项。没有当前项时和方向键一样从头或从尾开始。
    pub(crate) fn keyboard_jump_target(
        &self,
        current_path: Option<&Path>,
        jump: IconGridJump,
    ) -> Option<IconGridNavigationTarget<'a>> {
        let entries = self.interactive_entry_geometry();
        let first = *entries.first()?;
        let last = *entries.last()?;
        let current = current_path
            .and_then(|path| entries.iter().position(|entry| entry.entry.path() == path));
        let row_step = row_height(self.icon_edge);
        let target = match (jump, current) {
            (IconGridJump::First, _) | (IconGridJump::PageDown(_), None) => first,
            (IconGridJump::Last, _) | (IconGridJump::PageUp(_), None) => last,
            (IconGridJump::PageUp(viewport_height), Some(index)) => {
                let distance = (viewport_height - row_step).max(row_step);
                interaction_geometry::entry_one_page_away(&entries, index, -distance)
                    .unwrap_or(first)
            }
            (IconGridJump::PageDown(viewport_height), Some(index)) => {
                let distance = (viewport_height - row_step).max(row_step);
                interaction_geometry::entry_one_page_away(&entries, index, distance).unwrap_or(last)
            }
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
    /// 几何同源,滚动后的 hover 补偿由此与渲染瓦片落在同一格子上;
    /// 组头/占位/空白落点返回 None。
    pub(crate) fn entry_path_at_point(&self, point: Point) -> Option<&Path> {
        interaction_geometry::find_interactive_entry_at_point(
            &self.root,
            0.0,
            0.0,
            self.icon_edge,
            point,
        )
        .map(|entry| entry.path())
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

/// 排几何时整棵面板树共用的东西。
struct PanelBuilder<'a, 'p> {
    width: f32,
    icon_edge: u32,
    children_by_parent: Option<&'p IconGridChildrenByParent<'a>>,
    plan: &'p IconGridCellPlan,
}

impl<'a> PanelBuilder<'a, '_> {
    /// 一个面板的流段。`lead` 是铺成行的那截格子(未分组即全部);`groups` 只在分组的根面板非空,
    /// 按组追加[组头条, 组行段]。
    fn panel(
        &self,
        directory: Option<&'a Path>,
        entries: &'a [DirectoryEntry],
        cells: &Rc<IconGridCells>,
        status: IconGridPanelStatus,
        lead: IconGridCellSpan,
        groups: &[IconGridRootFileGroup],
    ) -> IconGridPanelLayout<'a> {
        let icon_edge = self.icon_edge;
        let column_count = column_count_for_width(self.width, icon_edge);
        if status != IconGridPanelStatus::Loaded || cells.is_empty() {
            return IconGridPanelLayout {
                status,
                height: ICON_GRID_CONTENT_PADDING * 2.0 + ICON_GRID_STATUS_HEIGHT,
                flow: Vec::new(),
            };
        }

        // 条目下标 -> 格子位置:格子自带真实条目下标(见 IconGridEntryCell),
        // 展开锚点按 path 校验出的条目下标经此映射到行号——分组重排与占位
        // 插入都不会让锚点漂移。不在铺成行的那截里(分组根的文件段)时
        // 回退原下标,与既有陈旧锚点的回退一致。
        let anchor_cell_index = |anchor_index: usize| {
            cells
                .position_of(anchor_index)
                .filter(|position| lead.contains(position))
                .map_or(anchor_index, |position| position - lead.start)
        };
        let rows_of = |span: IconGridCellSpan, start_row: usize, end_row: usize, top: f32| {
            IconGridFlowSegment::Rows(IconGridRowsLayout {
                directory,
                entries,
                cells: Rc::clone(cells),
                span,
                start_row,
                end_row,
                column_count,
                top,
                height: (end_row - start_row) as f32 * row_height(icon_edge),
            })
        };
        let total_rows = row_count_for_entries(lead.len(), column_count);
        let children = self
            .children_by_parent
            .zip(directory)
            .and_then(|(children, directory)| children.get(directory))
            .map(Vec::as_slice)
            .unwrap_or_default();

        let mut flow = Vec::with_capacity(children.len().saturating_mul(2).saturating_add(1));
        let mut next_row = 0;
        let mut top = ICON_GRID_CONTENT_PADDING;
        let mut child_cursor = 0;
        while child_cursor < children.len() {
            let anchor_row =
                anchor_cell_index(children[child_cursor].1.anchor_index) / column_count;
            let rows_end = anchor_row.saturating_add(1).min(total_rows);
            if next_row < rows_end {
                let segment = rows_of(lead.clone(), next_row, rows_end, top);
                top += segment.height();
                flow.push(segment);
                next_row = rows_end;
            }

            while child_cursor < children.len()
                && anchor_cell_index(children[child_cursor].1.anchor_index) / column_count
                    == anchor_row
            {
                let (child_path, child) = children[child_cursor];
                // 排格子与排几何读的是同一份展开状态(指纹里有显示中的展开目录)。
                let (child_cells, child_status) = &self.plan.children[child_path];
                let child_panel = self.panel(
                    Some(child_path),
                    &child.contents.entries,
                    child_cells,
                    *child_status,
                    0..child_cells.len(),
                    &[],
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
        // 展开带只推到锚点行;其后的行区(分组时是置顶目录的行区)接着入流。
        if next_row < total_rows {
            let segment = rows_of(lead.clone(), next_row, total_rows, top);
            top += segment.height();
            flow.push(segment);
        }
        // 分组根的文件段:每组输出[组头条, 组行段]。组头条是纯展示
        // chrome,高度计入流高与 reveal 数学,但不产生任何交互目标;
        // 划分层保证组非空,组行段至少一行。
        for group in groups {
            flow.push(IconGridFlowSegment::GroupHeader(
                IconGridGroupHeaderLayout {
                    title: group.title.clone(),
                    index_label: group.index_label.clone(),
                    count: group.span.len(),
                    top,
                    height: ICON_GRID_GROUP_HEADER_HEIGHT,
                },
            ));
            top += ICON_GRID_GROUP_HEADER_HEIGHT;
            let rows = row_count_for_entries(group.span.len(), column_count);
            let segment = rows_of(group.span.clone(), 0, rows, top);
            top += segment.height();
            flow.push(segment);
        }

        IconGridPanelLayout {
            status,
            height: top + ICON_GRID_CONTENT_PADDING,
            flow,
        }
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

/// 显示中的展开目录(排格子的输入)。
pub(crate) fn visible_expansion_directories(
    expansion: Option<&IconGridExpansionState>,
) -> Vec<(&Path, &crate::model::ExpandedDirectory)> {
    expansion
        .into_iter()
        .flat_map(|expansion| expansion.directories())
        .filter(|(_, directory)| directory.is_visible())
        .map(|(path, directory)| (path, &directory.contents))
        .collect()
}

#[cfg(test)]
#[path = "icon_grid_layout_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "icon_grid_layout_grouping_tests.rs"]
mod grouping_tests;
