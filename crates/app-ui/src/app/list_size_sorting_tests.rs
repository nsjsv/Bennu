use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode,
    ColumnBrowserViewport, ExpandedDirectory, ExpandedDirectoryStatus,
    ListDirectorySizeDisplayMode, ListDirectorySummary, SplitAxis,
};
use crate::thumbnail_cache::ColumnViewport;

fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(path, kind, EntryMetadata::default(), false, false, false)
}

fn loaded_directory(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
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
    }
}

fn pane_from_tab_for_test(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
    BrowserPane {
        id: pane_id,
        current_dir: tab.directory.clone(),
        is_trash_view: tab.is_trash_view,
        entries: tab.entries.clone(),
        directory_discovery: tab.directory_discovery.clone(),
        directory_loading_placeholder: None,
        trash_entries: tab.trash_entries.clone(),
        selected: tab.selected.clone(),
        selected_paths: tab.selected_paths.clone(),
        selection_anchor: tab.selection_anchor.clone(),
        deepest_open_column_directory: tab.deepest_open_column_directory.clone(),
        expanded_directories: tab.expanded_directories.clone(),
        view_mode: tab.view_mode,
        column_browser_viewport: ColumnBrowserViewport::default(),
        column_viewports: HashMap::<PathBuf, ColumnViewport>::new(),
        tabs: vec![tab.clone()],
        active_tab_id: tab.id,
        directory_load_generation: 0,
        directory_load_cancel: None,
        back_stack: Vec::new(),
        forward_stack: Vec::new(),
        directory_collection_phase: crate::model::DirectoryCollectionPhase::Ready,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

#[test]
fn recursive_size_sort_reorders_expanded_entries_and_inactive_pane() {
    let active_root = PathBuf::from("/workspace/active");
    let parent = active_root.join("parent");
    let child_large = parent.join("large");
    let child_small = parent.join("small");
    let inactive_root = PathBuf::from("/workspace/inactive");
    let inactive_large = inactive_root.join("wide");
    let inactive_small = inactive_root.join("narrow");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    browser.current_dir = active_root.clone();
    browser.view_mode = BrowserViewMode::List;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.user_config.list_directory_size_display_mode =
        ListDirectorySizeDisplayMode::RecursiveTotalSize;
    browser.options.sort_field = SortField::Size;
    browser.options.sort_direction = SortDirection::Ascending;
    browser.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![
            test_entry(child_large.clone(), FileKind::Directory),
            test_entry(child_small.clone(), FileKind::Directory),
        ]),
    );

    let mut active_tab = BrowserTab::directory(0, active_root);
    active_tab.view_mode = BrowserViewMode::List;
    active_tab.entries = browser.entries.clone();
    active_tab.expanded_directories = browser.expanded_directories.clone();
    active_tab.selected_paths = HashSet::new();
    let mut inactive_tab = BrowserTab::directory(1, inactive_root.clone());
    inactive_tab.view_mode = BrowserViewMode::List;
    inactive_tab.entries = vec![
        test_entry(inactive_large.clone(), FileKind::Directory),
        test_entry(inactive_small.clone(), FileKind::Directory),
    ]
    .into();
    inactive_tab.selected_paths = HashSet::new();
    browser.tabs = vec![active_tab.clone()];
    browser.active_tab_id = active_tab.id;
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };
    browser.panes = vec![
        pane_from_tab_for_test(BrowserPaneId::PRIMARY, active_tab),
        pane_from_tab_for_test(BrowserPaneId(1), inactive_tab),
    ];

    for (path, size) in [
        (child_large.clone(), 200u64),
        (child_small.clone(), 100u64),
        (inactive_large.clone(), 300u64),
        (inactive_small.clone(), 50u64),
    ] {
        let request = browser
            .list_directory_summary_cache
            .start_request(path, true)
            .expect("summary request");
        drop(browser.accept_list_directory_summary(
            request,
            Ok(ListDirectorySummary {
                direct_child_count: 0,
                recursive_total_size_bytes: Some(size),
                files_total_size_bytes: Some(size),
                hidden_files_total_size_bytes: Some(0),
                hidden_entry_count: Some(0),
            }),
        ));
    }

    assert_eq!(
        browser.expanded_directories[&parent]
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![child_small, child_large]
    );
    assert_eq!(
        browser
            .pane_by_id(BrowserPaneId(1))
            .expect("inactive pane")
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![inactive_small, inactive_large]
    );
}
