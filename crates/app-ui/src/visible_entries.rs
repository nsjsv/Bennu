use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;

use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};

mod list_entry_heights;
pub(crate) use list_entry_heights::ListEntryHeights;

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
        ExpandedDirectoryStatus::Loaded | ExpandedDirectoryStatus::Refreshing
            if expanded.entries.is_empty() =>
        {
            Some(VisibleEntryStatusRow::Empty)
        }
        ExpandedDirectoryStatus::Loaded | ExpandedDirectoryStatus::Refreshing => None,
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
    current_directory: Option<&Path>,
    entries: &[DirectoryEntry],
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> Vec<PathBuf> {
    if current_directory == Some(directory) {
        return entries
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
    }

    expanded_directories
        .get(directory)
        .filter(|expanded| expanded.is_expanded)
        .filter(|expanded| expanded.status.has_listing())
        .map(|expanded| {
            expanded
                .entries
                .iter()
                .map(|entry| entry.path().to_path_buf())
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

fn visible_subtree_contains(
    path: &Path,
    entry: &DirectoryEntry,
    expanded_directories: &HashMap<PathBuf, ExpandedDirectory>,
) -> bool {
    if entry.path() == path {
        return true;
    }
    let Some(expanded) = expanded_directories
        .get(entry.path())
        .filter(|expanded| expanded.is_expanded || expanded.is_collapsing)
    else {
        return false;
    };
    expanded.status.has_listing()
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
    paths.push(entry.path().to_path_buf());
    let Some(expanded) = expanded_directories
        .get(entry.path())
        .filter(|expanded| expanded.is_expanded && !expanded.is_collapsing)
    else {
        return;
    };
    if expanded.status.has_listing() {
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
    use crate::app::FileBrowser;
    use crate::model::BrowserPaneId;
    use crate::virtual_range::VirtualRange;

    /// 列表画出来的条目行（路径, 深度, 展开动画进度），根层是 /workspace。
    fn rows(
        entries: Vec<DirectoryEntry>,
        expanded_directories: HashMap<PathBuf, ExpandedDirectory>,
    ) -> Vec<(PathBuf, usize, f32)> {
        let browser = browser_with(entries, expanded_directories);
        let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
        let layout = crate::transfer_placeholder_view::list_row_layout(&browser, pane, 46.0);
        layout
            .entries_in(pane, 0..layout.len())
            .map(|(_, visible)| {
                (
                    visible.entry.path().to_path_buf(),
                    visible.depth,
                    visible.animation_progress,
                )
            })
            .collect()
    }

    fn browser_with(
        entries: Vec<DirectoryEntry>,
        expanded_directories: HashMap<PathBuf, ExpandedDirectory>,
    ) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.tab_mut().location =
            crate::model::BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = entries.into();
        browser.tab_mut().expanded_directories = expanded_directories;
        browser
    }

    fn paths(rows: &[(PathBuf, usize, f32)]) -> Vec<&Path> {
        rows.iter().map(|(path, ..)| path.as_path()).collect()
    }

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
    fn flat_list_range_slices_the_visible_rows() {
        let root = PathBuf::from("/workspace");
        let entries = (0..5)
            .map(|index| test_entry(root.join(format!("item-{index}")), FileKind::File))
            .collect::<Vec<_>>();
        let browser = browser_with(entries.clone(), HashMap::new());
        let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
        let layout = crate::transfer_placeholder_view::list_row_layout(&browser, pane, 10.0);

        let range = layout.range_for_viewport(5.0, 30.0, 20.0, 0);
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 5);
        assert_eq!(range.before_height, 20.0);
        assert_eq!(range.after_height, 0.0);
        let visible = layout
            .entries_in(pane, range.start..range.end)
            .map(|(_, visible)| visible.entry.path())
            .collect::<Vec<_>>();
        assert_eq!(
            visible,
            [entries[2].path(), entries[3].path(), entries[4].path()]
        );
    }

    #[test]
    fn active_loaded_expansion_uses_recursive_rows() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("directory"), FileKind::Directory);
        let child = test_entry(directory.path().join("child.txt"), FileKind::File);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![directory.clone(), sibling.clone()];
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            expanded(vec![child.clone()], true),
        )]);

        assert_eq!(
            paths(&rows(entries, expanded_directories)),
            [directory.path(), child.path(), sibling.path()]
        );
    }

    #[test]
    fn collapsed_parent_hides_active_descendant_from_list_range() {
        let root = PathBuf::from("/workspace");
        let parent = test_entry(root.join("parent"), FileKind::Directory);
        let child = test_entry(parent.path().join("child"), FileKind::Directory);
        let nested = test_entry(child.path().join("nested.txt"), FileKind::File);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![parent.clone(), sibling.clone()];
        let expanded_directories = HashMap::from([
            (
                parent.path().to_path_buf(),
                expanded(vec![child.clone()], false),
            ),
            (child.path().to_path_buf(), expanded(vec![nested], true)),
        ]);

        assert_eq!(
            paths(&rows(entries, expanded_directories)),
            [parent.path(), sibling.path()]
        );
    }

    fn expanded(entries: Vec<DirectoryEntry>, is_expanded: bool) -> ExpandedDirectory {
        ExpandedDirectory {
            entries: entries.into(),
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

        assert_eq!(paths, vec![first.path(), second.path()]);
    }

    #[test]
    fn list_layout_range_counts_status_and_animation_heights() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("directory"), FileKind::Directory);
        let sibling = test_entry(root.join("sibling.txt"), FileKind::File);
        let entries = vec![directory.clone(), sibling.clone()];
        // 空的展开目录:目录行 0..46、「没有项目」状态行 46..92、sibling 92..138。
        let browser = browser_with(
            entries.clone(),
            HashMap::from([(directory.path().to_path_buf(), expanded(Vec::new(), true))]),
        );
        let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
        let layout = crate::transfer_placeholder_view::list_row_layout(&browser, pane, 46.0);
        assert_eq!(
            layout.range_for_viewport(32.0, 124.0, 46.0, 0),
            VirtualRange {
                start: 2,
                end: 3,
                before_height: 92.0,
                after_height: 0.0,
            }
        );
        assert_eq!(
            layout.initial_range(1),
            VirtualRange {
                start: 0,
                end: 1,
                before_height: 0.0,
                after_height: 92.0,
            }
        );

        let mut failed = expanded(Vec::new(), true);
        failed.status = ExpandedDirectoryStatus::Error;
        assert_eq!(
            visible_entry_status_row(&failed),
            Some(VisibleEntryStatusRow::Error)
        );
        assert_eq!(visible_entry_status_row_height(&failed, 46.0), 46.0);

        // 展开到一半:子行 46..69,sibling 69..115。
        let child = test_entry(directory.path().join("child.txt"), FileKind::File);
        let mut animated = expanded(vec![child], true);
        animated.animation_progress = 0.5;
        let browser = browser_with(
            entries,
            HashMap::from([(directory.path().to_path_buf(), animated)]),
        );
        let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
        let layout = crate::transfer_placeholder_view::list_row_layout(&browser, pane, 46.0);
        let range = layout.range_for_viewport(32.0, 101.0, 46.0, 0);
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 3);
        assert_eq!(range.before_height, 69.0);
    }

    #[test]
    fn visible_entries_include_expanded_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            expanded(vec![child.clone()], true),
        )]);

        assert_eq!(
            rows(vec![directory.clone()], expanded_directories),
            [
                (directory.path().to_path_buf(), 0, 1.0),
                (child.path().to_path_buf(), 1, 1.0)
            ]
        );
    }

    #[test]
    fn visible_entries_include_nested_expanded_children() {
        let root = PathBuf::from("/workspace");
        let parent = test_entry(root.join("parent"), FileKind::Directory);
        let child = test_entry(parent.path().join("child"), FileKind::Directory);
        let grandchild = test_entry(child.path().join("note.txt"), FileKind::File);
        let expanded_directories = HashMap::from([
            (
                parent.path().to_path_buf(),
                expanded(vec![child.clone()], true),
            ),
            (
                child.path().to_path_buf(),
                expanded(vec![grandchild.clone()], true),
            ),
        ]);

        let visible = rows(vec![parent], expanded_directories);

        assert_eq!(visible.len(), 3);
        assert_eq!(visible[2], (grandchild.path().to_path_buf(), 2, 1.0));
    }

    #[test]
    fn collapsed_directory_hides_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories =
            HashMap::from([(directory.path().to_path_buf(), expanded(vec![child], false))]);

        let paths = visible_entry_paths(&entries, &expanded_directories);

        assert_eq!(paths, vec![directory.path()]);
    }

    #[test]
    fn collapsed_directory_hides_children_even_when_animation_progress_remains() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            ExpandedDirectory {
                entries: vec![child].into(),
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

        assert_eq!(
            rows(entries, expanded_directories),
            [(directory.path().to_path_buf(), 0, 1.0)]
        );
    }

    #[test]
    fn loading_directory_does_not_show_stale_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            ExpandedDirectory {
                entries: vec![child].into(),
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

        assert_eq!(paths, vec![directory.path()]);
    }

    #[test]
    fn loading_directory_does_not_expose_stale_visible_children() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            ExpandedDirectory {
                entries: vec![child].into(),
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

        let paths = visible_child_paths(
            directory.path(),
            Some(&root),
            &entries,
            &expanded_directories,
        );

        assert!(paths.is_empty());
    }

    #[test]
    fn collapsing_directory_keeps_children_for_animation_but_not_selection_paths() {
        let root = PathBuf::from("/workspace");
        let directory = test_entry(root.join("project"), FileKind::Directory);
        let child = test_entry(directory.path().join("main.rs"), FileKind::File);
        let entries = vec![directory.clone()];
        let expanded_directories = HashMap::from([(
            directory.path().to_path_buf(),
            ExpandedDirectory {
                entries: vec![child.clone()].into(),
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

        let selectable_paths = visible_entry_paths(&entries, &expanded_directories);
        let visible = rows(entries, expanded_directories);

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[1], (child.path().to_path_buf(), 1, 0.5));
        assert_eq!(selectable_paths, vec![directory.path()]);
    }
}
