//! 图标网格排好的格子：传输占位按排序合入、根面板分组之后的顺序。格子只记下标，不借用条目，
//! 帧之间留得住；输入的指纹没变就用上次排好的（与列表行流同一做法，见 `list_rows`）。
//! 每帧只按格子数排几何（行段、展开带、组头），不再从全部条目重建格子数组。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use file_core::DirectoryEntry;

use super::root_group_plan::{split_root_cells, IconGridRowsPlan};
use super::{ExpandedTransferPlaceholderIndex, IconGridPanelStatus};
use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};
use crate::transfer_placeholder_view::root_grouping::RootGrouping;
use crate::transfer_placeholders::{
    hash_transfer_placeholders, merge_entries_with_placeholders, MergedTransferItem,
    TransferPlaceholder, TransferSortOptions,
};

/// 格子：条目（所属目录条目里的下标）或传输占位（面板占位表里的下标）。
#[derive(Debug, Clone, Copy)]
enum IconGridSlot {
    Entry(u32),
    Placeholder(u32),
}

/// 一个面板排好的格子。10 万个条目约 0.8 MB；占位和同名传输装饰放旁表。
#[derive(Debug, Default)]
pub(crate) struct IconGridCells {
    slots: Vec<IconGridSlot>,
    placeholders: Vec<TransferPlaceholder>,
    /// （条目下标, 同名传输占位）按下标排好。
    decorations: Vec<(u32, TransferPlaceholder)>,
    /// 条目下标 → 格子位置：展开锚点按它找行。有展开时才建。
    positions: OnceLock<Vec<u32>>,
}

/// 格子里画什么：条目或传输占位。
#[derive(Debug, Clone, Copy)]
pub(crate) enum IconGridCellContent<'a> {
    /// 条目在所属目录条目里的下标（展开锚点以它为键），和同名传输占位（图标上叠进度环）。
    Entry {
        entry_index: usize,
        transfer: Option<&'a TransferPlaceholder>,
    },
    Placeholder(&'a TransferPlaceholder),
}

impl IconGridCells {
    fn from_items<'i>(items: impl IntoIterator<Item = &'i MergedTransferItem<'i>>) -> Self {
        let mut cells = Self::default();
        for item in items {
            cells.push(item);
        }
        cells
            .decorations
            .sort_by_key(|(entry_index, _)| *entry_index);
        cells
    }

    pub(super) fn push(&mut self, item: &MergedTransferItem<'_>) {
        match item {
            MergedTransferItem::Entry {
                index, transfer, ..
            } => {
                let index = *index as u32;
                if let Some(transfer) = transfer {
                    self.decorations.push((index, transfer.clone()));
                }
                self.slots.push(IconGridSlot::Entry(index));
            }
            MergedTransferItem::Placeholder(placeholder) => {
                self.placeholders.push(placeholder.clone());
                self.slots.push(IconGridSlot::Placeholder(
                    (self.placeholders.len() - 1) as u32,
                ));
            }
        }
    }

    pub(super) fn sort_decorations(&mut self) {
        self.decorations
            .sort_by_key(|(entry_index, _)| *entry_index);
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 第 `position` 个格子。
    pub(crate) fn content(&self, position: usize) -> Option<IconGridCellContent<'_>> {
        Some(match *self.slots.get(position)? {
            IconGridSlot::Entry(entry_index) => IconGridCellContent::Entry {
                entry_index: entry_index as usize,
                transfer: self
                    .decorations
                    .binary_search_by_key(&entry_index, |(decorated, _)| *decorated)
                    .ok()
                    .map(|found| &self.decorations[found].1),
            },
            IconGridSlot::Placeholder(placeholder) => {
                IconGridCellContent::Placeholder(&self.placeholders[placeholder as usize])
            }
        })
    }

    /// 第 `position` 个格子的条目下标；占位格没有。
    pub(crate) fn entry_index(&self, position: usize) -> Option<usize> {
        match *self.slots.get(position)? {
            IconGridSlot::Entry(entry_index) => Some(entry_index as usize),
            IconGridSlot::Placeholder(_) => None,
        }
    }

    /// 条目下标 → 格子位置。
    pub(crate) fn position_of(&self, entry_index: usize) -> Option<usize> {
        let positions = self.positions.get_or_init(|| {
            let mut positions = vec![u32::MAX; self.slots.len()];
            for (position, slot) in self.slots.iter().enumerate() {
                if let IconGridSlot::Entry(entry_index) = *slot {
                    if let Some(found) = positions.get_mut(entry_index as usize) {
                        *found = position as u32;
                    }
                }
            }
            positions
        });
        positions
            .get(entry_index)
            .filter(|position| **position != u32::MAX)
            .map(|position| *position as usize)
    }
}

/// 一段格子（根面板分组时每组是一截）在 `IconGridCells` 里的范围。
pub(crate) type IconGridCellSpan = Range<usize>;

/// 排好的根面板与展开子面板的格子。
#[derive(Debug)]
pub(crate) struct IconGridCellPlan {
    fingerprint: u64,
    pub(super) root: Rc<IconGridCells>,
    pub(super) root_rows: IconGridRowsPlan,
    pub(super) root_status: IconGridPanelStatus,
    /// 显示中的展开目录 → 它的格子和状态。
    pub(super) children: HashMap<PathBuf, (Rc<IconGridCells>, IconGridPanelStatus)>,
}

/// 窗格上次排好的格子。
pub(crate) type IconGridCellCache = RefCell<Option<Rc<IconGridCellPlan>>>;

/// 排格子读的全部输入（指纹另外要根层条目快照的版本号，见 `icon_grid_cell_plan`）。
pub(crate) struct IconGridCellInputs<'a> {
    pub(crate) root: &'a [DirectoryEntry],
    pub(crate) root_placeholders: &'a [TransferPlaceholder],
    pub(crate) root_sort: TransferSortOptions,
    pub(crate) grouping: Option<&'a RootGrouping>,
    /// 显示中的展开目录。
    pub(crate) children: Vec<(&'a Path, &'a ExpandedDirectory)>,
    pub(crate) expanded_placeholders: &'a ExpandedTransferPlaceholderIndex,
}

impl IconGridCellInputs<'_> {
    pub(crate) fn build(&self, fingerprint: u64) -> IconGridCellPlan {
        let merged =
            merge_entries_with_placeholders(self.root, self.root_placeholders, self.root_sort);
        let (root, root_rows) = split_root_cells(merged, self.grouping);
        // 分组开启时根单元格只含置顶目录,文件组也可能非空——两者皆空才算空态。
        let root_status = if root.is_empty() {
            IconGridPanelStatus::Empty
        } else {
            IconGridPanelStatus::Loaded
        };
        let children = self
            .children
            .iter()
            .map(|(path, directory)| (path.to_path_buf(), self.child_cells(path, directory)))
            .collect();
        IconGridCellPlan {
            fingerprint,
            root: Rc::new(root),
            root_rows,
            root_status,
            children,
        }
    }

    /// 展开子面板与根面板同一规则:目标为该目录的传入占位按该目录的排序合入;
    /// 空目录但有占位时按有内容渲染,顶掉空态。刷新中照常显示旧条目。
    fn child_cells(
        &self,
        path: &Path,
        directory: &ExpandedDirectory,
    ) -> (Rc<IconGridCells>, IconGridPanelStatus) {
        match directory.status {
            ExpandedDirectoryStatus::Loading => (Rc::default(), IconGridPanelStatus::Loading),
            ExpandedDirectoryStatus::Error => (Rc::default(), IconGridPanelStatus::Error),
            ExpandedDirectoryStatus::Loaded | ExpandedDirectoryStatus::Refreshing => {
                let (placeholders, sort) = self
                    .expanded_placeholders
                    .get(path)
                    .map(|(placeholders, sort)| (placeholders.as_slice(), *sort))
                    .unwrap_or((&[], self.root_sort));
                let merged =
                    merge_entries_with_placeholders(&directory.entries, placeholders, sort);
                let cells = IconGridCells::from_items(&merged);
                let status = if cells.is_empty() {
                    IconGridPanelStatus::Empty
                } else {
                    IconGridPanelStatus::Loaded
                };
                (Rc::new(cells), status)
            }
        }
    }

    /// 先解构：加了输入没算进指纹，编译不过。条目只算版本号；文件信息补上时快照不变，
    /// 只有分组读它或要插占位（按大小、修改时间比位置）时才算进去。
    fn fingerprint(&self, root_revision: u64, entry_metadata_revision: u64) -> u64 {
        let Self {
            root: _,
            root_placeholders,
            root_sort,
            grouping,
            children,
            expanded_placeholders,
        } = self;
        let mut hasher = DefaultHasher::new();
        (root_revision, root_sort, grouping).hash(&mut hasher);
        hash_transfer_placeholders(root_placeholders, &mut hasher);
        let reads_entry_metadata = grouping.is_some_and(RootGrouping::reads_entry_metadata)
            || !root_placeholders.is_empty()
            || !expanded_placeholders.is_empty();
        reads_entry_metadata
            .then_some(entry_metadata_revision)
            .hash(&mut hasher);
        // 两张表都按项相加，与遍历顺序无关。
        hasher.write_u64(unordered(children.iter().map(|(path, directory)| {
            let mut hasher = DefaultHasher::new();
            (
                path,
                directory.entries.revision(),
                std::mem::discriminant(&directory.status),
            )
                .hash(&mut hasher);
            hasher.finish()
        })));
        hasher.write_u64(unordered(expanded_placeholders.iter().map(
            |(path, (placeholders, sort))| {
                let mut hasher = DefaultHasher::new();
                (path, sort).hash(&mut hasher);
                hash_transfer_placeholders(placeholders, &mut hasher);
                hasher.finish()
            },
        )));
        hasher.finish()
    }
}

/// 窗格排好的格子：输入没变就是上次那份。
pub(crate) fn icon_grid_cell_plan(
    cache: &IconGridCellCache,
    inputs: &IconGridCellInputs<'_>,
    root_revision: u64,
    entry_metadata_revision: u64,
) -> Rc<IconGridCellPlan> {
    let fingerprint = inputs.fingerprint(root_revision, entry_metadata_revision);
    let mut cached = cache.borrow_mut();
    if let Some(plan) = cached
        .as_ref()
        .filter(|plan| plan.fingerprint == fingerprint)
    {
        return Rc::clone(plan);
    }
    let plan = Rc::new(inputs.build(fingerprint));
    *cached = Some(Rc::clone(&plan));
    plan
}

fn unordered(hashes: impl Iterator<Item = u64>) -> u64 {
    hashes.fold(0, u64::wrapping_add)
}
