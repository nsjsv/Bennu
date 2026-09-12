use std::collections::HashSet;
use std::path::PathBuf;

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

    pub(crate) fn merge(&mut self, other: Self) {
        self.directory_count += other.directory_count;
        self.file_count += other.file_count;
        self.file_total_bytes += other.file_total_bytes;
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

/// 聚合候选条目中被选中的部分。候选集必须与窗格可见条目同源,
/// 已失效的选中路径(条目离开窗格)自然落空,不进入统计。
/// 大小经由 display_len 取 UI 权威值(基础扫描阶段 len 尚未补全,
/// 列表大小列用的 discovery 元数据才是真实大小)。
pub(crate) fn summarize_selected_entries<'a>(
    candidates: impl Iterator<Item = &'a DirectoryEntry>,
    selected_paths: &HashSet<PathBuf>,
    display_len: impl Fn(&DirectoryEntry) -> u64,
) -> PaneSelectionSummary {
    let mut summary = PaneSelectionSummary::default();
    for entry in candidates {
        if !selected_paths.contains(&entry.path) {
            continue;
        }
        match entry.kind {
            FileKind::Directory => summary.directory_count += 1,
            FileKind::File | FileKind::Symlink | FileKind::Other => {
                summary.file_count += 1;
                summary.file_total_bytes += display_len(entry);
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;
    use std::path::PathBuf;

    use file_core::EntryMetadata;

    fn entry(path: &str, kind: FileKind, len: u64) -> DirectoryEntry {
        DirectoryEntry {
            path: PathBuf::from(path),
            name: OsString::from(path),
            kind,
            metadata: EntryMetadata {
                len,
                ..EntryMetadata::default()
            },
            is_hidden: false,
            is_symlink: false,
            is_broken_symlink: false,
            discovery_index: None,
        }
    }

    #[test]
    fn counts_directories_separately_and_sums_only_file_sizes() {
        let candidates = [
            entry("/dir/a", FileKind::Directory, 4096),
            entry("/dir/b.txt", FileKind::File, 100),
            entry("/dir/c.txt", FileKind::File, 23),
            entry("/dir/d", FileKind::Directory, 4096),
        ];
        let mut selected = HashSet::new();
        selected.insert(PathBuf::from("/dir/a"));
        selected.insert(PathBuf::from("/dir/b.txt"));
        selected.insert(PathBuf::from("/dir/d"));

        let summary = summarize_selected_entries(candidates.iter(), &selected, |entry| entry.metadata.len);

        assert_eq!(summary.directory_count, 2);
        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.file_total_bytes, 100);
    }

    #[test]
    fn selected_paths_missing_from_candidates_do_not_count() {
        let candidates = [entry("/dir/a.txt", FileKind::File, 100)];
        let mut selected = HashSet::new();
        selected.insert(PathBuf::from("/dir/a.txt"));
        selected.insert(PathBuf::from("/dir/gone.txt"));

        let summary = summarize_selected_entries(candidates.iter(), &selected, |entry| entry.metadata.len);

        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.file_total_bytes, 100);
    }

    #[test]
    fn sizes_come_from_display_len_provider_not_raw_metadata() {
        // 基础扫描阶段 metadata.len 为 0,权威大小由 discovery 元数据提供。
        let candidates = [entry("/dir/a.mp4", FileKind::File, 0)];
        let mut selected = HashSet::new();
        selected.insert(PathBuf::from("/dir/a.mp4"));

        let summary =
            summarize_selected_entries(candidates.iter(), &selected, |entry| entry.metadata.len);
        assert_eq!(summary.file_total_bytes, 0);

        let summary = summarize_selected_entries(candidates.iter(), &selected, |_| 7_340_032);
        assert_eq!(summary.file_total_bytes, 7_340_032);
    }

    #[test]
    fn empty_selection_yields_empty_summary() {
        let candidates = [entry("/dir/a.txt", FileKind::File, 100)];
        let selected = HashSet::new();

        let summary = summarize_selected_entries(candidates.iter(), &selected, |entry| entry.metadata.len);

        assert!(summary.is_empty());
    }
}
