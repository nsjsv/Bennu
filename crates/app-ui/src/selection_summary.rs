use file_core::{DirectoryEntry, FileKind};

/// 窗格底部选中统计:文件夹只计数量,大小只累加非文件夹条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PaneSelectionSummary {
    pub(crate) directory_count: usize,
    pub(crate) file_count: usize,
    pub(crate) file_total_bytes: u64,
}

impl PaneSelectionSummary {
    pub(crate) fn is_empty(&self) -> bool {
        self.directory_count == 0 && self.file_count == 0
    }
}

/// 底部工具栏单窗格状态:选中统计(无选中为空)+ 当前目录可见文件总大小。
/// 总大小由文件系统事实按 show_hidden_files 推导,加载中为 None(显 "-")。
/// 隐藏条目数是原样事实,两种开关状态都显示;None = 未加载,幽灵组隐藏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneStatusStripEntry {
    pub(crate) selection: Option<PaneSelectionSummary>,
    pub(crate) visible_files_total_size_bytes: Option<u64>,
    pub(crate) hidden_entry_count: Option<usize>,
}

/// 状态栏的当前目录统计。由显示着的条目求和，与列表的大小列、选中统计用同一份文件信息；原来
/// 另起一遍读目录、逐个 stat，结果算完就扔，目录里有一个文件变化就整遍重来（10 万个文件一次约 1 秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectoryFileTotals {
    /// 可见文件（不含文件夹）的总大小；有文件的大小还没补上时为 `None`（显示 "-"）。
    pub(crate) visible_files_bytes: Option<u64>,
    pub(crate) hidden_entries: usize,
}

/// 窗格上次算的统计和它依据的输入：条目快照版本号、显不显示隐藏文件、扫描时见到的隐藏条目数；
/// 还没补齐时另加文件信息版本号（补齐之后格子不会再变，只看快照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectoryTotalsKey {
    pub(crate) entries_revision: u64,
    pub(crate) show_hidden: bool,
    pub(crate) hidden_entries_seen: usize,
    pub(crate) entry_metadata_revision: u64,
}

pub(crate) type DirectoryTotalsCache =
    std::cell::RefCell<Option<(DirectoryTotalsKey, DirectoryFileTotals)>>;

/// 输入没变就是上次那份，否则在 `entries` 上重算一遍（只读内存，不碰磁盘）。
pub(crate) fn directory_file_totals<'a>(
    cache: &DirectoryTotalsCache,
    key: DirectoryTotalsKey,
    entries: impl IntoIterator<Item = &'a DirectoryEntry>,
) -> DirectoryFileTotals {
    let mut cached = cache.borrow_mut();
    if let Some((cached_key, totals)) = *cached {
        let same_listing = (
            cached_key.entries_revision,
            cached_key.show_hidden,
            cached_key.hidden_entries_seen,
        ) == (
            key.entries_revision,
            key.show_hidden,
            key.hidden_entries_seen,
        );
        if same_listing
            && (totals.visible_files_bytes.is_some()
                || cached_key.entry_metadata_revision == key.entry_metadata_revision)
        {
            return totals;
        }
    }
    let mut visible_files_bytes = Some(0u64);
    let mut hidden_listed = 0;
    for entry in entries {
        hidden_listed += usize::from(entry.is_hidden());
        if entry.kind() != FileKind::Directory {
            visible_files_bytes = visible_files_bytes
                .zip(entry.size_if_resolved())
                .map(|(total, size)| total.saturating_add(size));
        }
    }
    let totals = DirectoryFileTotals {
        visible_files_bytes,
        // 不显示隐藏文件时它们不在条目里，用扫描时见到的数。
        hidden_entries: if key.show_hidden {
            hidden_listed
        } else {
            key.hidden_entries_seen
        },
    };
    *cached = Some((key, totals));
    totals
}

/// 统计选中的条目；大小是条目此刻的文件信息（还没补上的算 0）。
pub(crate) fn summarize_entries<'a>(
    selected_entries: impl Iterator<Item = &'a DirectoryEntry>,
) -> PaneSelectionSummary {
    let mut summary = PaneSelectionSummary::default();
    for entry in selected_entries {
        match entry.kind() {
            FileKind::Directory => summary.directory_count += 1,
            FileKind::File | FileKind::Symlink | FileKind::Other => {
                summary.file_count += 1;
                summary.file_total_bytes += entry.size();
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use file_core::EntryMetadata;

    fn entry(path: &str, kind: FileKind, len: u64) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from(path),
            kind,
            EntryMetadata {
                len,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    #[test]
    fn counts_directories_separately_and_sums_only_file_sizes() {
        let selected = [
            entry("/dir/a", FileKind::Directory, 4096),
            entry("/dir/b.txt", FileKind::File, 100),
            entry("/dir/d", FileKind::Directory, 4096),
        ];

        let summary = summarize_entries(selected.iter());

        assert_eq!(summary.directory_count, 2);
        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.file_total_bytes, 100);
    }

    #[test]
    fn empty_selection_yields_empty_summary() {
        let summary = summarize_entries(std::iter::empty());

        assert!(summary.is_empty());
    }
}
