use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserTab, ExpandedDirectory, ExpandedDirectoryStatus,
    SplitAxis,
};
use crate::operation_history::FileOperationOutcome;

fn directory_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn loading_expanded_directory(load_cancel: CancellationToken) -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loading,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 1,
        load_context: None,
        load_cancel: Some(load_cancel),
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

#[test]
fn cross_directory_move_invalidates_source_and_preserves_target_tab_tree() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source_directory = PathBuf::from("/source");
    let target_directory = PathBuf::from("/target");
    let source_path = source_directory.join("item");
    let target_path = target_directory.join("item");
    let target_expanded_directory = target_directory.join("existing");
    let source_load_cancellation = CancellationToken::new();
    let target_load_cancellation = CancellationToken::new();
    let source_pane_id = BrowserPaneId(1);
    let source_tab_id = 20;
    let target_tab_id = 21;

    let mut source_pane = browser.capture_active_pane_snapshot();
    source_pane.id = source_pane_id;
    source_pane.current_dir = source_directory.clone();
    source_pane.entries = vec![directory_entry("/source/item")].into();
    source_pane.selected = Some(source_path.clone());
    source_pane.deepest_open_column_directory = Some(source_path.clone());
    source_pane.expanded_directories.insert(
        source_path.clone(),
        loading_expanded_directory(source_load_cancellation.clone()),
    );
    source_pane.tabs = vec![BrowserTab::directory(source_tab_id, source_directory)];
    source_pane.active_tab_id = source_tab_id;
    source_pane.sync_active_tab_state();

    let mut target_pane = browser.capture_active_pane_snapshot();
    target_pane.current_dir = target_directory.clone();
    target_pane.entries = vec![directory_entry("/target/existing")].into();
    target_pane.deepest_open_column_directory = Some(target_expanded_directory.clone());
    target_pane.expanded_directories.insert(
        target_expanded_directory.clone(),
        loading_expanded_directory(target_load_cancellation.clone()),
    );
    target_pane.tabs = vec![BrowserTab::directory(
        target_tab_id,
        target_directory.clone(),
    )];
    target_pane.active_tab_id = target_tab_id;
    target_pane.sync_active_tab_state();

    browser.panes[0] = target_pane.clone();
    browser.panes.push(source_pane);
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: source_pane_id,
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };
    browser.apply_pane_browsing_snapshot(target_pane);
    let operation_directory = tempfile::tempdir().expect("create operation directory");
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(operation_directory.path().join("state.sqlite"))
            .expect("create operation store"),
    );
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Move {
            transfers: vec![crate::operation_queue::QueuedTransfer::new(
                source_path.clone(),
                target_path.clone(),
            )],
            verification: file_core::FileOperationVerification::default(),
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued move")
        .id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
            transfers: vec![crate::operation_history::CompletedTransfer {
                source: source_path,
                target: target_path,
            }],
            history_eligibility:
                crate::operation_history::FileOperationHistoryEligibility::Replayable,
        }),
    ));

    assert_eq!(browser.current_dir, target_directory);
    assert_eq!(browser.entries.len(), 1);
    assert_eq!(
        browser.deepest_open_column_directory,
        Some(target_expanded_directory.clone())
    );
    let refreshed_target = browser
        .expanded_directories
        .get(&target_expanded_directory)
        .expect("target expansion survives reload scheduling");
    assert!(refreshed_target.load_generation > 1);
    assert!(target_load_cancellation.is_cancelled());
    assert!(browser.directory_load_generation > 0);

    let source_pane = browser
        .pane_by_id(source_pane_id)
        .expect("source pane survives");
    assert!(source_pane.entries.is_empty());
    assert!(source_pane.selected.is_none());
    assert!(source_pane.deepest_open_column_directory.is_none());
    assert!(source_pane.expanded_directories.is_empty());
    let source_tab = source_pane.tabs.first().expect("source tab survives");
    assert!(source_tab.entries.is_empty());
    assert!(source_tab.selected.is_none());
    assert!(source_tab.deepest_open_column_directory.is_none());
    assert!(source_tab.expanded_directories.is_empty());
    assert!(source_load_cancellation.is_cancelled());

    let target_pane = browser
        .pane_by_id(BrowserPaneId::PRIMARY)
        .expect("target pane survives");
    assert_eq!(target_pane.entries.len(), 1);
    assert_eq!(
        target_pane.deepest_open_column_directory.as_ref(),
        Some(&target_expanded_directory)
    );
    assert!(target_pane
        .expanded_directories
        .contains_key(&target_expanded_directory));
    let target_tab = target_pane.tabs.first().expect("target tab survives");
    assert_eq!(target_tab.entries.len(), 1);
    assert_eq!(
        target_tab.deepest_open_column_directory.as_ref(),
        Some(&target_expanded_directory)
    );
    assert!(target_tab
        .expanded_directories
        .contains_key(&target_expanded_directory));
}
