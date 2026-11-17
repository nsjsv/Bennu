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
