use std::collections::HashSet;
use std::path::PathBuf;

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};

use crate::app::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, IconGridExpansionAnchor,
    IconGridExpansionContext, IconGridExpansionSessionId, IconGridExpansionState, NavigationMode,
};
use crate::operation_history::{FileOperationCompletion, FileOperationOutcome};
use crate::operation_queue::QueuedFileOperation;

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

fn finish_queued_rename(browser: &mut FileBrowser, from: PathBuf, to: PathBuf) {
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Rename {
            path: from.clone(),
            new_name: to
                .file_name()
                .expect("rename target name")
                .to_string_lossy()
                .into_owned(),
        })
        .error()
        .is_none());
    let task_id = browser
        .operation_queue
        .tasks()
        .last()
        .expect("queued rename")
        .id;
    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename { from, to }),
    ));
}

fn list_request(browser: &FileBrowser, path: &PathBuf) -> ExpandedDirectoryLoadRequest {
    let request = browser
        .pending_list_expansion_follow_request()
        .expect("pending list expansion follow request");
    assert_eq!(&request.path, path);
    request
}

#[test]
fn navigation_cancels_icons_to_list_follow_before_old_load_completes() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let target = project.join("main.rs");
    let destination = PathBuf::from("/elsewhere");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::Icons;
    browser.icon_grid_expansion = Some(IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: root.clone(),
            session_id: IconGridExpansionSessionId::new(43),
        },
        IconGridExpansionAnchor {
            parent_directory: root,
            path: project.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    ));
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let request = list_request(&browser, &project);

    drop(browser.navigate_to(destination.clone(), NavigationMode::RecordHistory));
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project,
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));

    assert_eq!(browser.current_dir, destination);
    assert!(browser.list_expansion_follow.is_none());
    assert!(browser.expanded_directories.is_empty());
    assert_eq!(browser.selected, None);
}

struct NestedIconSelection {
    browser: FileBrowser,
    root: PathBuf,
    project: PathBuf,
    source: PathBuf,
    target: PathBuf,
    other: PathBuf,
}

fn nested_icon_selection() -> NestedIconSelection {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let source = project.join("src");
    let target = source.join("main.rs");
    let other = root.join("other.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(other.clone(), FileKind::File),
    ]
    .into();
    browser.view_mode = BrowserViewMode::Icons;
    let mut expansion = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: root.clone(),
            session_id: IconGridExpansionSessionId::new(44),
        },
        IconGridExpansionAnchor {
            parent_directory: root.clone(),
            path: project.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(source.clone(), FileKind::Directory)]),
    );
    assert!(expansion.insert_directory(
        IconGridExpansionAnchor {
            parent_directory: project.clone(),
            path: source.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    ));
    browser.icon_grid_expansion = Some(expansion);
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target.clone()]);
    NestedIconSelection {
        browser,
        root,
        project,
        source,
        target,
        other,
    }
}

#[test]
fn completed_path_migration_cancels_icons_to_list_follow() {
    let NestedIconSelection {
        mut browser,
        project,
        ..
    } = nested_icon_selection();
    let renamed_project = PathBuf::from("/workspace/renamed");
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let stale_request = list_request(&browser, &project);

    finish_queued_rename(&mut browser, project.clone(), renamed_project);

    assert!(browser.list_expansion_follow.is_none());
    assert!(!browser.expanded_directories.contains_key(&project));
    drop(browser.accept_complete_expanded_directory_fixture(
        stale_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));
    assert!(!browser.expanded_directories.contains_key(&project));
}

#[test]
fn completed_path_migration_cancels_list_to_icons_follow() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let renamed_project = root.join("renamed");
    let source = project.join("src");
    let target = source.join("main.rs");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(source.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        source,
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    );
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(IconGridExpansionState::has_follow_plan));

    finish_queued_rename(&mut browser, project, renamed_project.clone());

    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("renamed expansion remains visible");
    assert!(!state.has_follow_plan());
    assert_eq!(state.root_path(), renamed_project);
}

#[test]
fn stale_list_follow_session_cannot_write_into_restarted_same_path_and_generation() {
    let NestedIconSelection {
        mut browser,
        root,
        project,
        source,
        target,
        other: _,
    } = nested_icon_selection();
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let stale_request = list_request(&browser, &project);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));
    let mut expansion = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: root.clone(),
            session_id: IconGridExpansionSessionId::new(45),
        },
        IconGridExpansionAnchor {
            parent_directory: root,
            path: project.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(source.clone(), FileKind::Directory)]),
    );
    assert!(expansion.insert_directory(
        IconGridExpansionAnchor {
            parent_directory: project.clone(),
            path: source.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    ));
    browser.icon_grid_expansion = Some(expansion);
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let current_request = list_request(&browser, &project);
    assert_eq!(stale_request.path, current_request.path);
    assert_eq!(stale_request.generation, current_request.generation);
    assert_ne!(stale_request.context, current_request.context);

    drop(browser.accept_complete_expanded_directory_fixture(
        stale_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));

    assert!(browser
        .expanded_directories
        .get(&project)
        .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));
    assert_eq!(list_request(&browser, &project), current_request);
}

#[test]
fn browser_tree_request_cannot_write_into_list_follow_owned_node() {
    let NestedIconSelection {
        mut browser,
        project,
        source,
        ..
    } = nested_icon_selection();
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let current_request = list_request(&browser, &project);
    let stale_browser_tree_request = ExpandedDirectoryLoadRequest {
        context: DirectoryExpansionLoadContext::BrowserTree {
            pane_id: BrowserPaneId::PRIMARY,
        },
        path: current_request.path.clone(),
        generation: current_request.generation,
    };

    drop(browser.accept_complete_expanded_directory_fixture(
        stale_browser_tree_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));

    assert!(browser
        .expanded_directories
        .get(&project)
        .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));
    assert_eq!(list_request(&browser, &project), current_request);

    drop(browser.accept_complete_expanded_directory_fixture(
        current_request,
        Ok(DirectoryScan {
            path: project,
            entries: vec![test_entry(source.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    assert!(browser.expanded_directories.contains_key(&source));
}

#[test]
fn stale_nested_selection_does_not_start_list_to_icons_follow() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let stale_target = project.join("removed.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::List;
    browser
        .expanded_directories
        .insert(project, loaded_directory(Vec::new()));
    browser.selected = Some(stale_target.clone());
    browser.selected_paths = HashSet::from([stale_target]);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert!(browser.icon_grid_expansion.is_none());
    assert!(browser.selected.is_none());
}

#[test]
fn select_all_cancels_icons_to_list_follow() {
    let NestedIconSelection {
        mut browser,
        project,
        source,
        target: _,
        other,
        ..
    } = nested_icon_selection();
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let request = list_request(&browser, &project);
    let cancellation = browser
        .expanded_directories
        .get(&project)
        .and_then(|expanded| expanded.load_cancel.clone())
        .expect("list follow cancellation");

    drop(browser.select_all_in_file_selection_scope());
    assert!(cancellation.is_cancelled());
    assert!(!browser.expanded_directories.contains_key(&project));
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: vec![test_entry(source.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));

    assert!(browser.list_expansion_follow.is_none());
    assert!(!browser.expanded_directories.contains_key(&project));
    assert!(!browser.expanded_directories.contains_key(&source));
    assert!(browser.selected_paths.contains(&other));
}

#[test]
fn watcher_refresh_keeps_current_list_follow_session_and_rejects_prior_generation() {
    let NestedIconSelection {
        mut browser,
        project,
        source,
        ..
    } = nested_icon_selection();
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let stale_request = list_request(&browser, &project);

    drop(browser.reload_observed_directory(observed_changes(&project)));
    let refreshed_request = list_request(&browser, &project);
    assert_eq!(stale_request.context, refreshed_request.context);
    assert!(refreshed_request.generation > stale_request.generation);

    drop(browser.accept_complete_expanded_directory_fixture(
        stale_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));
    assert!(browser
        .expanded_directories
        .get(&project)
        .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));

    drop(browser.accept_complete_expanded_directory_fixture(
        refreshed_request,
        Ok(DirectoryScan {
            path: project,
            entries: vec![test_entry(source.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    assert!(browser.expanded_directories.contains_key(&source));
}

#[test]
fn select_all_cancels_list_to_icons_follow() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let source = project.join("src");
    let target = source.join("main.rs");
    let other = root.join("other.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(other.clone(), FileKind::File),
    ]
    .into();
    browser.view_mode = BrowserViewMode::List;
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(source.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        source.clone(),
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    );
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));
    let request = {
        let state = browser
            .icon_grid_expansion
            .as_ref()
            .expect("icon grid expansion");
        let expanded = state.directory(&project).expect("expanded project");
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::IconGrid {
                pane_id: state.context().pane_id,
                current_dir: state.context().current_dir.clone(),
                session_id: state.context().session_id,
            },
            path: project.clone(),
            generation: expanded.contents.load_generation,
        }
    };

    drop(browser.select_all_in_file_selection_scope());
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project,
            entries: vec![test_entry(source.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    for _ in 0..8 {
        drop(browser.advance_icon_grid_expansion_animation());
    }

    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| !state.has_follow_plan() && state.directory(&source).is_none()));
    assert!(browser.selected_paths.contains(&other));
}

fn observed_changes(directory: &std::path::Path) -> file_core::DirectoryEntryChanges {
    file_core::DirectoryEntryChanges {
        directory: directory.to_path_buf(),
        changes: vec![file_core::ResolvedEntryChange::RescanRequired],
    }
}
