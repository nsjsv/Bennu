use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext, DirectoryLoadFailure,
    ExpandedDirectory, ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus,
    IconGridExpansionAnchor, IconGridExpansionContext, IconGridExpansionSessionId,
    IconGridExpansionState, NavigationMode,
};

const ROOT_GENERATION: u64 = 7;

fn entry(path: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn expanded(
    entries: Vec<DirectoryEntry>,
    status: ExpandedDirectoryStatus,
    generation: u64,
) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: generation,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn root_anchor() -> IconGridExpansionAnchor {
    IconGridExpansionAnchor {
        parent_directory: PathBuf::from("/workspace"),
        path: PathBuf::from("/workspace/root"),
        index: 0,
    }
}

fn root_context() -> IconGridExpansionContext {
    IconGridExpansionContext {
        pane_id: BrowserPaneId::PRIMARY,
        current_dir: PathBuf::from("/workspace"),
        session_id: IconGridExpansionSessionId::new(11),
    }
}

fn icon_request(
    browser: &mut FileBrowser,
    path: &Path,
    generation: u64,
) -> ExpandedDirectoryLoadRequest {
    let context = root_context();
    let request = ExpandedDirectoryLoadRequest {
        context: DirectoryExpansionLoadContext::IconGrid {
            pane_id: context.pane_id,
            current_dir: context.current_dir,
            session_id: context.session_id,
        },
        path: path.to_path_buf(),
        generation,
    };
    browser
        .icon_grid_expansion
        .as_mut()
        .and_then(|state| state.directory_mut(path))
        .expect("expanded icon directory")
        .contents
        .load_context = Some(request.context.clone());
    request
}

fn browser_with_root(root_contents: ExpandedDirectory) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = BrowserViewMode::Icons;
    browser.entries = vec![entry("/workspace/root", FileKind::Directory)].into();
    browser.icon_grid_expansion = Some(IconGridExpansionState::new(
        root_context(),
        root_anchor(),
        root_contents,
    ));
    browser
}

fn browser_with_loaded_root() -> FileBrowser {
    browser_with_root(expanded(
        vec![entry("/workspace/root/photo.png", FileKind::File)],
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ))
}

#[test]
fn unavailable_icon_root_is_removed_without_a_global_error() {
    let mut browser = browser_with_root(expanded(
        Vec::new(),
        ExpandedDirectoryStatus::Loading,
        ROOT_GENERATION,
    ));
    let root = PathBuf::from("/workspace/root");
    let request = icon_request(&mut browser, &root, ROOT_GENERATION);

    drop(browser.accept_complete_icon_grid_directory_fixture(
        request,
        Err(DirectoryLoadFailure::DirectoryUnavailable {
            message: "root disappeared".to_owned(),
        }),
    ));

    assert!(browser.icon_grid_expansion.is_none());
    assert_eq!(browser.current_error(), None);
}

#[test]
fn unavailable_icon_child_removes_only_its_subtree_without_a_global_error() {
    let child = PathBuf::from("/workspace/root/child");
    let mut browser = browser_with_root(expanded(
        vec![entry("/workspace/root/child", FileKind::Directory)],
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ));
    assert!(browser
        .icon_grid_expansion
        .as_mut()
        .unwrap()
        .insert_directory(
            IconGridExpansionAnchor {
                parent_directory: PathBuf::from("/workspace/root"),
                path: child.clone(),
                index: 0,
            },
            expanded(Vec::new(), ExpandedDirectoryStatus::Loading, 13),
        ));

    let request = icon_request(&mut browser, &child, 13);
    drop(browser.accept_complete_icon_grid_directory_fixture(
        request,
        Err(DirectoryLoadFailure::DirectoryUnavailable {
            message: "child disappeared".to_owned(),
        }),
    ));

    let state = browser.icon_grid_expansion.as_ref().unwrap();
    assert!(state.directory(Path::new("/workspace/root")).is_some());
    assert!(state.directory(&child).is_none());
    assert_eq!(browser.current_error(), None);
}

#[test]
fn icon_read_failure_keeps_an_error_panel_and_reports_the_failure() {
    let mut browser = browser_with_root(expanded(
        Vec::new(),
        ExpandedDirectoryStatus::Loading,
        ROOT_GENERATION,
    ));
    let root = PathBuf::from("/workspace/root");
    let request = icon_request(&mut browser, &root, ROOT_GENERATION);

    drop(browser.accept_complete_icon_grid_directory_fixture(
        request,
        Err(DirectoryLoadFailure::ReadFailed {
            message: "permission denied".to_owned(),
        }),
    ));

    assert_eq!(browser.current_error(), Some("permission denied"));
    assert!(matches!(
        browser
            .icon_grid_expansion
            .as_ref()
            .and_then(|state| state.directory(&root))
            .map(|directory| &directory.contents.status),
        Some(ExpandedDirectoryStatus::Error)
    ));
}

#[test]
fn observed_path_refreshes_icon_and_persistent_directory_owners() {
    let root = PathBuf::from("/workspace/root");
    let mut browser = browser_with_root(expanded(
        Vec::new(),
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ));
    browser.expanded_directories.insert(
        root.clone(),
        expanded(Vec::new(), ExpandedDirectoryStatus::Loaded, 3),
    );

    drop(browser.reload_observed_directory(observed_changes(&root)));

    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .and_then(|state| state.directory(&root))
            .map(|directory| directory.contents.load_generation),
        Some(ROOT_GENERATION + 1),
    );
    assert_eq!(
        browser
            .expanded_directories
            .get(&root)
            .map(|directory| directory.load_generation),
        Some(4),
    );
}

#[test]
fn icons_entry_lookup_never_reads_hidden_persistent_children() {
    let hidden = entry("/workspace/root/hidden.txt", FileKind::File);
    let root = PathBuf::from("/workspace/root");
    let mut browser = browser_with_root(expanded(
        vec![hidden.clone()],
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ));
    browser.expanded_directories.insert(
        root,
        expanded(vec![hidden.clone()], ExpandedDirectoryStatus::Loaded, 2),
    );
    browser.icon_grid_expansion = None;

    assert!(browser.entry_for_path(&hidden.path).is_none());

    browser.icon_grid_expansion = Some(IconGridExpansionState::new(
        root_context(),
        root_anchor(),
        expanded(
            vec![hidden.clone()],
            ExpandedDirectoryStatus::Loaded,
            ROOT_GENERATION,
        ),
    ));
    assert_eq!(
        browser
            .entry_for_path(&hidden.path)
            .map(|entry| &entry.path),
        Some(&hidden.path),
    );
}

#[test]
fn icon_navigation_does_not_snapshot_hidden_persistent_rows() {
    let mut browser = browser_with_loaded_root();
    browser.expanded_directories.insert(
        PathBuf::from("/workspace/hidden"),
        expanded(
            vec![entry("/workspace/hidden/file.txt", FileKind::File)],
            ExpandedDirectoryStatus::Loaded,
            3,
        ),
    );

    drop(browser.navigate_to(
        PathBuf::from("/workspace/next"),
        NavigationMode::RecordHistory,
    ));

    assert!(browser.directory_loading_placeholder.is_none());
}

#[test]
fn refreshing_icon_subtree_removes_selection_before_entries_leave_layout() {
    let hidden = PathBuf::from("/workspace/root/hidden.txt");
    let mut browser = browser_with_root(expanded(
        vec![entry("/workspace/root/hidden.txt", FileKind::File)],
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ));
    browser.selected = Some(hidden.clone());
    browser.selected_paths.insert(hidden.clone());
    browser.selection_anchor = Some(hidden.clone());
    browser.pending_created_entry_rename = Some(hidden.clone());
    browser.preview = Some(crate::model::PreviewState::Loading(hidden));

    let command = browser
        .reload_observed_icon_grid_directory(Path::new("/workspace/root"))
        .expect("interactive root refresh");

    assert!(command.units() > 0);
    assert!(browser.selected_paths.is_empty());
    assert_eq!(browser.selected, None);
    assert_eq!(browser.selection_anchor, None);
    assert_eq!(browser.pending_created_entry_rename, None);
    assert!(browser.preview.is_none());
    let root = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .directory(Path::new("/workspace/root"))
        .unwrap();
    assert!(root.contents.entries.is_empty());
    assert!(matches!(
        root.contents.status,
        ExpandedDirectoryStatus::Loading,
    ));
}

#[test]
fn disclosure_rotation_follows_opening_and_closing_progress() {
    let mut browser = browser_with_loaded_root();
    let root = browser
        .icon_grid_expansion
        .as_mut()
        .unwrap()
        .directory_mut(Path::new("/workspace/root"))
        .unwrap();
    root.contents.animation_progress = 0.4;

    assert_eq!(
        browser.icon_grid_disclosure(
            BrowserPaneId::PRIMARY,
            Path::new("/workspace"),
            Path::new("/workspace/root"),
        ),
        Some((36.0, true)),
    );

    browser
        .icon_grid_expansion
        .as_mut()
        .unwrap()
        .begin_directory_dismissal(Path::new("/workspace/root"));
    assert_eq!(
        browser.icon_grid_disclosure(
            BrowserPaneId::PRIMARY,
            Path::new("/workspace"),
            Path::new("/workspace/root"),
        ),
        Some((36.0, false)),
    );
}

#[test]
fn completing_open_animation_schedules_newly_interactive_thumbnails() {
    let mut browser = browser_with_loaded_root();
    browser
        .icon_grid_expansion
        .as_mut()
        .unwrap()
        .directory_mut(Path::new("/workspace/root"))
        .unwrap()
        .contents
        .animation_progress = 0.9;

    let command = browser.advance_icon_grid_expansion_animation();

    assert!(command.units() > 0);
    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(Path::new("/workspace/root"))
            .unwrap()
            .contents
            .animation_progress,
        1.0,
    );
}

#[test]
fn dropping_icon_state_clears_hidden_selection_and_rename_state() {
    let hidden = PathBuf::from("/workspace/root/hidden.txt");
    let mut browser = browser_with_root(expanded(
        vec![entry("/workspace/root/hidden.txt", FileKind::File)],
        ExpandedDirectoryStatus::Loaded,
        ROOT_GENERATION,
    ));
    browser.selected = Some(hidden.clone());
    browser.selected_paths.insert(hidden.clone());
    browser.selection_anchor = Some(hidden.clone());
    browser.drag_selection_anchor = Some(hidden.clone());
    browser.renaming = Some(hidden.clone());
    browser.pending_created_entry_rename = Some(hidden.clone());
    browser.rename_input = "hidden.txt".to_owned();
    browser.preview = Some(crate::model::PreviewState::Loading(hidden));

    browser.clear_icon_grid_expansion();

    assert!(browser.icon_grid_expansion.is_none());
    assert!(browser.selected_paths.is_empty());
    assert_eq!(browser.selected, None);
    assert_eq!(browser.selection_anchor, None);
    assert_eq!(browser.drag_selection_anchor, None);
    assert_eq!(browser.renaming, None);
    assert_eq!(browser.pending_created_entry_rename, None);
    assert!(browser.rename_input.is_empty());
    assert!(browser.preview.is_none());
}

fn observed_changes(directory: &std::path::Path) -> file_core::DirectoryEntryChanges {
    file_core::DirectoryEntryChanges {
        directory: directory.to_path_buf(),
        changes: vec![file_core::ResolvedEntryChange::RescanRequired],
    }
}
