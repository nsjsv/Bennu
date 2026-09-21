//! 大图根面板的分组计划:复用列表侧的 RootGrouping 划分器(单一划分
//! 事实源,禁止复制划分逻辑),把根合并流切成"置顶目录单元格 + 按组
//! 的文件段"。分组是纯视图概念,划分只在布局构建期发生,不触目录数据。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use file_core::{DirectoryEntry, EntryMetadata};

use crate::transfer_placeholder_view::root_grouping::{split_directories_first, RootGrouping};
use crate::transfer_placeholders::MergedTransferItem;

use super::{IconGridCell, IconGridEntryCell};

/// 根面板分组入参:划分器 + 显示同源的元数据解析。网格布局不感知
/// BrowserPaneView,解析闭包由调用方绑定 pane 提供(与列表同一来源)。
pub(crate) struct IconGridRootGrouping<'a> {
    pub(crate) grouping: &'a RootGrouping,
    pub(crate) metadata_for_entry: &'a dyn Fn(&DirectoryEntry) -> EntryMetadata,
}

/// 分组后的一个文件组:组头文案与索引栏短标签(构造时已本地化的
/// 成品)+ 组内单元格(保持合并流原序,沿用现有排序)。
#[derive(Debug)]
pub(crate) struct IconGridRootFileGroup<'a> {
    pub(crate) title: String,
    pub(crate) index_label: String,
    pub(crate) count: usize,
    pub(crate) cells: Arc<[IconGridCell<'a>]>,
}

/// 根面板行区计划:未分组时行铺满全部单元格;分组开启时行区只覆盖
/// 置顶的目录单元格,文件段由 build_panel 按组追加[组头条, 组行段]。
#[derive(Debug)]
pub(crate) enum IconGridRowsPlan<'a> {
    Full,
    GroupedFiles(Vec<IconGridRootFileGroup<'a>>),
}

impl IconGridRowsPlan<'_> {
    pub(crate) fn has_any_group_cells(&self) -> bool {
        match self {
            Self::Full => false,
            Self::GroupedFiles(groups) => groups.iter().any(|group| !group.cells.is_empty()),
        }
    }
}

/// 根合并流 -> (根单元格数组, 行区计划)。分组关闭:合并序即单元格序,
/// 条目下标按出现序计数(与条目数组同序)。分组开启:目录(含目录占位)
/// 稳定置顶,文件段按组切分;单元格被重排后条目下标必须按 path 查表
/// 还原成条目数组的真实下标,展开锚点才不会因分组漂移。
pub(crate) fn split_root_cells<'a>(
    merged: Vec<MergedTransferItem<'a>>,
    root_entries: &[DirectoryEntry],
    grouping: Option<IconGridRootGrouping<'_>>,
) -> (Arc<[IconGridCell<'a>]>, IconGridRowsPlan<'a>) {
    let Some(input) = grouping else {
        // 合并契约:条目相对顺序保持条目数组原序,出现序即真实下标。
        let mut next_entry_index = 0usize;
        let cells = merged
            .into_iter()
            .map(|item| match item {
                MergedTransferItem::Entry { entry, transfer } => {
                    let cell = IconGridCell::Entry(IconGridEntryCell {
                        entry,
                        entry_index: next_entry_index,
                        transfer,
                    });
                    next_entry_index += 1;
                    cell
                }
                MergedTransferItem::Placeholder(placeholder) => {
                    IconGridCell::Placeholder(placeholder)
                }
            })
            .collect();
        return (cells, IconGridRowsPlan::Full);
    };

    let entry_index_of_path: HashMap<&Path, usize> = root_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path.as_path(), index))
        .collect();
    let cell_of = |item: &MergedTransferItem<'a>| match item {
        MergedTransferItem::Entry { entry, transfer } => IconGridCell::Entry(IconGridEntryCell {
            entry,
            entry_index: entry_index_of_path[entry.path.as_path()],
            transfer: transfer.clone(),
        }),
        // 占位是合并时逐个克隆出的临时集合,按引用再克隆一份成本可忽略。
        MergedTransferItem::Placeholder(placeholder) => {
            IconGridCell::Placeholder(placeholder.clone())
        }
    };
    let (directories, files) = split_directories_first(merged);
    let directory_cells = directories.iter().map(&cell_of).collect::<Arc<[_]>>();
    let groups = input
        .grouping
        .partition(&files, |entry| (input.metadata_for_entry)(entry))
        .into_iter()
        .map(|section| {
            let count = section.items.len();
            let cells = section.items.iter().map(|item| cell_of(item)).collect();
            IconGridRootFileGroup {
                title: section.descriptor.title,
                index_label: section.descriptor.index_label,
                count,
                cells,
            }
        })
        .collect();
    (directory_cells, IconGridRowsPlan::GroupedFiles(groups))
}
