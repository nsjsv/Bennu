use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::app::FileBrowser;
use crate::model::BrowserLocation;
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
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    let expanded_path = browser.tab().current_directory().unwrap().join("project");
    browser.tab_mut().entries = vec![test_directory_entry(expanded_path.clone())].into();
    browser.tab_mut().expanded_directories.insert(
        expanded_path,
        ExpandedDirectory {
            entries: child_entries.into(),
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
    let layout = crate::transfer_placeholder_view::list_row_layout(
        browser,
        pane,
        ListGeometry::for_level(browser.user_config().list_view_density).row_height,
    );
    (0..layout.len()).any(|row| {
        matches!(
            layout.row(pane, row),
            Some(crate::transfer_placeholder_view::ListTransferRow::DirectoryStatusRow { .. })
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

#[test]
fn refreshing_expanded_directory_keeps_showing_its_children() {
    // 磁盘变化触发的重扫期间照常显示旧条目,结果落地整体替换;子行消失再出现就是一闪。
    let child = test_directory_entry(PathBuf::from("/workspace/project/src"));
    let browser = browser_with_expanded_directory(ExpandedDirectoryStatus::Refreshing, vec![child]);
    let pane = browser
        .pane_view(browser.active_pane_id())
        .expect("active pane");
    let layout = crate::transfer_placeholder_view::list_row_layout(
        &browser,
        pane,
        ListGeometry::for_level(browser.user_config().list_view_density).row_height,
    );

    let paths = (0..layout.len())
        .filter_map(|row| match layout.row(pane, row)? {
            crate::transfer_placeholder_view::ListTransferRow::Entry { visible, .. } => {
                Some(visible.entry.path().to_path_buf())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(paths.contains(&PathBuf::from("/workspace/project/src")));
}
