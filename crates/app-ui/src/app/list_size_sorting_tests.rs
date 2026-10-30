use std::collections::HashSet;
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode, ExpandedDirectory,
    ExpandedDirectoryStatus, ListDirectorySizeDisplayMode, ListDirectorySummary, SplitAxis,
};

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

fn pane_from_tab_for_test(pane_id: BrowserPaneId, mut tab: BrowserTab) -> BrowserPane {
    tab.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    BrowserPane::new(pane_id, tab)
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

    browser.user_config.list_directory_size_display_mode =
        ListDirectorySizeDisplayMode::RecursiveTotalSize;
    browser.options.sort_field = SortField::Size;
    browser.options.sort_direction = SortDirection::Ascending;

    let mut active_tab = BrowserTab::directory(0, active_root);
    active_tab.view_mode = BrowserViewMode::List;
    active_tab.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    active_tab.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![
            test_entry(child_large.clone(), FileKind::Directory),
            test_entry(child_small.clone(), FileKind::Directory),
        ]),
    );
    let mut inactive_tab = BrowserTab::directory(1, inactive_root.clone());
    inactive_tab.view_mode = BrowserViewMode::List;
    inactive_tab.entries = vec![
        test_entry(inactive_large.clone(), FileKind::Directory),
        test_entry(inactive_small.clone(), FileKind::Directory),
    ]
    .into();
    inactive_tab.selected_paths = HashSet::new();
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
                recursive_total_size: Some(crate::model::RecursiveTotalSize {
                    bytes: size,
                    complete: true,
                }),
                files_total_size_bytes: Some(size),
                hidden_files_total_size_bytes: Some(0),
                hidden_entry_count: Some(0),
            }),
        ));
    }

    assert_eq!(
        browser.tab().expanded_directories[&parent]
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
            .tab()
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![inactive_small, inactive_large]
    );
}
