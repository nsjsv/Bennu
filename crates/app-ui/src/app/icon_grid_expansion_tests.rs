use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};
use iced::{keyboard, Point, Rectangle};

use super::*;
use crate::config;
use crate::model::{
    AddressEditingSession, AddressEditingSessionId, BatchRenameMessage, ColumnEntryBounds,
    DirectoryLoadRequest, SelectionMarquee, SelectionMarqueePhase, SelectionMarqueeScrollAnchor,
    SelectionMarqueeSource,
};
use crate::shortcuts::FileSelectionDirection;

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

fn browser_with_entries(entries: Vec<DirectoryEntry>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = entries.into();
    browser.view_mode = BrowserViewMode::Icons;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser
}

fn anchor(parent: &str, path: &str, index: usize) -> IconGridExpansionAnchor {
    IconGridExpansionAnchor {
        parent_directory: PathBuf::from(parent),
        path: PathBuf::from(path),
        index,
    }
}

fn current_request(browser: &FileBrowser, path: &Path) -> ExpandedDirectoryLoadRequest {
    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("icon grid expansion");
    let expanded = state.directory(path).expect("expanded directory");
    ExpandedDirectoryLoadRequest {
        context: icon_grid_load_context(state.context()),
        path: path.to_path_buf(),
        generation: expanded.contents.load_generation,
    }
}

fn finish_scan(
    browser: &mut FileBrowser,
    path: &Path,
    entries: Vec<DirectoryEntry>,
) -> ExpandedDirectoryLoadRequest {
    let request = current_request(browser, path);
    drop(browser.accept_complete_expanded_directory_fixture(
        request.clone(),
        Ok(DirectoryScan {
            path: path.to_path_buf(),
            entries,
            skipped: Vec::new(),
        }),
    ));
    request
}

fn finish_open_animation(browser: &mut FileBrowser) {
    for _ in 0..8 {
        drop(browser.advance_icon_grid_expansion_animation());
    }
}

#[test]
fn toggle_creates_nonpersistent_icon_grid_request_context() {
    let root = PathBuf::from("/workspace/root");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));

    let state = browser.icon_grid_expansion.as_ref().unwrap();
    assert_eq!(state.root_path(), root);
    assert_eq!(state.context().pane_id, BrowserPaneId::PRIMARY);
    assert_eq!(state.context().current_dir, Path::new("/workspace"));
    let request = current_request(&browser, &root);
    assert!(matches!(
        request.context,
        DirectoryExpansionLoadContext::IconGrid { .. }
    ));
    assert!(browser.expanded_directories.is_empty());
}

#[test]
fn icon_grid_load_rejects_every_stale_identity_dimension() {
    let root = PathBuf::from("/workspace/root");
    let child = entry("/workspace/root/child.txt", FileKind::File);
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    let request = current_request(&browser, &root);
    let context = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .context()
        .clone();

    let stale_requests = [
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::IconGrid {
                pane_id: BrowserPaneId(9),
                current_dir: context.current_dir.clone(),
                session_id: context.session_id,
            },
            ..request.clone()
        },
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::IconGrid {
                pane_id: context.pane_id,
                current_dir: PathBuf::from("/other"),
                session_id: context.session_id,
            },
            ..request.clone()
        },
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::IconGrid {
                pane_id: context.pane_id,
                current_dir: context.current_dir.clone(),
                session_id: IconGridExpansionSessionId::new(999),
            },
            ..request.clone()
        },
        ExpandedDirectoryLoadRequest {
            generation: request.generation.wrapping_add(1),
            ..request.clone()
        },
    ];
    for stale in stale_requests {
        drop(browser.accept_complete_expanded_directory_fixture(
            stale,
            Ok(DirectoryScan {
                path: root.clone(),
                entries: vec![child.clone()],
                skipped: Vec::new(),
            }),
        ));
    }

    let expanded = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .directory(&root)
        .unwrap();
    assert!(matches!(
        expanded.contents.status,
        ExpandedDirectoryStatus::Loading
    ));
    assert!(expanded.contents.entries.is_empty());

    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: root.clone(),
            entries: vec![child.clone()],
            skipped: Vec::new(),
        }),
    ));
    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .contents
            .entries,
        vec![child]
    );
}

#[test]
fn top_level_switch_closes_old_root_before_loading_new_root() {
    let alpha = PathBuf::from("/workspace/alpha");
    let beta = PathBuf::from("/workspace/beta");
    let alpha_child = PathBuf::from("/workspace/alpha/child.txt");
    let mut browser = browser_with_entries(vec![
        entry(alpha.to_str().unwrap(), FileKind::Directory),
        entry(beta.to_str().unwrap(), FileKind::Directory),
    ]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/alpha", 0),
    ));
    finish_scan(
        &mut browser,
        &alpha,
        vec![entry(alpha_child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(alpha_child);
    let old_session = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .context()
        .session_id;

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/beta", 1),
    ));

    let closing = browser.icon_grid_expansion.as_ref().unwrap();
    assert_eq!(closing.root_path(), alpha);
    assert_eq!(closing.pending_root().unwrap().path, beta);
    assert_eq!(browser.selected, Some(alpha.clone()));

    finish_open_animation(&mut browser);
    let replacement = browser.icon_grid_expansion.as_ref().unwrap();
    assert_eq!(replacement.root_path(), beta);
    assert_ne!(replacement.context().session_id, old_session);
    assert!(matches!(
        replacement.directory(&beta).unwrap().contents.status,
        ExpandedDirectoryStatus::Loading
    ));
}

#[test]
fn reopening_parent_restarts_cancelled_loading_descendants() {
    let root = PathBuf::from("/workspace/root");
    let child = PathBuf::from("/workspace/root/child");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::Directory)],
    );
    finish_open_animation(&mut browser);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/child", 0),
    ));

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    let cancelled_generation = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .directory(&child)
        .unwrap()
        .contents
        .load_generation;

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));

    let reloaded = &browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .directory(&child)
        .unwrap()
        .contents;
    assert!(reloaded.load_generation > cancelled_generation);
    assert!(reloaded
        .load_cancel
        .as_ref()
        .is_some_and(|cancellation| !cancellation.is_cancelled()));
}

#[test]
fn arrow_collapse_uses_root_fallback_but_outside_dismissal_does_not() {
    let root = PathBuf::from("/workspace/root");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(child.clone());

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    assert_eq!(browser.selected, Some(root.clone()));

    finish_open_animation(&mut browser);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(child);
    drop(browser.dismiss_icon_grid_expansion_from_outside());
    assert_eq!(browser.selected, None);
    assert!(browser.selected_paths.is_empty());
}

#[test]
fn view_mode_switch_cancels_icon_grid_load_and_keeps_direct_selection_only() {
    let root = PathBuf::from("/workspace/root");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    let cancellation = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .directory(&root)
        .unwrap()
        .contents
        .load_cancel
        .clone()
        .unwrap();
    browser.select_path(root.clone());

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert!(cancellation.is_cancelled());
    assert!(browser.icon_grid_expansion.is_none());
    assert_eq!(browser.selected, Some(root));
}

#[test]
fn icon_grid_panel_selection_scope_is_consistent_across_click_range_and_select_all() {
    let root = PathBuf::from("/workspace/root");
    let outside = PathBuf::from("/workspace/outside.txt");
    let first = PathBuf::from("/workspace/root/first.txt");
    let second = PathBuf::from("/workspace/root/second.txt");
    let mut browser = browser_with_entries(vec![
        entry(root.to_str().unwrap(), FileKind::Directory),
        entry(outside.to_str().unwrap(), FileKind::File),
    ]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![
            entry(first.to_str().unwrap(), FileKind::File),
            entry(second.to_str().unwrap(), FileKind::File),
        ],
    );
    finish_open_animation(&mut browser);

    drop(browser.handle_flat_entry_clicked(first.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_flat_entry_clicked(second.clone()));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([first.clone(), second.clone()])
    );

    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    drop(browser.handle_flat_entry_clicked(root.clone()));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([first.clone(), second.clone(), root.clone()])
    );

    browser.keyboard_modifiers = keyboard::Modifiers::default();
    drop(browser.handle_flat_entry_clicked(first.clone()));
    drop(browser.select_all_in_file_selection_scope());
    assert_eq!(browser.selected_paths, HashSet::from([first, second]));
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| state.root_path() == root));
}

#[test]
fn batch_rename_resolves_selected_entries_from_icon_grid_expansion() {
    let root = PathBuf::from("/workspace/root");
    let first = PathBuf::from("/workspace/root/first.txt");
    let second = PathBuf::from("/workspace/root/second.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![
            entry(first.to_str().unwrap(), FileKind::File),
            entry(second.to_str().unwrap(), FileKind::File),
        ],
    );
    finish_open_animation(&mut browser);
    browser.selected = Some(second.clone());
    browser.selected_paths = HashSet::from([first, second]);

    drop(browser.handle_batch_rename_message(BatchRenameMessage::OpenSelected));

    assert!(browser.batch_rename.is_some());
}

#[test]
fn shift_click_across_icon_grid_panels_selects_only_the_target_panel_entry() {
    let root = PathBuf::from("/workspace/root");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    drop(browser.handle_flat_entry_clicked(child));

    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_flat_entry_clicked(root.clone()));

    assert_eq!(browser.selected_paths, HashSet::from([root.clone()]));
    assert_eq!(browser.selection_anchor, Some(root.clone()));
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| state.root_path() == root));
}

#[test]
fn icon_grid_panel_marquee_ignores_intersecting_entries_from_other_panels() {
    let root = PathBuf::from("/workspace/root");
    let outside = PathBuf::from("/workspace/outside.txt");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser = browser_with_entries(vec![
        entry(root.to_str().unwrap(), FileKind::Directory),
        entry(outside.to_str().unwrap(), FileKind::File),
    ]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.selection_marquee = Some(SelectionMarquee {
        gesture_origin: Point::new(0.0, 0.0),
        start: Point::new(0.0, 0.0),
        current: Point::new(100.0, 100.0),
        source: SelectionMarqueeSource::IconGridPanel {
            directory: root.clone(),
        },
        phase: SelectionMarqueePhase::Selecting,
        scroll_anchor: SelectionMarqueeScrollAnchor::Icons {
            pane_id: BrowserPaneId::PRIMARY,
            offset_y: 0.0,
        },
        base_selection: HashSet::new(),
        preserve_existing: false,
    });
    let bounds = |path: PathBuf| ColumnEntryBounds {
        pane_id: BrowserPaneId::PRIMARY,
        path,
        bounds: Rectangle {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        },
    };

    drop(browser.update_selection_from_column_entry_bounds(vec![
        bounds(outside.clone()),
        bounds(child.clone()),
    ]));

    assert_eq!(browser.selected_paths, HashSet::from([child]));
    assert!(!browser.selected_paths.contains(&outside));
}

#[test]
fn outside_icon_grid_entry_closes_tree_and_cleans_hidden_rename_before_selection() {
    let root = PathBuf::from("/workspace/root");
    let outside = PathBuf::from("/workspace/outside.txt");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser = browser_with_entries(vec![
        entry(root.to_str().unwrap(), FileKind::Directory),
        entry(outside.to_str().unwrap(), FileKind::File),
    ]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(child.clone());
    browser.renaming = Some(child);
    browser.rename_input = "child.txt".to_owned();

    drop(browser.handle_flat_entry_clicked(outside.clone()));

    assert_eq!(browser.selected_paths, HashSet::from([outside.clone()]));
    assert_eq!(browser.selected, Some(outside));
    assert!(browser.renaming.is_none());
    assert_eq!(browser.rename_input, "outside.txt");
    assert!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .contents
            .is_collapsing
    );
}

#[test]
fn icon_grid_keyboard_navigation_crosses_band_and_updates_panel_scope() {
    let root = PathBuf::from("/workspace/root");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.handle_icon_grid_scrolled(BrowserPaneId::PRIMARY, 0.0, 420.0, 120.0));
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(root.clone());

    drop(browser.move_file_selection(FileSelectionDirection::Down));

    assert_eq!(browser.selected, Some(child.clone()));
    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .selection_directory(),
        root
    );
    drop(browser.select_all_in_file_selection_scope());
    assert_eq!(browser.selected_paths, HashSet::from([child]));
}

#[test]
fn icon_grid_nested_entries_resolve_drag_sources_and_drop_directories() {
    let root = PathBuf::from("/workspace/root");
    let file = PathBuf::from("/workspace/root/report.txt");
    let directory = PathBuf::from("/workspace/root/archive");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![
            entry(file.to_str().unwrap(), FileKind::File),
            entry(directory.to_str().unwrap(), FileKind::Directory),
        ],
    );
    finish_open_animation(&mut browser);

    assert_eq!(
        browser.file_drag_release_directory_for_entry(BrowserPaneId::PRIMARY, &file),
        Some(root.clone())
    );
    assert_eq!(
        browser.file_drag_release_directory_for_entry(BrowserPaneId::PRIMARY, &directory),
        Some(directory)
    );
    drop(browser.handle_flat_entry_clicked(file.clone()));
    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.sources.clone()),
        Some(vec![file])
    );
}

#[test]
fn escape_closes_floating_state_before_icon_grid_tree() {
    let root = PathBuf::from("/workspace/root");
    let child = PathBuf::from("/workspace/root/child.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![entry(child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    drop(browser.handle_entry_right_clicked(child.clone()));

    drop(browser.handle_focused_window_escape_pressed());
    assert!(browser.context_menu.is_none());
    assert!(
        !browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .contents
            .is_collapsing
    );

    drop(browser.handle_focused_window_escape_pressed());
    assert_eq!(browser.selected, Some(root.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([root.clone()]));
    assert!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .contents
            .is_collapsing
    );
}

#[test]
fn escape_cancels_address_editing_before_icon_grid_tree() {
    let root = PathBuf::from("/workspace/root");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(&mut browser, &root, Vec::new());
    finish_open_animation(&mut browser);
    browser.address_editing = Some(AddressEditingSession::new(
        BrowserPaneId::PRIMARY,
        AddressEditingSessionId(12),
        Path::new("/workspace"),
    ));

    drop(browser.handle_focused_window_escape_pressed());

    assert!(browser.address_editing.is_none());
    assert!(
        !browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .contents
            .is_collapsing
    );
}

#[test]
fn current_directory_scan_reconciles_anchor_and_drops_removed_root() {
    let root = PathBuf::from("/workspace/root");
    let mut browser = browser_with_entries(vec![
        entry("/workspace/first.txt", FileKind::File),
        entry(root.to_str().unwrap(), FileKind::Directory),
    ]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 1),
    ));
    let generation = browser.directory_load_generation;

    drop(browser.accept_complete_directory_fixture(
        DirectoryLoadRequest {
            pane_id: BrowserPaneId::PRIMARY,
            path: PathBuf::from("/workspace"),
            generation,
        },
        DirectoryScan {
            path: PathBuf::from("/workspace"),
            entries: vec![entry(root.to_str().unwrap(), FileKind::Directory)],
            skipped: Vec::new(),
        },
    ));
    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .unwrap()
            .directory(&root)
            .unwrap()
            .anchor_index,
        0
    );

    drop(browser.accept_complete_directory_fixture(
        DirectoryLoadRequest {
            pane_id: BrowserPaneId::PRIMARY,
            path: PathBuf::from("/workspace"),
            generation,
        },
        DirectoryScan {
            path: PathBuf::from("/workspace"),
            entries: Vec::new(),
            skipped: Vec::new(),
        },
    ));
    assert!(browser.icon_grid_expansion.is_none());
}

#[test]
fn archive_entry_anchors_icon_grid_disclosure() {
    // 压缩包视作目录:点击 disclosure 箭头复用目录锚点校验。
    let archive = PathBuf::from("/workspace/bundle.zip");
    let mut browser = browser_with_entries(vec![entry(archive.to_str().unwrap(), FileKind::File)]);

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/bundle.zip", 0),
    ));

    let state = browser.icon_grid_expansion.as_ref().unwrap();
    assert_eq!(state.root_path(), archive);
    assert!(state.directory(&archive).is_some());
}

#[test]
fn unsupported_file_entry_rejects_icon_grid_anchor() {
    let text = PathBuf::from("/workspace/notes.txt");
    let mut browser = browser_with_entries(vec![entry(text.to_str().unwrap(), FileKind::File)]);

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/notes.txt", 0),
    ));

    assert!(browser.icon_grid_expansion.is_none());
}
