use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::app::FileBrowser;
use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};

fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(path, kind, EntryMetadata::default(), false, false, false)
}

#[test]
fn current_column_distinguishes_pending_empty_and_streamed_entries() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let pane_id = browser.active_pane_id();
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Vec::new().into();
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Discovering;

    assert!(matches!(
        column_content(
            browser.pane_view(pane_id).expect("active pane"),
            &browser.current_dir,
        ),
        ColumnContent::Pending
    ));

    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    assert!(matches!(
        column_content(
            browser.pane_view(pane_id).expect("active pane"),
            &browser.current_dir,
        ),
        ColumnContent::Empty
    ));

    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Discovering;
    browser.entries = vec![test_entry(
        browser.current_dir.join("streamed.txt"),
        FileKind::File,
    )]
    .into();
    let ColumnContent::Entries(entries) = column_content(
        browser.pane_view(pane_id).expect("active pane"),
        &browser.current_dir,
    ) else {
        panic!("streamed entries must be visible before load completion");
    };
    assert_eq!(entries.len(), 1);
}

#[test]
fn child_column_preserves_pending_loaded_and_failed_mappings() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let pane_id = browser.active_pane_id();
    browser.current_dir = PathBuf::from("/workspace");
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    let directory = browser.current_dir.join("project");
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        directory.clone(),
        ExpandedDirectory {
            entries: Vec::new(),
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
    );

    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Pending
    ));

    let expanded = browser
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory");
    expanded
        .entries
        .push(test_entry(directory.join("src"), FileKind::Directory));
    let ColumnContent::Entries(entries) =
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory)
    else {
        panic!("an in-flight child column must retain existing entries");
    };
    assert_eq!(entries.len(), 1);

    let expanded = browser
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory");
    expanded.entries.clear();
    expanded.status = ExpandedDirectoryStatus::Loaded;
    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Empty
    ));

    browser
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory")
        .status = ExpandedDirectoryStatus::Error;
    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Empty
    ));
}
