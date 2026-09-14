use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::app::FileBrowser;
use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};

fn test_directory_entry(path: PathBuf) -> DirectoryEntry {
    DirectoryEntry::new(
        path,
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn browser_with_expanded_directory(
    status: ExpandedDirectoryStatus,
    child_entries: Vec<DirectoryEntry>,
) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    let expanded_path = browser.current_dir.join("project");
    browser.entries = vec![test_directory_entry(expanded_path.clone())].into();
    browser.expanded_directories.insert(
        expanded_path,
        ExpandedDirectory {
            entries: child_entries,
            directory_discovery: None,
            status,
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
    browser
}

fn expanded_directory_status_row_exists(browser: &FileBrowser) -> bool {
    let pane = browser
        .pane_view(browser.active_pane_id())
        .expect("active pane");
    let rows = crate::transfer_placeholder_view::build_list_transfer_rows(
        browser,
        pane,
        ListGeometry::for_level(browser.user_config().list_view_density).row_height,
    );
    rows.iter().any(|row| {
        matches!(
            row,
            crate::transfer_placeholder_view::ListTransferRow::DirectoryStatusRow { .. }
        )
    })
}

#[test]
fn loading_expanded_directory_does_not_render_status_row() {
    let browser = browser_with_expanded_directory(ExpandedDirectoryStatus::Loading, Vec::new());

    assert!(!expanded_directory_status_row_exists(&browser));
}

#[test]
fn expanded_directory_keeps_error_and_empty_status_rows() {
    let failed = browser_with_expanded_directory(ExpandedDirectoryStatus::Error, Vec::new());
    let empty = browser_with_expanded_directory(ExpandedDirectoryStatus::Loaded, Vec::new());

    assert!(expanded_directory_status_row_exists(&failed));
    assert!(expanded_directory_status_row_exists(&empty));
}

#[test]
fn loaded_expanded_directory_with_children_has_no_status_row() {
    let child = test_directory_entry(PathBuf::from("/workspace/project/src"));
    let browser = browser_with_expanded_directory(ExpandedDirectoryStatus::Loaded, vec![child]);

    assert!(!expanded_directory_status_row_exists(&browser));
}
