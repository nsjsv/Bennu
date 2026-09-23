//! 预览树交互已迁 bennu-preview 的 PreviewEngine（engine/tree.rs，impl
//! 块与纯函数逐字节保真）；此处保留同名薄转发并注入宿主扫描选项
//! （目录会话状态非预览域所有）。preview_tree_animation_is_active 经
//! FileBrowser 的 Deref 垫片直接命中引擎同名方法。

use std::path::PathBuf;

use file_core::DirectoryEntry;
use iced::Task;

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    pub(in crate::app) fn toggle_preview_tree_directory(
        &mut self,
        entry_id: usize,
    ) -> Task<Message> {
        self.preview_engine
            .toggle_preview_tree_directory(entry_id, self.options.clone())
            .map(Message::Preview)
    }

    pub(in crate::app) fn accept_preview_directory_children(
        &mut self,
        parent_path: PathBuf,
        children_outcome: Result<Vec<DirectoryEntry>, String>,
    ) -> Task<Message> {
        let options = self.options.clone();
        let expand_levels = self.preview_directory_expand_levels();
        self.preview_engine
            .accept_preview_directory_children(
                parent_path,
                children_outcome,
                options,
                expand_levels,
            )
            .map(Message::Preview)
    }

    pub(in crate::app) fn advance_preview_tree_animation(&mut self) -> Task<Message> {
        self.preview_engine
            .advance_preview_tree_animation()
            .map(Message::Preview)
    }
}
