use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;

use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};
use crate::virtual_range::VirtualRange;

#[derive(Debug, Clone, Copy)]
pub(crate) struct VisibleEntry<'a> {
    pub(crate) entry: &'a DirectoryEntry,
    pub(crate) depth: usize,
    pub(crate) animation_progress: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisibleEntryStatusRow {
    Error,
    Empty,
}

pub(crate) fn visible_entry_status_row(
    expanded: &ExpandedDirectory,
) -> Option<VisibleEntryStatusRow> {
    if !expanded.is_expanded && !expanded.is_collapsing {
        return None;
    }
    match &expanded.status {
        ExpandedDirectoryStatus::Loading => None,
        ExpandedDirectoryStatus::Error => Some(VisibleEntryStatusRow::Error),
        ExpandedDirectoryStatus::Loaded if expanded.entries.is_empty() => {
            Some(VisibleEntryStatusRow::Empty)
        }
        ExpandedDirectoryStatus::Loaded => None,
    }
}

pub(crate) fn visible_entry_status_row_height(
    expanded: &ExpandedDirectory,
    row_height: f32,
) -> f32 {
    if visible_entry_status_row(expanded).is_some() {
        row_height * expanded.animation_progress.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub(crate) fn list_entry_range_for_viewport(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    row_height: f32,
    header_height: f32,
    viewport_offset: f32,
    viewport_height: f32,
    overscan_rows: usize,
) -> VirtualRange {
    if row_height <= f32::EPSILON || viewport_height <= f32::EPSILON {
        return VirtualRange::empty();
    }
    let viewport_top = viewport_offset.max(0.0);
    let overscan_height = overscan_rows as f32 * row_height;
    let limit = ListEntryRangeLimit::Viewport {
        top: (viewport_top - header_height - overscan_height).max(0.0),
        bottom: (viewport_top + viewport_height - header_height).max(0.0) + overscan_height,
    };
    if !has_visible_list_expansion(entries, expanded_directories) {
        return flat_list_entry_height_range(entries.len(), row_height, limit);
    }
    list_entry_height_range(entries, expanded_directories, row_height, limit)
}

pub(crate) fn initial_list_entry_range(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    row_height: f32,
    initial_rows: usize,
) -> VirtualRange {
    if row_height <= f32::EPSILON || initial_rows == 0 {
        return VirtualRange::empty();
    }
    let limit = ListEntryRangeLimit::InitialRows(initial_rows);
    if !has_visible_list_expansion(entries, expanded_directories) {
        return flat_list_entry_height_range(entries.len(), row_height, limit);
    }
    list_entry_height_range(entries, expanded_directories, row_height, limit)
}

fn has_visible_list_expansion(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    let Some(root_parent) = entries.first().and_then(|entry| entry.path.parent()) else {
        return false;
    };

    expanded_directories.iter().any(|(path, expanded)| {
        (expanded.is_expanded || expanded.is_collapsing)
            && list_expansion_path_is_visible(path, root_parent, expanded_directories)
    })
}

fn list_expansion_path_is_visible(
    path: &Path,
    root_parent: &Path,
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    // 折叠祖先留下的缓存节点不可见；沿父路径判断可见性，避免平面目录扫描全部条目。
    let mut parent = path.parent();
    while let Some(candidate) = parent {
        if candidate == root_parent {
            return true;
        }
        let Some(ancestor) = expanded_directories.get(candidate) else {
            return false;
        };
        if !ancestor.is_expanded && !ancestor.is_collapsing {
            return false;
        }
        if !matches!(ancestor.status, ExpandedDirectoryStatus::Loaded) {
            return false;
        }
        parent = candidate.parent();
    }
    false
}

fn flat_list_entry_height_range(
    entry_count: usize,
    row_height: f32,
    limit: ListEntryRangeLimit,
) -> VirtualRange {
    match limit {
        ListEntryRangeLimit::InitialRows(rows) => {
            let end = rows.min(entry_count);
            VirtualRange {
                start: 0,
                end,
                before_height: 0.0,
                after_height: entry_count.saturating_sub(end) as f32 * row_height,
            }
        }
        ListEntryRangeLimit::Viewport { top, bottom } => {
            let start = ((top / row_height).floor().max(0.0) as usize).min(entry_count);
            let end = ((bottom / row_height).ceil().max(0.0) as usize).min(entry_count);
            VirtualRange {
                start,
                end: end.max(start),
                before_height: start as f32 * row_height,
                after_height: entry_count.saturating_sub(end.max(start)) as f32 * row_height,
            }
        }
    }
}

/// 列表行区总高(不含表头):与虚拟范围渲染的累计高度同口径(含展开
/// 状态行),供视口越界自愈判定最大滚动偏移。
pub(crate) fn list_rows_content_height(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    row_height: f32,
) -> f32 {
    let mut total_height = 0.0;
    for_each_visible_entry(entries, expanded_directories, &mut |visible_entry| {
        total_height += row_height * visible_entry.animation_progress.clamp(0.0, 1.0);
        if let Some(expanded) = expanded_directories.get(&visible_entry.entry.path) {
            total_height += visible_entry_status_row_height(expanded, row_height);
        }
    });
    total_height
}

pub(crate) fn list_entry_vertical_bounds(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    path: &Path,
    row_height: f32,
    header_height: f32,
) -> Option<(f32, f32)> {
    if !has_visible_list_expansion(entries, expanded_directories) {
        return entries
            .iter()
            .position(|entry| entry.path == path)
            .map(|index| (header_height + index as f32 * row_height, row_height));
    }
    let mut offset = header_height;
    let mut target = None;
    for_each_visible_entry(entries, expanded_directories, &mut |visible_entry| {
        let item_height = row_height * visible_entry.animation_progress.clamp(0.0, 1.0);
        if target.is_none() && visible_entry.entry.path == path {
            target = Some((offset, item_height));
        }
        offset += item_height;
        if let Some(expanded) = expanded_directories.get(&visible_entry.entry.path) {
            offset += visible_entry_status_row_height(expanded, row_height);
        }
    });
    target
}

#[derive(Debug, Clone, Copy)]
enum ListEntryRangeLimit {
    Viewport { top: f32, bottom: f32 },
    InitialRows(usize),
}

fn list_entry_height_range(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    row_height: f32,
    limit: ListEntryRangeLimit,
) -> VirtualRange {
    let mut entry_count = 0;
    let mut total_height = 0.0;
    let mut start = None;
    let mut end = 0;
    let mut before_height = 0.0;
    let mut rendered_end_height = 0.0;

    for_each_visible_entry(entries, expanded_directories, &mut |visible_entry| {
        let index = entry_count;
        entry_count += 1;
        let block_top = total_height;
        total_height += row_height * visible_entry.animation_progress.clamp(0.0, 1.0);
        if let Some(expanded) = expanded_directories.get(&visible_entry.entry.path) {
            total_height += visible_entry_status_row_height(expanded, row_height);
        }

        let include = match limit {
            ListEntryRangeLimit::Viewport { top, bottom } => {
                total_height > top && block_top < bottom
            }
            ListEntryRangeLimit::InitialRows(rows) => index < rows,
        };
        if include {
            if start.is_none() {
                start = Some(index);
                before_height = block_top;
            }
            end = index + 1;
            rendered_end_height = total_height;
        }
    });

    let start = start.unwrap_or(entry_count);
    if start == entry_count {
        before_height = total_height;
        rendered_end_height = total_height;
        end = entry_count;
    }
    VirtualRange {
        start,
        end,
        before_height,
        after_height: (total_height - rendered_end_height).max(0.0),
    }
}

fn for_each_visible_entry<'a>(
    entries: &'a [DirectoryEntry],
    expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
    visit: &mut impl FnMut(VisibleEntry<'a>),
) {
    for entry in entries {
        visit_visible_entry(entry, 0, 1.0, expanded_directories, visit);
    }
}

fn visit_visible_entry<'a>(
    entry: &'a DirectoryEntry,
    depth: usize,
    animation_progress: f32,
    expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
    visit: &mut impl FnMut(VisibleEntry<'a>),
) {
    visit(VisibleEntry {
        entry,
        depth,
        animation_progress,
    });
    let Some(expanded) = expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return;
    };
    if matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
        let child_progress = animation_progress * expanded.animation_progress.clamp(0.0, 1.0);
        for child in &expanded.entries {
            visit_visible_entry(
                child,
                depth + 1,
                child_progress,
                expanded_directories,
                visit,
            );
        }
    }
}

#[cfg(test)]
pub(crate) fn visible_entries<'a>(
    entries: &'a [DirectoryEntry],
    expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
) -> Vec<VisibleEntry<'a>> {
    visible_entries_in_range(entries, expanded_directories, 0, usize::MAX)
}

pub(crate) fn visible_entries_in_range<'a>(
    entries: &'a [DirectoryEntry],
    expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
    start: usize,
    end: usize,
) -> Vec<VisibleEntry<'a>> {
    if !has_visible_list_expansion(entries, expanded_directories) {
        let end = end.min(entries.len());
        let start = start.min(end);
        return entries
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|entry| VisibleEntry {
                entry,
                depth: 0,
                animation_progress: 1.0,
            })
            .collect();
    }

    if start >= end {
        return Vec::new();
    }

    let mut cursor = VisibleEntryRangeCursor {
        next_index: 0,
        start,
        end,
        rows: Vec::new(),
    };
    for entry in entries {
        push_visible_entry_in_range(entry, 0, 1.0, expanded_directories, &mut cursor);
        if cursor.next_index >= end {
            break;
        }
    }
    cursor.rows
}

pub(crate) fn visible_entry_paths(
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in entries {
        push_selectable_entry_path(entry, expanded_directories, &mut paths);
    }
    paths
}

pub(crate) fn visible_child_paths(
    directory: &Path,
    current_dir: &Path,
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> Vec<PathBuf> {
    if directory == current_dir {
        return entries.iter().map(|entry| entry.path.clone()).collect();
    }

    expanded_directories
        .get(directory)
        .filter(|expanded| expanded.is_expanded)
        .filter(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loaded))
        .map(|expanded| {
            expanded
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn entry_is_visible(
    path: &Path,
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    entries
        .iter()
        .any(|entry| visible_subtree_contains(path, entry, expanded_directories))
}

struct VisibleEntryRangeCursor<'a> {
    next_index: usize,
    start: usize,
    end: usize,
    rows: Vec<VisibleEntry<'a>>,
}

fn push_visible_entry_in_range<'a>(
    entry: &'a DirectoryEntry,
    depth: usize,
    animation_progress: f32,
    expanded_directories: &'a HashMap<PathBuf, ExpandedDirectory>,
    cursor: &mut VisibleEntryRangeCursor<'a>,
) {
    if cursor.next_index >= cursor.start && cursor.next_index < cursor.end {
        cursor.rows.push(VisibleEntry {
            entry,
            depth,
            animation_progress,
        });
    }
    cursor.next_index = cursor.next_index.saturating_add(1);
    if cursor.next_index >= cursor.end {
        return;
    }

    let Some(expanded) = expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return;
    };
    if matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
        let child_progress = animation_progress * expanded.animation_progress.clamp(0.0, 1.0);
        for child in &expanded.entries {
            push_visible_entry_in_range(
                child,
                depth + 1,
                child_progress,
                expanded_directories,
                cursor,
            );
            if cursor.next_index >= cursor.end {
                break;
            }
        }
    }
}

fn visible_subtree_contains(
    path: &Path,
    entry: &DirectoryEntry,
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    if entry.path == path {
        return true;
    }
    let Some(expanded) = expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return false;
    };
    matches!(expanded.status, ExpandedDirectoryStatus::Loaded)
        && expanded
            .entries
            .iter()
            .any(|child| visible_subtree_contains(path, child, expanded_directories))
}

fn push_selectable_entry_path(
    entry: &DirectoryEntry,
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
    paths: &mut Vec<PathBuf>,
) {
    paths.push(entry.path.clone());
    let Some(expanded) = expanded_directories
        .get(&entry.path)
        .filter(|expanded| expanded.is_expanded && !expanded.is_collapsing)
    else {
        return;
    };
    if matches!(expanded.status, ExpandedDirectoryStatus::Loaded) {
        for child in &expanded.entries {
            push_selectable_entry_path(child, expanded_directories, paths);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::*;

    fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
        DirectoryEntry::new(
            path,
            kind,
            EntryMetadata {
                len: 0,
                modified: None,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }
    #[test]
    fn flat_list_fast_paths_preserve_range_slice_and_bounds() {
        let root = PathBuf::from("/workspace");
        let entries = (0..5)
            .map(|index| test_entry(root.join(format!("item-{index}")), FileKind::File))
            .collect::<Vec<_>>();
        let expanded_directories = HashMap::new();
        let range = list_entry_range_for_viewport(
            &entries,
            &expanded_directories,
            10.0,
            5.0,
            30.0,
            20.0,
            0,
        );
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 5);
        assert_eq!(range.before_height, 20.0);
        assert_eq!(range.after_height, 0.0);
        let visible =
            visible_entries_in_range(&entries, &expanded_directories, range.start, range.end);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].entry.path, entries[2].path);
        assert_eq!(visible[2].entry.path, entries[4].path);
        assert_eq!(
            list_entry_vertical_bounds(
                &entries,
                &expanded_directories,
                &entries[3].path,
                10.0,
                5.0,
            ),
            Some((35.0, 10.0))
        );
    }

    #[test]
    fn active_loaded_expansion_uses_recursive_rows() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("directory"), FileKind::Directory);
        let child = test_entry(directory.path.join("child.txt"), FileKind::File);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![directory.clone(), sibling];
        let expanded_directories =
            HashMap::from([(directory.path.clone(), expanded(vec![child.clone()], true))]);

        let visible = visible_entries_in_range(&entries, &expanded_directories, 0, 3);

        assert_eq!(
            visible
                .iter()
                .map(|entry| entry.entry.path.clone())
                .collect::<Vec<_>>(),
            vec![
                directory.path,
                child.path,
                PathBuf::from("/workspace/sibling.txt")
            ]
        );
    }

    #[test]
    fn collapsed_parent_hides_active_descendant_from_list_range() {
        let root = PathBuf::from("/workspace");
        let parent = test_entry(root.join("parent"), FileKind::Directory);
        let child = test_entry(parent.path.join("child"), FileKind::Directory);
        let nested = test_entry(child.path.join("nested.txt"), FileKind::File);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![parent.clone(), sibling.clone()];
        let expanded_directories = HashMap::from([
            (parent.path.clone(), expanded(vec![child.clone()], false)),
            (child.path.clone(), expanded(vec![nested], true)),
        ]);

        let visible = visible_entries_in_range(&entries, &expanded_directories, 0, 2);

        assert_eq!(
            visible
                .iter()
                .map(|entry| entry.entry.path.clone())
                .collect::<Vec<_>>(),
            vec![parent.path, sibling.path]
        );
    }

    fn expanded(entries: Vec<DirectoryEntry>, is_expanded: bool) -> ExpandedDirectory {
        ExpandedDirectory {
            entries,
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        }
    }

    #[test]
    fn visible_entries_include_root_rows() {
        let root = PathBuf::from("/workspace");
        let first = test_entry(root.join("a.txt"), FileKind::File);
        let second = test_entry(root.join("b.txt"), FileKind::File);
        let entries = vec![first.clone(), second.clone()];

        let paths = visible_entry_paths(&entries, &HashMap::new());

        assert_eq!(paths, vec![first.path, second.path]);
    }

    #[test]
    fn list_layout_range_counts_status_and_animation_heights() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("directory"), FileKind::Directory);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![directory.clone(), sibling.clone()];
        let expanded_directories =
            HashMap::from([(directory.path.clone(), expanded(Vec::new(), true))]);

        let range = list_entry_range_for_viewport(
            &entries,
            &expanded_directories,
            46.0,
            32.0,
            124.0,
            46.0,
            0,
        );
        assert_eq!(range.start, 1);
        assert_eq!(range.end, 2);
        assert_eq!(range.before_height, 92.0);
        assert_eq!(range.after_height, 0.0);

        let mut failed = expanded(Vec::new(), true);
        failed.status = ExpandedDirectoryStatus::Error;
        assert_eq!(
            visible_entry_status_row(&failed),
            Some(VisibleEntryStatusRow::Error)
        );
        assert_eq!(visible_entry_status_row_height(&failed, 46.0), 46.0);

        assert_eq!(
            initial_list_entry_range(&entries, &expanded_directories, 46.0, 1),
            VirtualRange {
                start: 0,
                end: 1,
                before_height: 0.0,
                after_height: 46.0,
            }
        );

        let child = test_entry(directory.path.join("child.txt"), FileKind::File);
        let mut animated = expanded(vec![child], true);
        animated.animation_progress = 0.5;
        let expanded_directories = HashMap::from([(directory.path.clone(), animated)]);
        let range = list_entry_range_for_viewport(
            &entries,
            &expanded_directories,
            46.0,
            32.0,
            101.0,
            46.0,
            0,
        );
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 3);
        assert_eq!(range.before_height, 69.0);
        assert_eq!(
            list_entry_vertical_bounds(&entries, &expanded_directories, &sibling.path, 46.0, 32.0,),
            Some((101.0, 46.0))
        );
    }

    #[test]
    fn visible_entries_include_expanded_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories =
            HashMap::from([(directory.path.clone(), expanded(vec![child.clone()], true))]);

        let visible = visible_entries(&entries, &expanded_directories);

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].entry.path, directory.path);
        assert_eq!(visible[0].depth, 0);
        assert_eq!(visible[1].entry.path, child.path);
        assert_eq!(visible[1].depth, 1);
    }

    #[test]
    fn visible_entries_include_nested_expanded_children() {
        let root = PathBuf::from("/workspace");
        let parent = test_entry(root.join("parent"), FileKind::Directory);
        let child = test_entry(parent.path.join("child"), FileKind::Directory);
        let grandchild = test_entry(child.path.join("note.txt"), FileKind::File);
        let entries = vec![parent.clone()];
        let expanded_directories = HashMap::from([
            (parent.path.clone(), expanded(vec![child.clone()], true)),
            (child.path.clone(), expanded(vec![grandchild.clone()], true)),
        ]);

        let visible = visible_entries(&entries, &expanded_directories);

        assert_eq!(visible.len(), 3);
        assert_eq!(visible[2].entry.path, grandchild.path);
        assert_eq!(visible[2].depth, 2);
    }

    #[test]
    fn collapsed_directory_hides_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories =
            HashMap::from([(directory.path.clone(), expanded(vec![child], false))]);

        let paths = visible_entry_paths(&entries, &expanded_directories);

        assert_eq!(paths, vec![directory.path]);
    }

    #[test]
    fn collapsed_directory_hides_children_even_when_animation_progress_remains() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path.clone(),
            ExpandedDirectory {
                entries: vec![child],
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Loaded,
                is_expanded: false,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                    field: file_core::SortField::Name,
                    direction: file_core::SortDirection::Ascending,
                },
            },
        )]);

        let visible = visible_entries(&entries, &expanded_directories);

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].entry.path, directory.path);
        assert_eq!(visible[0].depth, 0);
    }

    #[test]
    fn loading_directory_does_not_show_stale_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path.clone(),
            ExpandedDirectory {
                entries: vec![child],
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Loading,
                is_expanded: true,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                    field: file_core::SortField::Name,
                    direction: file_core::SortDirection::Ascending,
                },
            },
        )]);

        let paths = visible_entry_paths(&entries, &expanded_directories);

        assert_eq!(paths, vec![directory.path]);
    }

    #[test]
    fn loading_directory_does_not_expose_stale_visible_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path.clone(),
            ExpandedDirectory {
                entries: vec![child],
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Loading,
                is_expanded: true,
                is_collapsing: false,
                animation_progress: 1.0,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                    field: file_core::SortField::Name,
                    direction: file_core::SortDirection::Ascending,
                },
            },
        )]);

        let paths = visible_child_paths(&directory.path, &root, &entries, &expanded_directories);

        assert!(paths.is_empty());
    }

    #[test]
    fn collapsing_directory_keeps_children_for_animation_but_not_selection_paths() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path.join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path.clone(),
            ExpandedDirectory {
                entries: vec![child.clone()],
                directory_discovery: None,
                status: ExpandedDirectoryStatus::Loaded,
                is_expanded: true,
                is_collapsing: true,
                animation_progress: 0.5,
                load_generation: 0,
                load_context: None,
                load_cancel: None,
                directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                    field: file_core::SortField::Name,
                    direction: file_core::SortDirection::Ascending,
                },
            },
        )]);

        let visible = visible_entries(&entries, &expanded_directories);
        let selectable_paths = visible_entry_paths(&entries, &expanded_directories);

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[1].entry.path, child.path);
        assert_eq!(visible[1].animation_progress, 0.5);
        assert_eq!(selectable_paths, vec![directory.path]);
    }
}
