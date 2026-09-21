use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::config;
use crate::model::{
    BrowserTab, BrowserViewMode, FileDragStationaryAction, FileDropOrigin, FileDropTarget,
    InternalFileDragSnapshot, TabFileDropTarget,
};

fn file_entry(path: PathBuf) -> DirectoryEntry {
    DirectoryEntry::new(
        path,
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

#[test]
fn column_tab_hover_renders_target_columns_without_losing_source_snapshot() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let pane_id = browser.active_pane_id();
    let source_tab_id = browser.active_tab_id;
    let root = PathBuf::from("/workspace");
    let source_directory = root.join("source");
    let target_directory = root.join("target");
    let source_path = root.join("report.txt");

    browser.current_dir = root.clone();
    browser.view_mode = BrowserViewMode::Columns;
    browser.deepest_open_column_directory = Some(source_directory.clone());
    browser.entries = vec![file_entry(source_path.clone())].into();
    browser.selected_paths.insert(source_path.clone());
    drop(browser.start_file_drag(
        source_path,
        FileDragStationaryAction::ActivateColumnEntry,
        vec![root.clone(), source_directory.clone()],
    ));

    let target_tab_id = source_tab_id + 1;
    let mut target_tab = BrowserTab::directory(target_tab_id, root.clone());
    target_tab.deepest_open_column_directory = Some(target_directory.clone());
    browser.tabs.push(target_tab);

    drop(browser.select_tab_for_file_drop(pane_id, target_tab_id));

    assert_eq!(
        crate::three_column_view::column_directories_for_pane(
            browser.pane_view(pane_id).expect("target pane"),
        ),
        vec![root.clone(), target_directory],
    );
    assert_eq!(
        browser
            .file_drag
            .as_ref()
            .and_then(|drag| drag.source_column_directories(pane_id, source_tab_id)),
        Some(&[root, source_directory][..]),
    );
}

#[test]
fn column_tab_drop_destination_follows_displayed_directory_only_for_columns() {
    let root = PathBuf::from("/workspace");
    let deepest = root.join("project/src");
    let mut tab = BrowserTab::directory(1, root.clone());
    tab.deepest_open_column_directory = Some(deepest.clone());

    tab.view_mode = BrowserViewMode::Columns;
    assert_eq!(
        tab.file_drop_destination(),
        TabDropDestination::Directory(deepest),
    );

    for mode in [BrowserViewMode::List, BrowserViewMode::Icons] {
        tab.view_mode = mode;
        assert_eq!(
            tab.file_drop_destination(),
            TabDropDestination::Directory(root.clone()),
        );
    }

    assert_eq!(
        BrowserTab::trash(2).file_drop_destination(),
        TabDropDestination::Trash,
    );
}

#[test]
fn column_tab_drop_rejects_destination_changed_after_target_was_frozen() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let pane_id = browser.active_pane_id();
    let root = PathBuf::from("/workspace");
    let original_destination = root.join("project");
    let replacement_destination = root.join("archive");
    let tab_id = browser.active_tab_id + 1;
    let mut tab = BrowserTab::directory(tab_id, root);
    tab.deepest_open_column_directory = Some(original_destination);
    let target = TabFileDropTarget {
        pane_id,
        tab_id,
        destination: tab.file_drop_destination(),
    };
    browser.tabs.push(tab);

    drop(browser.dispatch_file_drop(
        FileDropOrigin::External,
        Some(FileDropTarget::Tab(target.clone())),
        vec![PathBuf::from("/outside/report.txt")],
    ));
    assert!(browser.file_drop_prompt.is_some());

    browser.file_drop_prompt = None;
    browser
        .tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .expect("target tab")
        .deepest_open_column_directory = Some(replacement_destination);
    let source = PathBuf::from("/outside/report.txt");
    let task = browser.dispatch_file_drop(
        FileDropOrigin::Internal(InternalFileDragSnapshot {
            source_session_id: None,
            sources: vec![source.clone()],
            bookmark_source: None,
        }),
        Some(FileDropTarget::Tab(target)),
        vec![source],
    );

    assert!(iced_runtime::task::into_stream(task).is_none());
    assert!(browser.file_drop_prompt.is_none());
    assert!(browser.operation_queue.tasks().is_empty());
}
