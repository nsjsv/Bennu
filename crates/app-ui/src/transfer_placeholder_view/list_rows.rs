//! 列表的合并行流：根层条目与各展开目录的子行按显示顺序摊平，传输占位按各自
//! 目录的排序插入，分组只作用于根层。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;

use super::root_grouping;
use super::{transfer_sort_for_expanded, transfer_sort_for_root, ListTransferRow};
use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::list_view::LIST_GROUP_HEADER_HEIGHT;
use crate::model::ExpandedDirectoryStatus;
use crate::transfer_placeholders::{
    merge_entries_with_placeholders, transfer_placeholders_for_directory, MergedTransferItem,
    TransferPlaceholder, TransferSortOptions,
};
use crate::visible_entries::{ListEntryHeights, VisibleEntry};

/// 构建列表合并行流:根层与各展开目录分别派生占位并按各自排序合入。
pub(crate) fn build_list_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    row_height: f32,
) -> Vec<ListTransferRow<'a>> {
    let mut placeholder_cache: HashMap<PathBuf, Vec<TransferPlaceholder>> = HashMap::new();
    let mut rows = Vec::new();
    let mut stripe_index = 0usize;
    push_root_transfer_rows(
        browser,
        pane,
        row_height,
        &mut placeholder_cache,
        &mut stripe_index,
        &mut rows,
    );
    rows
}

/// 根层：当前位置自己的条目。分组只作用于根层，展开的子目录平铺。
pub(super) fn push_root_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    stripe_index: &mut usize,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    // 传输只会落进真实目录，回收站没有占位。
    let placeholders = pane
        .current_directory()
        .map(|directory| cached_transfer_placeholders(browser, placeholder_cache, directory))
        .unwrap_or_default();
    let merged = merge_entries_with_placeholders(
        pane.entries,
        &placeholders,
        transfer_sort_for_root(browser),
    );
    let Some(grouping) = root_grouping::RootGrouping::for_pane_root(browser, pane.location) else {
        for item in &merged {
            push_list_transfer_item(
                browser,
                pane,
                item,
                0,
                1.0,
                row_height,
                placeholder_cache,
                stripe_index,
                rows,
            );
        }
        return;
    };
    // 分组开启:目录(含目录占位)稳定置顶(不受 directories_first 影响),
    // 文件段按组分段,每组先输出组头行再摊平组内条目/占位行。
    let (directories, files) = root_grouping::split_directories_first(merged);
    for item in &directories {
        push_list_transfer_item(
            browser,
            pane,
            item,
            0,
            1.0,
            row_height,
            placeholder_cache,
            stripe_index,
            rows,
        );
    }
    for section in grouping.partition(&files, |entry| pane.metadata_for_entry(entry)) {
        rows.push(ListTransferRow::GroupHeader {
            title: section.descriptor.title,
            index_label: section.descriptor.index_label,
            count: section.items.len(),
            height: LIST_GROUP_HEADER_HEIGHT,
        });
        for item in section.items {
            push_list_transfer_item(
                browser,
                pane,
                item,
                0,
                1.0,
                row_height,
                placeholder_cache,
                stripe_index,
                rows,
            );
        }
    }
}

fn cached_transfer_placeholders(
    browser: &FileBrowser,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    directory: &Path,
) -> Vec<TransferPlaceholder> {
    placeholder_cache
        .entry(directory.to_path_buf())
        .or_insert_with(|| transfer_placeholders_for_directory(&browser.operation_queue, directory))
        .clone()
}

/// 展开目录的子行，平铺不分组。
#[allow(clippy::too_many_arguments)]
fn push_directory_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    entries: &'a [DirectoryEntry],
    directory: &Path,
    sort: TransferSortOptions,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    stripe_index: &mut usize,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    let placeholders = cached_transfer_placeholders(browser, placeholder_cache, directory);
    let merged = merge_entries_with_placeholders(entries, &placeholders, sort);
    for item in &merged {
        push_list_transfer_item(
            browser,
            pane,
            item,
            depth,
            animation_progress,
            row_height,
            placeholder_cache,
            stripe_index,
            rows,
        );
    }
}

/// 单个合并项的行摊平:条目行后紧跟其展开子行,占位行直接入流。
/// 组头行不经过此处——它不属于任何合并项,由分组划分单独插入。
/// stripe_index 是整条行流共用的条纹计数器(见 ListTransferRow)。
#[allow(clippy::too_many_arguments)]
fn push_list_transfer_item<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    item: &MergedTransferItem<'a>,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    stripe_index: &mut usize,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    match item {
        MergedTransferItem::Entry { entry, transfer } => {
            rows.push(ListTransferRow::Entry {
                visible: VisibleEntry {
                    entry,
                    depth,
                    animation_progress,
                },
                transfer: transfer.clone(),
                stripe_index: *stripe_index,
                snippet_height: ListEntryHeights::at(pane.location, row_height)
                    .snippet_height(entry),
            });
            *stripe_index += 1;
            push_expansion_transfer_rows(
                browser,
                pane,
                entry,
                depth,
                animation_progress,
                row_height,
                placeholder_cache,
                stripe_index,
                rows,
            );
        }
        // 占位是合并时逐个克隆出的临时集合,按引用再克隆一份成本可忽略。
        MergedTransferItem::Placeholder(placeholder) => {
            rows.push(ListTransferRow::Placeholder {
                placeholder: placeholder.clone(),
                stripe_index: *stripe_index,
            });
            // 占位行参与条纹交替。被同名真实条目替换时序号 1:1 传承,
            // 行相位不跳变;仅取消且未落地时后续行移一次相位,与任何
            // 普通行增删一致。
            *stripe_index += 1;
        }
    }
}

/// 展开目录的状态行与子行:与 visible_entries 的可见性规则一致
/// (折叠中仍显示,Loading 不暴露子条目,Error/空目录显示状态行)。
#[allow(clippy::too_many_arguments)]
fn push_expansion_transfer_rows<'a>(
    browser: &FileBrowser,
    pane: BrowserPaneView<'a>,
    entry: &'a DirectoryEntry,
    depth: usize,
    animation_progress: f32,
    row_height: f32,
    placeholder_cache: &mut HashMap<PathBuf, Vec<TransferPlaceholder>>,
    stripe_index: &mut usize,
    rows: &mut Vec<ListTransferRow<'a>>,
) {
    let Some(expanded) = pane
        .expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return;
    };
    if let Some(status) = crate::visible_entries::visible_entry_status_row(expanded) {
        rows.push(ListTransferRow::DirectoryStatusRow {
            status,
            depth: depth + 1,
            height: row_height * expanded.animation_progress.clamp(0.0, 1.0),
            stripe_index: *stripe_index,
        });
        *stripe_index += 1;
    }
    if !matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
        return;
    }
    let child_progress = animation_progress * expanded.animation_progress.clamp(0.0, 1.0);
    push_directory_transfer_rows(
        browser,
        pane,
        &expanded.entries,
        entry.path.as_path(),
        transfer_sort_for_expanded(browser, expanded),
        depth + 1,
        child_progress,
        row_height,
        placeholder_cache,
        stripe_index,
        rows,
    );
}
