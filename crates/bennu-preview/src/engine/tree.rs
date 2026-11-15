//! 树形预览的公共部分：箭头转动动画（文件夹、压缩包都有）、压缩包的展开收起与滚动
//! 位置。文件夹预览自己的状态机在 `engine/folder.rs`。

use iced::Task;

use crate::preview::{PreviewContent, PreviewState, PreviewTreeEntry, TreeViewport};
use crate::preview_message::PreviewMessage;

const PREVIEW_TREE_TOGGLE_ROTATION_STEP: f32 = 0.18;
const PREVIEW_TREE_TOGGLE_ROTATION_EPSILON: f32 = 0.001;

impl super::PreviewEngine {
    /// 压缩包里的文件夹（内容一开始就全列出来了，只是收起/展开）。
    pub fn toggle_archive_tree_directory(&mut self, entry_id: usize) -> Task<PreviewMessage> {
        if let Some(PreviewState::Ready(PreviewContent::Archive { entries, .. })) =
            &mut self.preview
        {
            if let Some(entry) = entries
                .get_mut(entry_id)
                .filter(|entry| entry.is_directory())
            {
                entry.is_expanded = !entry.is_expanded;
            }
        }
        Task::none()
    }

    pub fn scroll_preview_tree(&mut self, scrolled: TreeViewport) {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::Archive { viewport, .. })) => {
                *viewport = scrolled;
            }
            Some(PreviewState::Ready(PreviewContent::Folder(folder))) => folder.viewport = scrolled,
            _ => {}
        }
    }

    pub fn preview_tree_animation_is_active(&self) -> bool {
        match &self.preview {
            Some(PreviewState::Ready(PreviewContent::Archive { entries, .. })) => {
                entries.iter().any(archive_rotation_is_active)
            }
            Some(PreviewState::Ready(PreviewContent::Folder(folder))) => {
                folder.rotation_is_active()
            }
            _ => false,
        }
    }

    pub fn advance_preview_tree_animation(&mut self) -> Task<PreviewMessage> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::Archive { entries, .. })) => {
                for entry in entries.iter_mut().filter(|entry| entry.is_directory()) {
                    let target = archive_rotation_target(entry);
                    entry.toggle_rotation_progress = if entry.toggle_rotation_progress < target {
                        (entry.toggle_rotation_progress + PREVIEW_TREE_TOGGLE_ROTATION_STEP)
                            .min(target)
                    } else {
                        (entry.toggle_rotation_progress - PREVIEW_TREE_TOGGLE_ROTATION_STEP)
                            .max(target)
                    };
                }
            }
            Some(PreviewState::Ready(PreviewContent::Folder(folder))) => {
                folder.advance_rotation(PREVIEW_TREE_TOGGLE_ROTATION_STEP);
            }
            _ => {}
        }
        Task::none()
    }
}

fn archive_rotation_is_active(entry: &PreviewTreeEntry) -> bool {
    entry.is_directory()
        && (entry.toggle_rotation_progress - archive_rotation_target(entry)).abs()
            > PREVIEW_TREE_TOGGLE_ROTATION_EPSILON
}

fn archive_rotation_target(entry: &PreviewTreeEntry) -> f32 {
    if entry.is_expanded {
        1.0
    } else {
        0.0
    }
}
