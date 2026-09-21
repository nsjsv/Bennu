//! 根目录分组划分层:分组是纯视图概念,只对当前目录(根)生效——列表
//! 行流与大图网格共用本划分器,展开子目录递归摊平时 directory 已经是
//! 子目录路径,天然拿不到划分器。
//! 目录(含目录占位)稳定置顶,文件段经 `partition_files_into_groups`
//! 做稳定划分——组内保持合并流原序(沿用现有排序),组间按秩排列。

use std::path::Path;
use std::time::SystemTime;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection};

use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::config::UiLanguage;
use crate::localization::current_language;
use crate::model::{
    date_group_key, dynamic_size_buckets, kind_category_group_key, name_initial_group_key,
    partition_files_into_groups, size_group_bucket_index, FileGroupKey, FileGroupSection,
    FileGroupingContext, FileGroupingMode,
};
use crate::transfer_placeholders::MergedTransferItem;

/// 根目录分组划分器的全部入参:模式、与根排序同源的方向、组标题
/// 本地化语言、日期桶锚点(每次渲染取一次,同帧行流键一致)。
pub(crate) struct RootGrouping {
    pub(crate) mode: FileGroupingMode,
    pub(crate) direction: SortDirection,
    pub(crate) language: UiLanguage,
    pub(crate) now: SystemTime,
}

impl RootGrouping {
    /// 仅当分组开启且 directory 是当前目录时返回划分器。分组关闭与
    /// 展开子目录都返回 None,行流走原有平铺分支(分组关闭回归保护)。
    pub(crate) fn for_pane_root(
        browser: &FileBrowser,
        pane: BrowserPaneView<'_>,
        directory: &Path,
    ) -> Option<Self> {
        let mode = browser.user_config().file_grouping;
        if mode == FileGroupingMode::None || directory != pane.current_dir.as_path() {
            return None;
        }
        Some(Self {
            mode,
            direction: browser.options.sort_direction,
            language: current_language(),
            now: SystemTime::now(),
        })
    }

    /// 文件段(已剔除置顶目录)的稳定划分:组内保持输入序,降序只反转
    /// 组序列;分组键按维度取显示同源的条目元数据。元数据解析由调用方
    /// 以闭包注入(列表/网格各自绑定 pane 的显示同源入口),划分器本体
    /// 不感知视图类型,列表行流与网格流段共用同一实现。
    pub(crate) fn partition<'section, 'entries>(
        &'section self,
        files: &'section [MergedTransferItem<'entries>],
        metadata_for_entry: impl Fn(&DirectoryEntry) -> EntryMetadata,
    ) -> Vec<FileGroupSection<'section, MergedTransferItem<'entries>>> {
        let resolve_metadata = &metadata_for_entry;
        let size_buckets = self.size_buckets(files, resolve_metadata);
        partition_files_into_groups(
            files,
            FileGroupingContext {
                direction: self.direction,
                language: self.language,
            },
            |item| self.group_key(item, resolve_metadata, &size_buckets),
        )
    }

    /// 大小维度的桶由当前文件段的真实尺寸集合现算(设计:桶边界随目录
    /// 内容变化是预期行为);未知尺寸不参与算桶。
    fn size_buckets(
        &self,
        files: &[MergedTransferItem<'_>],
        metadata_for_entry: &dyn Fn(&DirectoryEntry) -> EntryMetadata,
    ) -> Vec<(u64, u64)> {
        let mut sizes: Vec<u64> = files
            .iter()
            .filter_map(|item| self.item_size_bytes(item, metadata_for_entry))
            .collect();
        sizes.sort_unstable();
        dynamic_size_buckets(&sizes)
    }

    fn group_key(
        &self,
        item: &MergedTransferItem<'_>,
        metadata_for_entry: &dyn Fn(&DirectoryEntry) -> EntryMetadata,
        size_buckets: &[(u64, u64)],
    ) -> FileGroupKey {
        match self.mode {
            FileGroupingMode::NameInitial => {
                FileGroupKey::NameInitial(name_initial_group_key(merged_item_name(item)))
            }
            FileGroupingMode::Kind => {
                FileGroupKey::Kind(kind_category_group_key(merged_item_name(item)))
            }
            FileGroupingMode::Size => {
                // 占位 total_bytes 未知 → u64::MAX 归最末兜底组(模型层约定)。
                let size_bytes = self
                    .item_size_bytes(item, metadata_for_entry)
                    .unwrap_or(u64::MAX);
                let bucket_index = size_group_bucket_index(size_buckets, size_bytes);
                // 桶集为空(文件段全部尺寸未知)时按条目自身值自成一组。
                let (min_bytes, max_bytes) = size_buckets
                    .get(bucket_index)
                    .copied()
                    .unwrap_or((size_bytes, size_bytes));
                FileGroupKey::Size {
                    bucket_index,
                    bucket_count: size_buckets.len().max(1),
                    min_bytes,
                    max_bytes,
                }
            }
            FileGroupingMode::ModifiedTime
            | FileGroupingMode::CreatedTime
            | FileGroupingMode::AccessedTime => FileGroupKey::Date(date_group_key(
                self.item_group_timestamp(item, metadata_for_entry),
                self.now,
            )),
            // for_pane_root 边界已过滤 None 模式,此处不可达;保留以穷尽匹配。
            FileGroupingMode::None => unreachable!("grouping mode None never reaches partition"),
        }
    }

    /// 条目取显示同源的元数据(与列单元格/网格瓦片同一来源,元数据增量
    /// 到达后分组自然重算);占位取已知 total_bytes。
    fn item_size_bytes(
        &self,
        item: &MergedTransferItem<'_>,
        metadata_for_entry: &dyn Fn(&DirectoryEntry) -> EntryMetadata,
    ) -> Option<u64> {
        match item {
            MergedTransferItem::Entry { entry, .. } => Some(metadata_for_entry(entry).len),
            MergedTransferItem::Placeholder(placeholder) => placeholder.total_bytes,
        }
    }

    /// 日期维度的时间键:条目取对应元数据时间戳(未加载 None → Earlier);
    /// 占位行只有入队时刻可用,创建/访问维度字段缺失同样归 Earlier 兜底。
    fn item_group_timestamp(
        &self,
        item: &MergedTransferItem<'_>,
        metadata_for_entry: &dyn Fn(&DirectoryEntry) -> EntryMetadata,
    ) -> Option<SystemTime> {
        match item {
            MergedTransferItem::Entry { entry, .. } => {
                let metadata = metadata_for_entry(entry);
                match self.mode {
                    FileGroupingMode::ModifiedTime => metadata.modified,
                    FileGroupingMode::CreatedTime => metadata.created,
                    FileGroupingMode::AccessedTime => metadata.accessed,
                    _ => None,
                }
            }
            MergedTransferItem::Placeholder(placeholder) => match self.mode {
                FileGroupingMode::ModifiedTime => Some(placeholder.enqueued_at),
                _ => None,
            },
        }
    }
}

/// 目录(含目录占位)稳定前置:分组开启即目录置顶,不受 directories_first
/// 开关影响;两段内部都保持合并流原序。
pub(crate) fn split_directories_first<'a>(
    merged: Vec<MergedTransferItem<'a>>,
) -> (Vec<MergedTransferItem<'a>>, Vec<MergedTransferItem<'a>>) {
    let (directories, files): (Vec<_>, Vec<_>) =
        merged.into_iter().partition(merged_item_is_directory);
    (directories, files)
}

fn merged_item_is_directory(item: &MergedTransferItem<'_>) -> bool {
    match item {
        // 只有真实目录档置顶;符号链接等按文件参与分组。
        MergedTransferItem::Entry { entry, .. } => entry.kind == FileKind::Directory,
        MergedTransferItem::Placeholder(placeholder) => placeholder.is_directory,
    }
}

/// 分组键用的名字:非 UTF-8 名按空名走 `#` 兜底组,与首字母键约定一致。
fn merged_item_name<'item>(item: &'item MergedTransferItem<'_>) -> &'item str {
    match item {
        MergedTransferItem::Entry { entry, .. } => entry.name().to_str().unwrap_or(""),
        MergedTransferItem::Placeholder(placeholder) => &placeholder.name,
    }
}
