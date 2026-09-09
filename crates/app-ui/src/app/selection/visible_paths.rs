use std::path::{Path, PathBuf};

use crate::app::FileBrowser;
use crate::model::BrowserViewMode;

impl FileBrowser {
    pub(crate) fn entry_paths_in_directory(&self, directory: &Path) -> Vec<PathBuf> {
        if directory == self.current_dir.as_path() {
            return self
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect();
        }

        if self.view_mode == BrowserViewMode::Icons {
            return self
                .icon_grid_expansion
                .as_ref()
                .and_then(|state| state.entries_in_interactive_directory(directory))
                .map(|entries| entries.iter().map(|entry| entry.path.clone()).collect())
                .unwrap_or_default();
        }

        crate::visible_entries::visible_child_paths(
            directory,
            &self.current_dir,
            &self.entries,
            &self.expanded_directories,
        )
    }

    /// shift 范围选择只作用于点击所在的目录容器:图标网格是一个交互面板,
    /// 多栏视图一栏就是一个目录。列表视图按 Finder 惯例让范围扫过展开子行;
    /// 回收站条目保留原始父目录且永远单列,不能按父目录分容器,保持摊平。
    pub(super) fn range_selection_confined_to_directory(&self) -> bool {
        matches!(self.view_mode, BrowserViewMode::Icons | BrowserViewMode::Columns)
            && !self.is_trash_view
    }

    pub(super) fn visible_range_paths(&self, anchor: &Path, target: &Path) -> Vec<PathBuf> {
        let paths = if self.range_selection_confined_to_directory() {
            let target_directory = target.parent().unwrap_or(self.current_dir.as_path());
            if anchor.parent() != Some(target_directory) {
                return vec![target.to_path_buf()];
            }
            self.entry_paths_in_directory(target_directory)
        } else {
            self.visible_entry_paths()
        };
        let Some(anchor_index) = paths.iter().position(|path| path == anchor) else {
            return vec![target.to_path_buf()];
        };
        let Some(target_index) = paths.iter().position(|path| path == target) else {
            return vec![target.to_path_buf()];
        };

        let (start, end) = if anchor_index <= target_index {
            (anchor_index, target_index)
        } else {
            (target_index, anchor_index)
        };
        paths[start..=end].to_vec()
    }

    pub(super) fn visible_entry_paths(&self) -> Vec<PathBuf> {
        match self.view_mode {
            BrowserViewMode::Icons => self
                .pane_view(self.active_pane_id())
                .map(|pane| {
                    self.icon_grid_layout_for_pane(pane)
                        .interactive_entry_paths()
                })
                .unwrap_or_else(|| {
                    self.entries
                        .iter()
                        .map(|entry| entry.path.clone())
                        .collect()
                }),
            BrowserViewMode::Columns | BrowserViewMode::List => {
                crate::visible_entries::visible_entry_paths(
                    &self.entries,
                    &self.expanded_directories,
                )
            }
        }
    }
}
