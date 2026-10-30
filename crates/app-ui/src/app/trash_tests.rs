use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind, ScanWarning, TrashEntry, TrashScan};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, NavigationMode, SplitAxis,
};

fn pane_from_tab(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
    BrowserPane::new(pane_id, tab)
}

#[test]
fn file_operation_without_a_trash_tab_discards_the_cached_snapshot() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.trash_refresh.replace_snapshot(TrashScan {
        entries: Vec::new(),
        skipped: Vec::new(),
    });

    drop(browser.refresh_trash_snapshot_for_visible_panes());

    assert!(browser.trash_refresh.snapshot().is_none());
}

#[test]
fn opening_trash_projects_the_cached_snapshot_without_a_loading_placeholder() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let trash_path = PathBuf::from("/trash/files/cached.txt");
    let entry = TrashEntry::from_historical_entry(
        trash_path.clone(),
        PathBuf::from("/trash/info/cached.txt.trashinfo"),
        PathBuf::from("/home/test/cached.txt"),
        None,
        DirectoryEntry::new(
            trash_path.clone(),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        ),
    );
    browser.trash_refresh.replace_snapshot(TrashScan {
        entries: vec![entry],
        skipped: Vec::new(),
    });

    drop(browser.open_trash_view(NavigationMode::RecordHistory));

    assert!(browser.tab().is_trash_view);
    assert!(!browser.tab().directory_collection_phase.is_discovering());
    assert!(browser.pane().directory_loading_placeholder.is_none());
    assert_eq!(browser.tab().entries.len(), 1);
    assert_eq!(browser.tab().entries[0].path, trash_path);
}

#[test]
fn failed_refresh_keeps_the_last_valid_snapshot_and_its_local_warnings() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let first = browser
        .trash_refresh
        .begin_if_idle()
        .expect("initial refresh");
    drop(browser.accept_trash_refresh_completion(
        first.generation,
        Ok(TrashScan {
            entries: Vec::new(),
            skipped: vec![ScanWarning {
                path: PathBuf::from("/volume/.Trash-1000"),
                message: "permission denied".to_owned(),
            }],
        }),
    ));
    let second = browser.trash_refresh.begin_if_idle().expect("next refresh");

    drop(browser.accept_trash_refresh_completion(
        second.generation,
        Err("mount snapshot unavailable".to_owned()),
    ));

    let snapshot = browser.trash_refresh.snapshot().expect("valid snapshot");
    assert_eq!(snapshot.skipped.len(), 1);
    assert_eq!(
        browser.trash_refresh.last_error(),
        Some("mount snapshot unavailable")
    );
}

#[test]
fn completed_refresh_removes_unmounted_entries_and_selection_from_all_trash_panes() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let primary_id = BrowserPaneId::PRIMARY;
    let secondary_id = BrowserPaneId(1);
    let trashed_path = PathBuf::from("/volume/.Trash-1000/files/item.txt");
    let directory_entry = DirectoryEntry::new(
        trashed_path.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    );
    let trash_entry = TrashEntry::from_historical_entry(
        trashed_path.clone(),
        PathBuf::from("/volume/.Trash-1000/info/item.txt.trashinfo"),
        PathBuf::from("/volume/item.txt"),
        None,
        directory_entry.clone(),
    );
    let trash_tab = |id| {
        let mut tab = BrowserTab::trash(id);
        tab.entries = vec![directory_entry.clone()].into();
        tab.trash_entries = vec![trash_entry.clone()];
        tab.selected = Some(trashed_path.clone());
        tab.selected_paths.insert(trashed_path.clone());
        tab.selection_anchor = Some(trashed_path.clone());
        tab
    };
    let primary = pane_from_tab(primary_id, trash_tab(1));
    let secondary = pane_from_tab(secondary_id, trash_tab(2));
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: primary_id,
        second: secondary_id,
        active: primary_id,
        first_portion: 500,
    };
    browser.panes = vec![primary, secondary];
    browser.reset_active_pane_context();
    let request = browser.trash_refresh.begin_if_idle().unwrap();

    drop(browser.accept_trash_refresh_completion(
        request.generation,
        Ok(TrashScan {
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));

    for pane in &browser.panes {
        let tab = pane.tab();
        assert!(tab.entries.is_empty());
        assert!(tab.trash_entries.is_empty());
        assert!(tab.selected.is_none());
        assert!(tab.selected_paths.is_empty());
        assert!(tab.selection_anchor.is_none());
        assert!(!tab.directory_collection_phase.is_discovering());
    }
}

#[test]
fn completed_refresh_survives_initiating_tab_close_and_updates_other_trash_pane() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let primary_id = BrowserPaneId::PRIMARY;
    let secondary_id = BrowserPaneId(1);
    let primary_trash = pane_from_tab(primary_id, BrowserTab::trash(1));
    let mut secondary_tab = BrowserTab::trash(2);
    let trashed_path = PathBuf::from("/trash/files/item.txt");
    secondary_tab.selected = Some(trashed_path.clone());
    secondary_tab.selected_paths.insert(trashed_path.clone());
    let secondary_trash = pane_from_tab(secondary_id, secondary_tab);
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: primary_id,
        second: secondary_id,
        active: primary_id,
        first_portion: 500,
    };
    browser.panes = vec![primary_trash, secondary_trash];
    browser.reset_active_pane_context();

    let request = browser
        .trash_refresh
        .begin_if_idle()
        .expect("initial trash refresh");

    let primary_directory = pane_from_tab(
        primary_id,
        BrowserTab::directory(3, PathBuf::from("/workspace")),
    );
    browser.panes[0] = primary_directory;
    browser.reset_active_pane_context();

    let entry = TrashEntry::from_historical_entry(
        trashed_path.clone(),
        PathBuf::from("/trash/info/item.txt.trashinfo"),
        PathBuf::from("/home/test/item.txt"),
        None,
        DirectoryEntry::new(
            trashed_path.clone(),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        ),
    );
    drop(browser.accept_trash_refresh_completion(
        request.generation,
        Ok(TrashScan {
            entries: vec![entry],
            skipped: Vec::new(),
        }),
    ));

    let secondary = browser.pane_by_id(secondary_id).expect("secondary pane");
    assert_eq!(secondary.tab().trash_entries.len(), 1);
    assert_eq!(secondary.tab().selected.as_ref(), Some(&trashed_path));
    assert!(secondary.tab().selected_paths.contains(&trashed_path));
    assert_eq!(
        browser
            .trash_refresh
            .snapshot()
            .expect("global snapshot")
            .entries
            .len(),
        1
    );
}

#[test]
fn trash_watch_event_refreshes_snapshot_but_unrelated_paths_do_not() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().is_trash_view = true;
    let watch_root = file_core::trash_bin::trash_watch_directories()
        .into_iter()
        .next()
        .expect("home trash watch root");

    drop(browser.reload_observed_directory(observed_changes(&watch_root.join("info"))));
    assert!(browser.trash_refresh.begin_if_idle().is_none());

    drop(
        browser.reload_observed_directory(observed_changes(Path::new("/tmp/unrelated-directory"))),
    );
    assert!(browser.trash_refresh.begin_if_idle().is_none());
}

#[test]
fn trash_watch_event_during_batch_operation_defers_rescan_until_it_finishes() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().is_trash_view = true;
    let watch_root = file_core::trash_bin::trash_watch_directories()
        .into_iter()
        .next()
        .expect("home trash watch root");

    drop(browser.enqueue_file_operation(crate::operation_queue::QueuedFileOperation::EmptyTrash));

    // 批量任务运行中:watcher 事件只记脏,不发起重扫。
    drop(browser.reload_observed_directory(observed_changes(&watch_root.join("info"))));
    assert!(browser.trash_refresh.begin_if_idle().is_some());

    // 任务终结:统一补刷一次,重扫被占用。
    let task_id = browser
        .operation_queue
        .tasks()
        .iter()
        .find(|task| task.operation.changes_trash())
        .expect("queued trash operation")
        .id;
    drop(browser.update(crate::model::Message::FileOperationFinished(
        task_id,
        0,
        file_operations::FileOperationCompletion::Succeeded(
            file_operations::FileOperationOutcome::NoHistory,
        ),
    )));
    assert!(browser.trash_refresh.begin_if_idle().is_none());
}

fn observed_changes(directory: &std::path::Path) -> file_core::DirectoryEntryChanges {
    file_core::DirectoryEntryChanges {
        directory: directory.to_path_buf(),
        changes: vec![file_core::ResolvedEntryChange::RescanRequired],
    }
}
