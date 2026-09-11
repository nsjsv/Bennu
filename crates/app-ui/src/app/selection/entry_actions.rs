//! 复制路径与创建符号链接:两个作用于当前选中项的工具动作。

use std::path::{Path, PathBuf};

use desktop_linux::write_desktop_clipboard_text;
use iced::Task;

use super::super::FileBrowser;
use crate::model::{
    entry_exists, unique_symlink_directory_name, unique_symlink_file_name, Message,
};
use crate::operation_queue::{QueuedFileOperation, SymbolicLinkCreation};

impl FileBrowser {
    /// 「复制路径」:把选中路径按可见顺序拼成多行纯文本写入桌面剪贴板,
    /// 无 file:// 前缀,终端直接可粘。
    pub(in crate::app) fn copy_selected_path_text(&mut self) -> Task<Message> {
        self.context_menu = None;
        let paths = self.active_file_selection();
        if paths.is_empty() {
            return Task::none();
        }
        let text = copied_paths_text(&paths);
        Task::perform(write_desktop_clipboard_text(text), |write_outcome| {
            Message::PathTextCopied(write_outcome.map_err(|error| error.to_string()))
        })
    }

    /// 「创建符号链接」:链接建在各自源父目录,名字按共享命名规则起名,
    /// 目标一律绝对路径;gvfs 等远程挂载路径由菜单 gating 拦下。
    pub(in crate::app) fn create_symlinks_for_selection(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.is_trash_view {
            return Task::none();
        }
        let sources = self.active_file_selection();
        if sources.is_empty() {
            return Task::none();
        }
        let links = sources
            .iter()
            .map(|source| {
                let parent = self.entry_parent_directory(source);
                self.symbolic_link_creation_for(source, &parent)
            })
            .collect::<Vec<_>>();
        self.enqueue_file_operation(QueuedFileOperation::CreateSymbolicLinks { links })
    }

    /// 单个源的符号链接构造:名字按共享唯一命名规则在 directory 里起名,
    /// 目标一律绝对路径。右键菜单(建在源父目录)与 Alt 拖放(建在落点)
    /// 共用此不变量,重名自动递增。
    pub(in crate::app) fn symbolic_link_creation_for(
        &self,
        source: &Path,
        directory: &Path,
    ) -> SymbolicLinkCreation {
        let name = source
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("item"));
        let link_name = if self.entry_kind(source) == Some(file_core::FileKind::Directory) {
            unique_symlink_directory_name(name, |candidate| {
                entry_exists(&directory.join(candidate))
            })
        } else {
            unique_symlink_file_name(name, |candidate| {
                entry_exists(&directory.join(candidate))
            })
        };
        SymbolicLinkCreation {
            link_path: directory.join(link_name),
            target_path: source.to_path_buf(),
        }
    }
}

/// 拼接规则独立成纯函数,便于锁定「可见顺序 + 多行 + 无前缀」的文本格式。
fn copied_paths_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_paths_join_in_visible_order_without_uri_prefix() {
        let paths = vec![
            PathBuf::from("/tmp/a b.txt"),
            PathBuf::from("/tmp/第二"),
        ];

        let text = copied_paths_text(&paths);

        assert_eq!(text, "/tmp/a b.txt\n/tmp/第二");
        assert!(!text.contains("file://"));
    }

    #[test]
    fn single_path_copies_without_trailing_newline() {
        let text = copied_paths_text(&[PathBuf::from("/tmp/report.pdf")]);

        assert_eq!(text, "/tmp/report.pdf");
    }
}
