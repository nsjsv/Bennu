use std::collections::HashSet;
use std::path::PathBuf;

use desktop_linux::WaylandDndWindowHandle;
use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};
use iced::{keyboard, window, Point};

use crate::app::FileBrowser;
use crate::config;
use crate::model::{
    trash_location_path, BrowserPaneId, BrowserViewMode, DirectoryExpansionLoadContext,
    ExpandedDirectory, ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus,
    FileDragNativeDndState, FileDragPhase, Message,
};
use crate::shortcuts::FileSelectionDirection;

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

fn browser_with_entries(paths: &[PathBuf]) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.entries = paths
        .iter()
        .cloned()
        .map(|path| test_entry(path, FileKind::File))
        .collect::<Vec<_>>()
        .into();
    browser
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

#[test]
fn pending_column_drag_preserves_the_press_time_target_columns() {
    let root = PathBuf::from("/workspace");
    let source = root.join("report.txt");
    let target_directory = root.join("downloads");
    let mut browser = browser_with_entries(&[]);
    browser.entries = vec![
        test_entry(source.clone(), FileKind::File),
        test_entry(target_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser
        .expanded_directories
        .insert(target_directory.clone(), loaded_directory(Vec::new()));
    browser.deepest_open_column_directory = Some(target_directory.clone());
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root.clone(), target_directory.clone()]
    );

    drop(browser.handle_column_entry_clicked(source));

    assert!(matches!(
        browser.file_drag.as_ref().map(|drag| &drag.phase),
        Some(FileDragPhase::WaitingForMovement { .. })
    ));
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root, target_directory]
    );
}

#[test]
fn shift_range_click_does_not_seed_activation_double_click() {
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);

    drop(browser.handle_column_entry_clicked(first.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(second.clone()));

    assert!(browser.last_activation_click.is_none());
    assert_eq!(
        browser.selected_paths,
        HashSet::from([first.clone(), second.clone()])
    );

    browser.keyboard_modifiers = keyboard::Modifiers::default();
    drop(browser.handle_column_entry_clicked(second.clone()));

    let file_drag = browser
        .file_drag
        .as_ref()
        .expect("plain click after range selection starts file drag");
    assert_eq!(file_drag.sources, vec![first, second]);
    assert!(matches!(
        file_drag.phase,
        FileDragPhase::WaitingForMovement { .. }
    ));

    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
    assert!(browser
        .file_drag
        .as_ref()
        .is_some_and(|file_drag| file_drag.is_dragging()));
    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(FileDragNativeDndState::NotRequested)
    );
}

#[test]
fn file_drag_stays_in_app_until_cursor_leaves() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.accept_wayland_dnd_handle(Ok(Some(WaylandDndWindowHandle::new(1, 2)))));

    drop(browser.handle_column_entry_clicked(source));
    drop(browser.update_file_drag(Point::new(2.0, 0.0)));
    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(FileDragNativeDndState::NotRequested)
    );

    // 激活不再立即请求原生拖放:窗口内保持应用内拖拽,滚轮/shift 滚轮/
    // 边缘自动滚才有输入可用。
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(FileDragNativeDndState::NotRequested)
    );

    // 离开主窗口才把拖放交给合成器(跨窗口拖放)。
    drop(browser.update(Message::CursorLeft {
        window: browser.main_window,
    }));
    assert!(matches!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(FileDragNativeDndState::Requested(_))
    ));
}

#[test]
fn outside_cursor_move_hands_drag_to_native() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.accept_wayland_dnd_handle(Ok(Some(WaylandDndWindowHandle::new(1, 2)))));

    drop(browser.handle_column_entry_clicked(source));
    drop(browser.update_file_drag(Point::new(10.0, 12.0)));
    // 拖动中(隐式 grab)越界后收不到 CursorLeft,交接只能按坐标判定。
    drop(browser.update_file_drag(Point::new(browser.main_window_width + 1.0, 12.0)));

    let file_drag = browser
        .file_drag
        .as_ref()
        .expect("file drag remains active");
    assert!(file_drag.is_dragging());
    assert!(matches!(
        file_drag.native_dnd,
        FileDragNativeDndState::Requested(_)
    ));
}

#[test]
fn fling_past_window_edge_hands_drag_to_native_in_one_move() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.accept_wayland_dnd_handle(Ok(Some(WaylandDndWindowHandle::new(1, 2)))));

    drop(browser.handle_column_entry_clicked(source));
    // 一步从按压点甩出窗口:激活与交接必须同时发生。
    drop(browser.update_file_drag(Point::new(browser.main_window_width + 1.0, 12.0)));

    let file_drag = browser
        .file_drag
        .as_ref()
        .expect("file drag remains active");
    assert!(file_drag.is_dragging());
    assert!(matches!(
        file_drag.native_dnd,
        FileDragNativeDndState::Requested(_)
    ));
}

#[test]
fn iced_release_does_not_consume_requested_wayland_source() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.accept_wayland_dnd_handle(Ok(Some(WaylandDndWindowHandle::new(1, 2)))));

    drop(browser.handle_column_entry_clicked(source));
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
    drop(browser.update(Message::CursorLeft {
        window: browser.main_window,
    }));
    let requested = browser
        .file_drag
        .as_ref()
        .map(|drag| drag.native_dnd)
        .expect("requested Wayland source");

    drop(browser.finish_drag_selection(None));

    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(requested)
    );
}

#[test]
fn auxiliary_window_cursor_move_does_not_request_wayland_dnd() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.accept_wayland_dnd_handle(Ok(Some(WaylandDndWindowHandle::new(1, 2)))));

    drop(browser.handle_column_entry_clicked(source));
    drop(browser.update(Message::CursorMoved {
        window: window::Id::unique(),
        position: Point::new(browser.main_window_width + 1.0, 12.0),
    }));

    assert_eq!(
        browser.file_drag.as_ref().map(|drag| drag.native_dnd),
        Some(FileDragNativeDndState::NotRequested)
    );
}

#[test]
fn control_selection_click_does_not_seed_activation_double_click() {
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);

    drop(browser.handle_column_entry_clicked(first.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    drop(browser.handle_column_entry_clicked(second.clone()));

    assert!(browser.last_activation_click.is_none());
    assert_eq!(
        browser.selected_paths,
        HashSet::from([first, second.clone()])
    );

    browser.keyboard_modifiers = keyboard::Modifiers::default();
    drop(browser.handle_column_entry_clicked(second));

    assert!(browser.file_drag.is_some());
}

#[test]
fn plain_double_click_still_activates_directory() {
    let directory = PathBuf::from("/workspace/project");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();

    drop(browser.handle_column_entry_clicked(directory.clone()));
    drop(browser.handle_column_entry_clicked(directory.clone()));

    assert_eq!(browser.current_dir, directory);
    assert!(matches!(browser.file_drag, None));
    assert!(browser.last_activation_click.is_none());
    assert_eq!(browser.back_stack, vec![PathBuf::from("/workspace")]);
    assert!(matches!(browser.is_trash_view, false));
}

#[test]
fn list_single_click_selects_directory_without_expanding_or_opening_column() {
    let directory = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/main.rs");
    let mut browser = browser_with_entries(&[directory.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        directory.clone(),
        ExpandedDirectory {
            entries: vec![test_entry(child, FileKind::File)],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: false,
            is_collapsing: false,
            animation_progress: 0.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );

    drop(browser.handle_flat_entry_clicked(directory.clone()));

    assert_eq!(browser.selected, Some(directory.clone()));
    assert_eq!(browser.current_dir, PathBuf::from("/workspace"));
    assert_eq!(browser.deepest_open_column_directory, None);
    assert!(browser
        .expanded_directories
        .get(&directory)
        .is_some_and(|expanded| !expanded.is_expanded));
}

#[test]
fn list_double_click_activates_directory() {
    let directory = PathBuf::from("/workspace/project");
    let mut browser = browser_with_entries(&[directory.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();

    drop(browser.handle_flat_entry_clicked(directory.clone()));
    drop(browser.handle_flat_entry_clicked(directory.clone()));

    assert_eq!(browser.current_dir, directory);
    assert_eq!(browser.back_stack, vec![PathBuf::from("/workspace")]);
}

#[test]
fn list_double_click_keeps_visible_placeholders_while_canonical_entries_load() {
    let directory = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/main.rs");
    let mut browser = browser_with_entries(&[directory.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        directory.clone(),
        loaded_directory(vec![test_entry(child.clone(), FileKind::File)]),
    );
    browser.select_path(child.clone());

    drop(browser.handle_flat_entry_clicked(directory.clone()));
    drop(browser.handle_flat_entry_clicked(directory.clone()));

    assert_eq!(browser.current_dir, directory.clone());
    assert!(browser.directory_collection_phase.is_discovering());
    assert!(browser.entries.is_empty());
    assert!(browser.expanded_directories.is_empty());
    assert_eq!(browser.selected, None);
    assert!(browser.selected_paths.is_empty());
    let placeholder = browser
        .directory_loading_placeholder
        .as_ref()
        .expect("list loading placeholder");
    assert_eq!(placeholder.entries.len(), 2);
    assert_eq!(placeholder.entries[0].entry.path, directory);
    assert_eq!(placeholder.entries[0].depth, 0);
    assert_eq!(placeholder.entries[0].row_index, 0);
    assert_eq!(placeholder.entries[1].entry.path, child);
    assert_eq!(placeholder.entries[1].depth, 1);
    assert_eq!(placeholder.entries[1].row_index, 1);
    assert_eq!(placeholder.before_height, 0.0);
    assert_eq!(placeholder.after_height, 0.0);
}

#[test]
fn column_single_click_still_opens_child_column() {
    let directory = PathBuf::from("/workspace/project");
    let mut browser = browser_with_entries(&[directory.clone()]);
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();

    drop(browser.handle_column_entry_clicked(directory.clone()));

    assert_eq!(browser.current_dir, PathBuf::from("/workspace"));
    assert_eq!(browser.deepest_open_column_directory, None);
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![PathBuf::from("/workspace")]
    );

    drop(browser.finish_drag_selection(None));

    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![PathBuf::from("/workspace"), directory.clone()]
    );
    assert!(browser
        .expanded_directories
        .get(&directory)
        .is_some_and(|expanded| expanded.is_expanded));
}

#[test]
fn column_single_click_requests_session_save_when_enabled() {
    let directory = PathBuf::from("/workspace/project");
    let mut user_config = config::default_user_config();
    user_config.startup_location_policy = config::StartupLocationPolicy::PreviousSession;
    user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
    let (mut browser, _) = FileBrowser::new(user_config);
    browser.current_dir = PathBuf::from("/workspace");
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.entries = vec![test_entry(directory, FileKind::Directory)].into();

    drop(browser.handle_column_entry_clicked(PathBuf::from("/workspace/project")));

    assert!(browser.pending_browser_session_save);
}

#[test]
fn right_arrow_enters_loaded_child_column_and_remembers_return_target() {
    let parent = PathBuf::from("/workspace/project");
    let first_child = PathBuf::from("/workspace/project/a.txt");
    let return_child = PathBuf::from("/workspace/project/notes.txt");
    let mut browser = browser_with_entries(&[parent.clone()]);
    browser.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![
            test_entry(first_child.clone(), FileKind::File),
            test_entry(return_child.clone(), FileKind::File),
        ]),
    );
    browser.deepest_open_column_directory = Some(parent.clone());

    browser.select_path(return_child.clone());
    drop(browser.move_file_selection(FileSelectionDirection::Left));

    assert_eq!(browser.selected, Some(parent.clone()));
    assert_eq!(browser.deepest_open_column_directory, Some(parent.clone()));

    drop(browser.move_file_selection(FileSelectionDirection::Right));

    assert_eq!(browser.selected, Some(return_child));
    assert_ne!(browser.selected, Some(first_child));
    assert_eq!(browser.deepest_open_column_directory, Some(parent));
}

#[test]
fn vertical_arrows_open_directory_contents_without_entering_child_column() {
    let first_parent = PathBuf::from("/workspace/a");
    let second_parent = PathBuf::from("/workspace/b");
    let second_child = PathBuf::from("/workspace/b/child.txt");
    let mut browser = browser_with_entries(&[first_parent.clone(), second_parent.clone()]);
    browser.entries = vec![
        test_entry(first_parent.clone(), FileKind::Directory),
        test_entry(second_parent.clone(), FileKind::Directory),
    ]
    .into();
    browser.expanded_directories.insert(
        second_parent.clone(),
        loaded_directory(vec![test_entry(second_child.clone(), FileKind::File)]),
    );
    browser.select_path(first_parent);

    drop(browser.move_file_selection(FileSelectionDirection::Down));

    assert_eq!(browser.selected, Some(second_parent.clone()));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([second_parent.clone()])
    );
    assert_eq!(browser.deepest_open_column_directory, Some(second_parent));
    assert_ne!(browser.selected, Some(second_child));
}

#[test]
fn right_arrow_focuses_first_child_after_directory_loads() {
    let parent = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/first.txt");
    let mut browser = browser_with_entries(&[parent.clone()]);
    browser.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    browser.select_path(parent.clone());

    drop(browser.move_file_selection(FileSelectionDirection::Right));

    assert_eq!(browser.selected, Some(parent.clone()));
    assert!(browser.pending_keyboard_column_focus.is_some());
    let load_generation = browser
        .expanded_directories
        .get(&parent)
        .map(|expanded| expanded.load_generation)
        .unwrap_or_default();

    drop(browser.accept_complete_expanded_directory_fixture(
        ExpandedDirectoryLoadRequest {
            context: DirectoryExpansionLoadContext::BrowserTree {
                pane_id: BrowserPaneId::PRIMARY,
            },
            path: parent.clone(),
            generation: load_generation,
        },
        Ok(DirectoryScan {
            path: parent.clone(),
            entries: vec![test_entry(child.clone(), FileKind::File)],
            skipped: Vec::new(),
        }),
    ));

    assert_eq!(browser.selected, Some(child));
    assert!(browser.pending_keyboard_column_focus.is_none());
    assert_eq!(browser.deepest_open_column_directory, Some(parent));
}

#[test]
fn list_vertical_arrows_move_across_expanded_directory_levels() {
    let parent = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/main.rs");
    let sibling = PathBuf::from("/workspace/readme.txt");
    let mut browser = browser_with_entries(&[parent.clone(), sibling.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![
        test_entry(parent.clone(), FileKind::Directory),
        test_entry(sibling.clone(), FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![test_entry(child.clone(), FileKind::File)]),
    );
    browser.select_path(parent.clone());

    drop(browser.move_file_selection(FileSelectionDirection::Down));
    assert_eq!(browser.selected, Some(child.clone()));

    drop(browser.move_file_selection(FileSelectionDirection::Down));
    assert_eq!(browser.selected, Some(sibling));

    drop(browser.move_file_selection(FileSelectionDirection::Up));
    assert_eq!(browser.selected, Some(child));
}

#[test]
fn list_right_arrow_expands_then_focuses_first_child() {
    let parent = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/main.rs");
    let mut browser = browser_with_entries(&[parent.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        parent.clone(),
        ExpandedDirectory {
            entries: vec![test_entry(child.clone(), FileKind::File)],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: false,
            is_collapsing: false,
            animation_progress: 0.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );
    browser.select_path(parent.clone());

    drop(browser.move_file_selection(FileSelectionDirection::Right));

    assert_eq!(browser.selected, Some(parent.clone()));
    assert!(browser
        .expanded_directories
        .get(&parent)
        .is_some_and(|expanded| expanded.is_expanded));

    drop(browser.move_file_selection(FileSelectionDirection::Right));

    assert_eq!(browser.selected, Some(child));
}

#[test]
fn list_left_arrow_collapses_then_focuses_visible_parent() {
    let parent = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/src");
    let grandchild = PathBuf::from("/workspace/project/src/main.rs");
    let mut browser = browser_with_entries(&[parent.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![test_entry(parent.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![test_entry(child.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        child.clone(),
        loaded_directory(vec![test_entry(grandchild, FileKind::File)]),
    );
    browser.select_path(child.clone());

    drop(browser.move_file_selection(FileSelectionDirection::Left));

    assert_eq!(browser.selected, Some(child.clone()));
    assert!(browser
        .expanded_directories
        .get(&child)
        .is_some_and(|expanded| expanded.is_expanded && expanded.is_collapsing));

    for _ in 0..8 {
        drop(browser.advance_list_directory_animations());
    }

    assert!(browser
        .expanded_directories
        .get(&child)
        .is_some_and(|expanded| !expanded.is_expanded && !expanded.is_collapsing));

    drop(browser.move_file_selection(FileSelectionDirection::Left));

    assert_eq!(browser.selected, Some(parent));
}

#[test]
fn list_select_all_and_range_use_the_same_visible_rows() {
    let parent = PathBuf::from("/workspace/project");
    let child = PathBuf::from("/workspace/project/main.rs");
    let sibling = PathBuf::from("/workspace/readme.txt");
    let mut browser = browser_with_entries(&[parent.clone(), sibling.clone()]);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![
        test_entry(parent.clone(), FileKind::Directory),
        test_entry(sibling.clone(), FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![test_entry(child.clone(), FileKind::File)]),
    );

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(
        browser.selected_paths,
        HashSet::from([parent.clone(), child.clone(), sibling.clone()])
    );

    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(sibling));

    assert_eq!(
        browser.selected_paths,
        HashSet::from([parent, child, PathBuf::from("/workspace/readme.txt")])
    );
}

#[test]
fn column_shift_range_stays_in_the_clicked_column() {
    let project = PathBuf::from("/workspace/project");
    let alpha = PathBuf::from("/workspace/alpha.txt");
    let zeta = PathBuf::from("/workspace/zeta.txt");
    let child = project.join("main.rs");
    let mut browser = browser_with_entries(&[alpha.clone(), zeta.clone()]);
    browser.view_mode = BrowserViewMode::Columns;
    browser.entries = vec![
        test_entry(alpha.clone(), FileKind::File),
        test_entry(project.clone(), FileKind::Directory),
        test_entry(zeta.clone(), FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(child.clone(), FileKind::File)]),
    );

    drop(browser.handle_column_entry_clicked(alpha.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(zeta.clone()));

    assert_eq!(
        browser.selected_paths,
        HashSet::from([alpha, project, zeta])
    );
    assert!(!browser.selected_paths.contains(&child));
}

#[test]
fn column_shift_click_in_another_column_selects_only_target_and_reanchors() {
    let project = PathBuf::from("/workspace/project");
    let alpha = PathBuf::from("/workspace/alpha.txt");
    let child = project.join("main.rs");
    let sibling = project.join("util.rs");
    let mut browser = browser_with_entries(&[alpha.clone()]);
    browser.view_mode = BrowserViewMode::Columns;
    browser.entries = vec![
        test_entry(alpha.clone(), FileKind::File),
        test_entry(project.clone(), FileKind::Directory),
    ]
    .into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![
            test_entry(child.clone(), FileKind::File),
            test_entry(sibling.clone(), FileKind::File),
        ]),
    );

    drop(browser.handle_column_entry_clicked(alpha.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(child.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([child.clone()]));
    assert_eq!(browser.selection_anchor, Some(child.clone()));

    drop(browser.handle_column_entry_clicked(sibling.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([child, sibling]));
}

#[test]
fn trash_shift_range_spans_entries_across_original_parents() {
    let first = PathBuf::from("/origin-a/first.txt");
    let second = PathBuf::from("/origin-b/nested/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);
    browser.view_mode = BrowserViewMode::Columns;
    browser.current_dir = trash_location_path();
    browser.is_trash_view = true;

    drop(browser.handle_column_entry_clicked(first.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(second.clone()));

    assert_eq!(browser.selected_paths, HashSet::from([first, second]));
}

#[test]
fn icon_grid_select_all_and_range_only_use_direct_entries() {
    let parent = PathBuf::from("/workspace/project");
    let hidden_child = parent.join("main.rs");
    let sibling = PathBuf::from("/workspace/readme.txt");
    let mut browser = browser_with_entries(&[parent.clone(), sibling.clone()]);
    browser.view_mode = BrowserViewMode::Icons;
    browser.entries = vec![
        test_entry(parent.clone(), FileKind::Directory),
        test_entry(sibling.clone(), FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        parent.clone(),
        loaded_directory(vec![test_entry(hidden_child, FileKind::File)]),
    );

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(
        browser.selected_paths,
        HashSet::from([parent.clone(), sibling.clone()])
    );

    drop(browser.handle_flat_entry_clicked(parent.clone()));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_flat_entry_clicked(sibling.clone()));

    assert_eq!(browser.selected_paths, HashSet::from([parent, sibling]));
}

#[test]
fn clicking_child_column_blank_keeps_select_all_focused_on_that_column() {
    let project = PathBuf::from("/workspace/project");
    let source = project.join("src");
    let first_child = source.join("main.rs");
    let second_child = source.join("lib.rs");
    let mut browser = browser_with_entries(&[]);
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(source.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        source.clone(),
        loaded_directory(vec![
            test_entry(first_child.clone(), FileKind::File),
            test_entry(second_child.clone(), FileKind::File),
        ]),
    );
    browser.deepest_open_column_directory = Some(source.clone());
    browser.selected = Some(source.clone());
    browser.selected_paths = HashSet::from([source.clone()]);
    browser.cursor_paste_directory = Some(source.clone());

    drop(browser.start_column_blank_selection_marquee(source.clone()));
    drop(browser.finish_drag_selection(None));
    assert!(browser.cursor_paste_directory.is_none());

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(
        browser.selected_paths,
        HashSet::from([first_child.clone(), second_child])
    );
    assert_eq!(browser.selected, Some(first_child));
    assert_eq!(browser.deepest_open_column_directory, Some(source));
}

#[test]
fn column_select_all_uses_hovered_column_directory() {
    let project = PathBuf::from("/workspace/project");
    let sibling = PathBuf::from("/workspace/readme.txt");
    let first_child = PathBuf::from("/workspace/project/a.txt");
    let second_child = PathBuf::from("/workspace/project/b.txt");
    let mut browser = browser_with_entries(&[project.clone(), sibling.clone()]);
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(sibling, FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![
            test_entry(first_child.clone(), FileKind::File),
            test_entry(second_child.clone(), FileKind::File),
        ]),
    );
    browser.deepest_open_column_directory = Some(project.clone());
    browser.cursor_paste_directory = Some(project.clone());

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(
        browser.selected_paths,
        HashSet::from([first_child.clone(), second_child])
    );
    assert_eq!(browser.selected, Some(first_child));
    assert_eq!(browser.deepest_open_column_directory, Some(project));
}

#[test]
fn column_select_all_hovered_directory_entry_stays_in_entry_column() {
    let project = PathBuf::from("/workspace/project");
    let sibling = PathBuf::from("/workspace/readme.txt");
    let child = PathBuf::from("/workspace/project/main.rs");
    let mut browser = browser_with_entries(&[project.clone(), sibling.clone()]);
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(sibling.clone(), FileKind::File),
    ]
    .into();
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(child, FileKind::File)]),
    );
    browser.deepest_open_column_directory = Some(project.clone());
    browser.hovered_entry = Some(project.clone());
    browser.cursor_paste_directory = Some(project.clone());

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(browser.selected_paths, HashSet::from([project, sibling]));
}

#[test]
fn focused_preview_window_blocks_file_select_all_fallback() {
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    let mut browser = browser_with_entries(&[first, second]);
    let preview_window = window::Id::unique();
    browser.preview_window = Some(preview_window);
    browser.focused_window = preview_window;

    drop(browser.select_all_in_file_selection_scope());

    assert!(browser.selected_paths.is_empty());
    assert!(browser.selected.is_none());
}

#[test]
fn trash_select_all_ignores_hovered_entry_real_parent() {
    let first = PathBuf::from("/home/user/.local/share/Trash/files/first.txt");
    let second = PathBuf::from("/home/user/.local/share/Trash/files/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);
    browser.current_dir = trash_location_path();
    browser.is_trash_view = true;
    browser.view_mode = BrowserViewMode::Columns;
    browser.hovered_entry = Some(first.clone());

    drop(browser.select_all_in_file_selection_scope());

    assert_eq!(
        browser.selected_paths,
        HashSet::from([first.clone(), second])
    );
    assert_eq!(browser.selected, Some(first));
}

#[test]
fn trash_plain_click_after_select_all_focuses_single_entry() {
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);
    browser.is_trash_view = true;

    drop(browser.select_all_in_file_selection_scope());
    drop(browser.handle_flat_entry_clicked(second.clone()));

    assert_eq!(browser.selected, Some(second.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([second]));
}

fn active_drag_browser(origin: Point) -> FileBrowser {
    let mut browser = browser_with_entries(&[]);
    browser.cursor_position = origin;
    browser.file_drag = Some(crate::model::FileDragState {
        gesture_id: crate::model::FileDragGestureId(1),
        source_pane_id: browser.active_pane_id(),
        source_tab_id: browser.active_tab_id,
        sources: vec![PathBuf::from("/workspace/report.txt")],
        pressed_path: PathBuf::from("/workspace/report.txt"),
        bookmark_source: None,
        stationary_action: crate::model::FileDragStationaryAction::SelectionOnly,
        phase: FileDragPhase::WaitingForMovement { origin },
        native_dnd: FileDragNativeDndState::NotRequested,
        column_directories_snapshot: Vec::new(),
        press_origin: iced::Point::ORIGIN,
        preview_entries: Vec::new(),
    });
    browser
}

#[test]
fn file_drag_activation_stays_in_app_until_cursor_leaves() {
    let origin = Point::new(100.0, 100.0);
    let mut browser = active_drag_browser(origin);

    drop(browser.update_file_drag(Point::new(104.0, 100.0)));

    let file_drag = browser.file_drag.as_ref().expect("drag survives activation");
    assert!(matches!(file_drag.phase, FileDragPhase::Dragging));
    // 窗口内必须保持应用内拖拽(原生 dnd 未请求),滚轮/shift 滚轮/
    // 边缘自动滚才有输入可用;离开窗口时才交给合成器。
    assert_eq!(file_drag.native_dnd, FileDragNativeDndState::NotRequested);
    assert!(browser.file_drop_session.is_some());
}

#[test]
fn cursor_leaving_hands_drag_to_native_session() {
    let origin = Point::new(100.0, 100.0);
    let mut browser = active_drag_browser(origin);
    drop(browser.update_file_drag(Point::new(104.0, 100.0)));
    let top_edge = Point::new(
        browser.sidebar_width + 20.0,
        browser.main_panes_area_top() + 5.0,
    );
    browser.update_file_drag_edge_scroll(top_edge);
    assert!(browser.file_drag_edge_scroll.is_some());

    drop(browser.start_native_file_drag_for_cursor_left());

    // 无 wayland dnd 运行时的测试环境走 Unavailable:拖拽仍保持应用内,
    // 且边缘自动滚计划必须清掉,防止合成器接管后残留滚动。
    assert_eq!(
        browser
            .file_drag
            .as_ref()
            .expect("drag survives handoff failure")
            .native_dnd,
        FileDragNativeDndState::NotRequested
    );
    assert!(browser.file_drag_edge_scroll.is_none());
}
