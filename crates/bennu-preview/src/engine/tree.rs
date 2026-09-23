//! 预览树（目录/归档展开树）状态处理：自 app-ui 的
//! preview_state/tree.rs 迁入（impl FileBrowser → impl PreviewEngine，
//! 逐字节保真；子目录加载命令直连本 crate 命令层）。

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use file_core::{DirectoryEntry, ScanOptions};
use iced::Task;

use crate::commands::preview::preview_directory_children_command;
use crate::preview::{
    PreviewContent, PreviewState, PreviewTreeDirectoryChildren, PreviewTreeEntry,
};
use crate::preview_message::PreviewMessage;

const PREVIEW_TREE_TOGGLE_ROTATION_STEP: f32 = 0.18;
const PREVIEW_TREE_TOGGLE_ROTATION_EPSILON: f32 = 0.001;

impl super::PreviewEngine {
    /// 扫描选项由宿主传入（宿主目录会话状态，非预览域所有）。
    pub fn toggle_preview_tree_directory(
        &mut self,
        entry_id: usize,
        options: ScanOptions,
    ) -> Task<PreviewMessage> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::Directory { entries, .. })) => {
                toggle_directory_preview_tree_entry(entries, entry_id, options)
            }
            Some(PreviewState::Ready(PreviewContent::Archive { entries, .. })) => {
                toggle_loaded_preview_tree_entry(entries, entry_id)
            }
            _ => Task::none(),
        }
    }

    /// 扫描选项与展开层数由宿主传入：两者都是宿主会话/配置的实时值
    /// （用户改设置不必落盘即可生效的路径存在，如测试工厂直改
    /// user_config），引擎配置快照只服务后续批次迁入的引擎内部消费者。
    pub fn accept_preview_directory_children(
        &mut self,
        parent_path: PathBuf,
        children_outcome: Result<Vec<DirectoryEntry>, String>,
        options: ScanOptions,
        expand_levels: u8,
    ) -> Task<PreviewMessage> {
        let Some(entries) = directory_preview_entries_mut(self.preview.as_mut()) else {
            return Task::none();
        };
        let inserted_range = match children_outcome {
            Ok(children) => {
                accept_loaded_preview_directory_children(entries, &parent_path, children)
            }
            Err(error) => {
                accept_preview_directory_children_error(entries, &parent_path, error);
                None
            }
        };
        match inserted_range {
            Some(range) => {
                auto_expand_preview_tree_directories(entries, range, expand_levels, &options)
            }
            None => Task::none(),
        }
    }

    pub fn preview_tree_animation_is_active(&self) -> bool {
        let Some(entries) = preview_tree_entries(self.preview.as_ref()) else {
            return false;
        };
        entries.iter().any(preview_tree_rotation_is_active)
    }

    pub fn advance_preview_tree_animation(&mut self) -> Task<PreviewMessage> {
        let Some(entries) = preview_tree_entries_mut(self.preview.as_mut()) else {
            return Task::none();
        };

        for entry in entries.iter_mut().filter(|entry| entry.is_directory()) {
            let target = preview_tree_rotation_target(entry);
            if entry.toggle_rotation_progress < target {
                entry.toggle_rotation_progress = (entry.toggle_rotation_progress
                    + PREVIEW_TREE_TOGGLE_ROTATION_STEP)
                    .min(target);
            } else if entry.toggle_rotation_progress > target {
                entry.toggle_rotation_progress = (entry.toggle_rotation_progress
                    - PREVIEW_TREE_TOGGLE_ROTATION_STEP)
                    .max(target);
            }
        }

        Task::none()
    }
}

fn toggle_directory_preview_tree_entry(
    entries: &mut [PreviewTreeEntry],
    entry_id: usize,
    options: ScanOptions,
) -> Task<PreviewMessage> {
    let Some(entry) = entries.get_mut(entry_id) else {
        return Task::none();
    };
    if !entry.is_directory() {
        return Task::none();
    }

    entry.is_expanded = !entry.is_expanded;
    if !entry.is_expanded || !directory_children_can_load(entry) {
        return Task::none();
    }

    let Some(path) = entry.filesystem_path.clone() else {
        return Task::none();
    };
    entry.directory_children = Some(PreviewTreeDirectoryChildren::Loading);
    preview_directory_children_command(path, options)
}

fn toggle_loaded_preview_tree_entry(
    entries: &mut [PreviewTreeEntry],
    entry_id: usize,
) -> Task<PreviewMessage> {
    let Some(entry) = entries.get_mut(entry_id) else {
        return Task::none();
    };
    if entry.is_directory() {
        entry.is_expanded = !entry.is_expanded;
    }

    Task::none()
}

fn directory_children_can_load(entry: &PreviewTreeEntry) -> bool {
    matches!(
        entry.directory_children.as_ref(),
        Some(PreviewTreeDirectoryChildren::Pending | PreviewTreeDirectoryChildren::Error(_))
    )
}

fn directory_preview_entries_mut(
    preview: Option<&mut PreviewState>,
) -> Option<&mut Vec<PreviewTreeEntry>> {
    match preview? {
        PreviewState::Ready(PreviewContent::Directory { entries, .. }) => Some(entries),
        _ => None,
    }
}

fn accept_loaded_preview_directory_children(
    entries: &mut Vec<PreviewTreeEntry>,
    parent_path: &Path,
    children: Vec<DirectoryEntry>,
) -> Option<Range<usize>> {
    let parent_id = preview_tree_entry_index_for_path(entries, parent_path)?;
    if !matches!(
        entries[parent_id].directory_children.as_ref(),
        Some(PreviewTreeDirectoryChildren::Loading)
    ) {
        return None;
    }

    let insert_at = preview_tree_subtree_end(entries, parent_id);
    let child_count = children.len();
    shift_parent_ids_after_insertion(entries, insert_at, child_count);
    let child_depth = entries[parent_id].depth + 1;
    let child_entries = children
        .into_iter()
        .enumerate()
        .map(|(offset, entry)| {
            PreviewTreeEntry::from_directory_entry(
                insert_at + offset,
                entry,
                child_depth,
                Some(parent_id),
            )
        })
        .collect::<Vec<_>>();
    entries.splice(insert_at..insert_at, child_entries);
    entries[parent_id].directory_children = Some(PreviewTreeDirectoryChildren::Loaded);
    renumber_preview_tree_entries(entries);
    Some(insert_at..insert_at + child_count)
}

/// 把新插入的目录按配置的预览展开层数自动展开，
/// 只作用于 `range` 内的节点，不覆盖用户手动收起的其他目录。
pub fn auto_expand_preview_tree_directories(
    entries: &mut [PreviewTreeEntry],
    range: Range<usize>,
    expand_levels: u8,
    options: &ScanOptions,
) -> Task<PreviewMessage> {
    if expand_levels == 0 {
        return Task::none();
    }
    let levels = expand_levels as usize;
    let commands = entries
        .iter_mut()
        .enumerate()
        .filter(|(index, entry)| {
            range.contains(index) && entry.is_directory() && entry.depth < levels
        })
        .filter_map(|(_index, entry)| {
            if entry.is_expanded || !directory_children_can_load(entry) {
                return None;
            }
            entry.is_expanded = true;
            let path = entry.filesystem_path.clone()?;
            entry.directory_children = Some(PreviewTreeDirectoryChildren::Loading);
            Some(preview_directory_children_command(path, options.clone()))
        })
        .collect::<Vec<_>>();
    Task::batch(commands)
}

fn accept_preview_directory_children_error(
    entries: &mut [PreviewTreeEntry],
    parent_path: &Path,
    error: String,
) {
    let Some(parent_id) = preview_tree_entry_index_for_path(entries, parent_path) else {
        return;
    };
    if matches!(
        entries[parent_id].directory_children.as_ref(),
        Some(PreviewTreeDirectoryChildren::Loading)
    ) {
        entries[parent_id].directory_children = Some(PreviewTreeDirectoryChildren::Error(error));
    }
}

fn preview_tree_entry_index_for_path(entries: &[PreviewTreeEntry], path: &Path) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.filesystem_path.as_deref() == Some(path))
}

fn preview_tree_subtree_end(entries: &[PreviewTreeEntry], parent_id: usize) -> usize {
    let parent_depth = entries[parent_id].depth;
    let mut index = parent_id + 1;
    while entries
        .get(index)
        .is_some_and(|entry| entry.depth > parent_depth)
    {
        index += 1;
    }
    index
}

fn shift_parent_ids_after_insertion(
    entries: &mut [PreviewTreeEntry],
    insert_at: usize,
    child_count: usize,
) {
    if child_count == 0 {
        return;
    }

    for entry in entries.iter_mut().skip(insert_at) {
        if let Some(parent) = entry.parent.as_mut().filter(|parent| **parent >= insert_at) {
            *parent += child_count;
        }
    }
}

fn renumber_preview_tree_entries(entries: &mut [PreviewTreeEntry]) {
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.id = index;
    }
}

fn preview_tree_entries(preview: Option<&PreviewState>) -> Option<&[PreviewTreeEntry]> {
    match preview? {
        PreviewState::Ready(
            PreviewContent::Directory { entries, .. } | PreviewContent::Archive { entries, .. },
        ) => Some(entries),
        _ => None,
    }
}

fn preview_tree_entries_mut(preview: Option<&mut PreviewState>) -> Option<&mut [PreviewTreeEntry]> {
    match preview? {
        PreviewState::Ready(
            PreviewContent::Directory { entries, .. } | PreviewContent::Archive { entries, .. },
        ) => Some(entries),
        _ => None,
    }
}

fn preview_tree_rotation_is_active(entry: &PreviewTreeEntry) -> bool {
    entry.is_directory()
        && (entry.toggle_rotation_progress - preview_tree_rotation_target(entry)).abs()
            > PREVIEW_TREE_TOGGLE_ROTATION_EPSILON
}

fn preview_tree_rotation_target(entry: &PreviewTreeEntry) -> f32 {
    if entry.is_expanded {
        1.0
    } else {
        0.0
    }
}
