//! 大图根面板的分组计划:复用列表侧的 RootGrouping 划分器(单一划分
//! 事实源,禁止复制划分逻辑),把根合并流切成"置顶目录单元格 + 按组
//! 的文件段"。分组是纯视图概念,划分只在排格子时发生,不触目录数据。

use crate::transfer_placeholder_view::root_grouping::{split_directories_first, RootGrouping};
use crate::transfer_placeholders::MergedTransferItem;

use super::cell_plan::{IconGridCellSpan, IconGridCells};

/// 分组后的一个文件组:组头文案与索引栏短标签(构造时已本地化的
/// 成品)+ 组内格子在根面板格子里的那一截(保持合并流原序,沿用现有排序)。
#[derive(Debug, Clone)]
pub(crate) struct IconGridRootFileGroup {
    pub(crate) title: String,
    pub(crate) index_label: String,
    pub(crate) span: IconGridCellSpan,
}

/// 根面板行区计划:未分组时行铺满全部格子;分组开启时行区只覆盖
/// 置顶的目录格子(`directories`),文件段由 build_panel 按组追加[组头条, 组行段]。
#[derive(Debug, Clone)]
pub(crate) enum IconGridRowsPlan {
    Full,
    GroupedFiles {
        directories: IconGridCellSpan,
        groups: Vec<IconGridRootFileGroup>,
    },
}

/// 根合并流 -> (根格子, 行区计划)。分组关闭:合并序即格子序。分组开启:目录(含目录占位)
/// 稳定置顶,文件段按组依次接在后面。格子的条目下标取合并项带的下标,重排后展开锚点也不漂移。
pub(crate) fn split_root_cells(
    merged: Vec<MergedTransferItem<'_>>,
    grouping: Option<&RootGrouping>,
) -> (IconGridCells, IconGridRowsPlan) {
    let mut cells = IconGridCells::default();
    let Some(grouping) = grouping else {
        for item in &merged {
            cells.push(item);
        }
        cells.sort_decorations();
        return (cells, IconGridRowsPlan::Full);
    };
    let (directories, files) = split_directories_first(merged);
    for item in &directories {
        cells.push(item);
    }
    let directories = 0..cells.len();
    let groups = grouping
        .partition(&files)
        .into_iter()
        .map(|section| {
            let start = cells.len();
            for item in &section.items {
                cells.push(item);
            }
            IconGridRootFileGroup {
                title: section.descriptor.title,
                index_label: section.descriptor.index_label,
                span: start..cells.len(),
            }
        })
        .collect();
    cells.sort_decorations();
    (
        cells,
        IconGridRowsPlan::GroupedFiles {
            directories,
            groups,
        },
    )
}
