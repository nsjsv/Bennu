use std::path::PathBuf;

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, IconGridExpansionAnchor,
    IconGridExpansionContext, IconGridExpansionSessionId, IconGridExpansionState,
};
use crate::operation_history::{FileOperationCompletion, FileOperationOutcome};
use crate::operation_queue::QueuedFileOperation;

fn loaded_directory() -> ExpandedDirectory {
    ExpandedDirectory {
        entries: Vec::new(),
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

#[test]
fn completed_root_rename_migrates_temporary_icon_grid_tree() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let root = PathBuf::from("/workspace/root");
    let renamed = PathBuf::from("/workspace/renamed");
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = BrowserViewMode::Icons;
    browser.entries = vec![directory_entry("/workspace/root")].into();
    browser.icon_grid_expansion = Some(IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: browser.current_dir.clone(),
            session_id: IconGridExpansionSessionId::new(4),
        },
        IconGridExpansionAnchor {
            parent_directory: browser.current_dir.clone(),
            path: root.clone(),
            index: 0,
        },
        ExpandedDirectory {
            entries: vec![DirectoryEntry::new(
                root.join("report.txt"),
                FileKind::File,
                EntryMetadata::default(),
                false,
                false,
                false,
            )],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 1,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    ));

    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Rename {
            path: root.clone(),
            new_name: "renamed".into(),
        })
        .error()
        .is_none());
    let task_id = browser.operation_queue.tasks().last().unwrap().id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Rename {
            from: root,
            to: renamed.clone(),
        }),
    ));

    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("root rename preserves icon expansion");
    assert_eq!(state.root_path(), renamed);
    let context = state.context().clone();
    let generation = state.directory(&renamed).unwrap().contents.load_generation;
    assert!(matches!(
        state.directory(&renamed).unwrap().contents.status,
        ExpandedDirectoryStatus::Loading,
    ));
    assert!(state
        .directory(&renamed)
        .unwrap()
        .contents
        .entries
        .is_empty());

    drop(browser.accept_complete_expanded_directory_fixture(
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::IconGrid {
                pane_id: context.pane_id,
                current_dir: context.current_dir,
                session_id: context.session_id,
            },
            path: renamed.clone(),
            generation,
        },
        Ok(DirectoryScan {
            path: renamed.clone(),
            entries: vec![DirectoryEntry::new(
                renamed.join("report.txt"),
                FileKind::File,
                EntryMetadata::default(),
                false,
                false,
                false,
            )],
            skipped: Vec::new(),
        }),
    ));

    let state = browser.icon_grid_expansion.as_ref().unwrap();
    assert_eq!(
        state
            .directory(&renamed)
            .unwrap()
            .contents
            .entries
            .first()
            .map(|entry| entry.path.clone()),
        Some(PathBuf::from("/workspace/renamed/report.txt")),
    );
    assert_eq!(
        state
            .entry(&PathBuf::from("/workspace/renamed/report.txt"))
            .map(|entry| entry.path.clone()),
        Some(PathBuf::from("/workspace/renamed/report.txt"))
    );
}

#[test]
fn successful_trash_prunes_deleted_icon_grid_branch_before_reload() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let root = PathBuf::from("/workspace/root");
    let branch = PathBuf::from("/workspace/root/branch");
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = BrowserViewMode::Icons;
    browser.entries = vec![directory_entry("/workspace/root")].into();
    let mut state = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: browser.current_dir.clone(),
            session_id: IconGridExpansionSessionId::new(5),
        },
        IconGridExpansionAnchor {
            parent_directory: browser.current_dir.clone(),
            path: root.clone(),
            index: 0,
        },
        ExpandedDirectory {
            entries: vec![directory_entry("/workspace/root/branch")],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 1,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );
    assert!(state.insert_directory(
        IconGridExpansionAnchor {
            parent_directory: root.clone(),
            path: branch.clone(),
            index: 0,
        },
        loaded_directory(),
    ));
    browser.icon_grid_expansion = Some(state);
    assert!(browser
        .operation_queue
        .enqueue(QueuedFileOperation::Trash {
            paths: vec![branch.clone()],
        })
        .error()
        .is_none());
    let task_id = browser.operation_queue.tasks().last().unwrap().id;

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::Trash {
            paths: vec![branch.clone()],
            entries: Vec::new(),
            tracking_warnings: Vec::new(),
        }),
    ));

    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("deleting a nested branch preserves the root tree");
    assert_eq!(state.root_path(), root);
    assert!(state.directory(&branch).is_none());
}
