//! 列表的合并行流：根层条目与各展开目录的子行按显示顺序摊平，传输占位按各自
//! 目录的排序插入，分组只作用于根层。
//!
//! 行流是派生数据，按输入记住：输入的指纹没变就用上次排好的。滚动、悬停、选中、缩略图到达都不改
//! 输入，10 万个文件的目录每帧从头排一遍要十几毫秒、十几 MB 临时内存（2026-11-18 实测）。没有改成在
//! 每个改动入口作废缓存：入口散在几十个处理函数里，漏一个就显示旧内容。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::list_row_layout::ListRowLayout;
use super::root_grouping::{split_directories_first, RootGrouping};
use super::{transfer_sort_for_expanded, transfer_sort_for_root};
use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::list_view::list_snippet_line_height;
use crate::model::{DirectoryEntrySnapshot, ExpandedDirectory};
use crate::transfer_placeholders::{
    hash_transfer_placeholders, merge_entries_with_placeholders,
    transfer_placeholders_for_directory, MergedTransferItem, TransferPlaceholder,
    TransferSortOptions,
};
use crate::visible_entries::{visible_entry_status_row, ListEntryHeights};

/// 窗格上次排好的行流。
pub(crate) type ListRowCache = RefCell<Option<Rc<ListRowLayout>>>;

/// 窗格的列表行流：输入没变就是上次那份。调用方在自己的函数里用完，期间不改模型（`row` 按下标现取条目）。
pub(crate) fn list_row_layout(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    row_height: f32,
) -> Rc<ListRowLayout> {
    let sources = ListRowSources::of(browser, pane, row_height);
    let fingerprint = sources.fingerprint();
    let mut cached = pane.list_rows.borrow_mut();
    if let Some(layout) = cached
        .as_ref()
        .filter(|layout| layout.fingerprint() == fingerprint)
    {
        return Rc::clone(layout);
    }
    let layout = Rc::new(sources.build(fingerprint));
    *cached = Some(Rc::clone(&layout));
    layout
}

/// 排行流读的全部输入：只从这里建，指纹也只从这里算。
struct ListRowSources<'a> {
    root: &'a DirectoryEntrySnapshot,
    root_directory: Option<&'a Path>,
    root_sort: TransferSortOptions,
    grouping: Option<RootGrouping>,
    expanded: &'a HashMap<PathBuf, ExpandedDirectory>,
    /// 根目录和显示中的展开目录正在传入的占位；没有占位的目录不在表里。
    placeholders: HashMap<&'a Path, Vec<TransferPlaceholder>>,
    heights: ListEntryHeights<'a>,
    entry_metadata_revision: u64,
}

impl<'a> ListRowSources<'a> {
    fn of(browser: &FileBrowser, pane: BrowserPaneView<'a>, row_height: f32) -> Self {
        // 传输只会落进真实目录，回收站、搜索的根层没有占位。
        let shown_expanded = pane
            .expanded_directories
            .iter()
            .filter(|(_, expanded)| expanded.is_expanded || expanded.is_collapsing)
            .map(|(path, _)| path.as_path());
        let placeholders = pane
            .current_directory()
            .into_iter()
            .chain(shown_expanded)
            .filter_map(|directory| {
                let placeholders =
                    transfer_placeholders_for_directory(&browser.operation_queue, directory);
                (!placeholders.is_empty()).then_some((directory, placeholders))
            })
            .collect();
        Self {
            root: pane.entries,
            root_directory: pane.current_directory(),
            root_sort: transfer_sort_for_root(browser),
            grouping: RootGrouping::for_pane_root(browser, pane.location),
            expanded: pane.expanded_directories,
            placeholders,
            heights: ListEntryHeights::at(
                pane.location,
                row_height,
                list_snippet_line_height(browser.user_config().list_view_density),
            ),
            entry_metadata_revision: browser.entry_metadata_revision,
        }
    }

    /// 先解构：加了输入没算进指纹，编译不过。大块数据（条目、摘要表）只算版本号。
    fn fingerprint(&self) -> u64 {
        let Self {
            root,
            root_directory,
            root_sort,
            grouping,
            expanded,
            placeholders,
            heights,
            entry_metadata_revision,
        } = self;
        let mut hasher = DefaultHasher::new();
        (
            root.revision(),
            root_directory,
            root_sort,
            grouping,
            heights,
        )
            .hash(&mut hasher);
        // 条目里的文件信息补上时快照不变。只有按大小、日期分组和插入占位（按大小、修改时间比位置）
        // 读它，只在这时算进去；否则滚动中可见行补信息会让行流一直重排。
        let reads_entry_metadata = grouping
            .as_ref()
            .is_some_and(RootGrouping::reads_entry_metadata)
            || !placeholders.is_empty();
        reads_entry_metadata
            .then_some(*entry_metadata_revision)
            .hash(&mut hasher);
        // 两张表都按项相加，与遍历顺序无关。
        hasher.write_u64(unordered(placeholders.iter().map(|(directory, list)| {
            let mut hasher = DefaultHasher::new();
            directory.hash(&mut hasher);
            hash_transfer_placeholders(list, &mut hasher);
            hasher.finish()
        })));
        hasher.write_u64(unordered(expanded.iter().map(|(path, directory)| {
            let mut hasher = DefaultHasher::new();
            (
                path,
                directory.entries.revision(),
                std::mem::discriminant(&directory.status),
                directory.is_expanded,
                directory.is_collapsing,
                directory.animation_progress.to_bits(),
                transfer_sort_for_expanded(*root_sort, directory),
            )
                .hash(&mut hasher);
            hasher.finish()
        })));
        hasher.finish()
    }

    fn build(&self, fingerprint: u64) -> ListRowLayout {
        let mut layout = ListRowLayout::new(
            fingerprint,
            self.heights.row_height(),
            self.heights.snippet_line_height(),
        );
        let root = layout.add_segment(None, 0, 1.0);
        let merged = merge_entries_with_placeholders(
            self.root,
            self.placeholders_in(self.root_directory),
            self.root_sort,
        );
        let Some(grouping) = &self.grouping else {
            for item in &merged {
                self.push_item(&mut layout, root, item);
            }
            return layout;
        };
        // 分组开启:目录(含目录占位)稳定置顶(不受 directories_first 影响),
        // 文件段按组分段,每组先输出组头行再摊平组内条目/占位行。
        let (directories, files) = split_directories_first(merged);
        for item in &directories {
            self.push_item(&mut layout, root, item);
        }
        for section in grouping.partition(&files) {
            layout.push_group_header(
                section.descriptor.title,
                section.descriptor.index_label,
                section.items.len(),
            );
            for item in section.items {
                self.push_item(&mut layout, root, item);
            }
        }
        layout
    }

    fn placeholders_in(&self, directory: Option<&Path>) -> &[TransferPlaceholder] {
        directory
            .and_then(|directory| self.placeholders.get(directory))
            .map_or(&[], Vec::as_slice)
    }

    /// 单个合并项:条目行后紧跟其展开子行,占位行直接入流。
    fn push_item(&self, layout: &mut ListRowLayout, segment: u32, item: &MergedTransferItem<'_>) {
        match item {
            MergedTransferItem::Entry {
                entry,
                index,
                transfer,
            } => {
                layout.push_entry(
                    segment,
                    *index,
                    self.heights.snippet_height(entry),
                    transfer.clone(),
                );
                self.push_expansion(layout, segment, entry.path());
            }
            // 占位行参与条纹交替。被同名真实条目替换时序号 1:1 传承,行相位不跳变;
            // 仅取消且未落地时后续行移一次相位,与任何普通行增删一致。
            MergedTransferItem::Placeholder(placeholder) => {
                layout.push_placeholder(placeholder.clone())
            }
        }
    }

    /// 展开目录的状态行与子行:折叠中仍显示,Loading 不暴露子条目,Error/空目录显示状态行;
    /// 刷新中照常显示旧条目,结果落地整体替换(磁盘变化触发的重扫不闪)。
    fn push_expansion(&self, layout: &mut ListRowLayout, parent: u32, path: &Path) {
        let Some(expanded) = self
            .expanded
            .get(path)
            .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
        else {
            return;
        };
        let (depth, progress) = layout.segment_depth_progress(parent);
        let expansion_progress = expanded.animation_progress.clamp(0.0, 1.0);
        if let Some(status) = visible_entry_status_row(expanded) {
            layout.push_status(status, depth + 1, layout.row_height() * expansion_progress);
        }
        if !expanded.status.has_listing() {
            return;
        }
        let children = layout.add_segment(
            Some(path.to_path_buf()),
            depth + 1,
            progress * expansion_progress,
        );
        let merged = merge_entries_with_placeholders(
            &expanded.entries,
            self.placeholders_in(Some(path)),
            transfer_sort_for_expanded(self.root_sort, expanded),
        );
        for item in &merged {
            self.push_item(layout, children, item);
        }
    }
}

fn unordered(hashes: impl Iterator<Item = u64>) -> u64 {
    hashes.fold(0, u64::wrapping_add)
}

/// 测试用：根目录的占位直接给，不用拼操作队列任务；其余输入照常从窗格取。
#[cfg(test)]
pub(super) fn list_row_layout_with_root_placeholders(
    browser: &FileBrowser,
    pane: BrowserPaneView<'_>,
    row_height: f32,
    placeholders: Vec<TransferPlaceholder>,
) -> ListRowLayout {
    let mut sources = ListRowSources::of(browser, pane, row_height);
    if let Some(directory) = sources.root_directory {
        sources.placeholders.insert(directory, placeholders);
    }
    sources.build(0)
}
