use std::collections::HashSet;
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::{Point, Rectangle};

use crate::{
    app::FileBrowser,
    config,
    model::{
        AddressEditingSession, AddressEditingSessionId, BrowserPaneId, BrowserViewMode,
        ColumnEntryBounds, ContextMenuState, FileDropTarget as FileDragTarget, ScrollbarRegion,
        SelectionMarquee, SelectionMarqueePhase, SelectionMarqueeScrollAnchor,
        SelectionMarqueeSource,
    },
};

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

fn active_selection_marquee(
    current: Point,
    base_selection: HashSet<PathBuf>,
    preserve_existing: bool,
) -> SelectionMarquee {
    SelectionMarquee {
        gesture_origin: Point::new(0.0, 0.0),
        start: Point::new(0.0, 0.0),
        current,
        source: SelectionMarqueeSource::PaneBlank,
        phase: SelectionMarqueePhase::Selecting,
        scroll_anchor: crate::model::SelectionMarqueeScrollAnchor::List {
            pane_id: BrowserPaneId::PRIMARY,
            offset_y: 0.0,
        },
        base_selection,
        preserve_existing,
    }
}

fn entry_bounds(path: PathBuf, x: f32, y: f32, width: f32, height: f32) -> ColumnEntryBounds {
    ColumnEntryBounds {
        pane_id: BrowserPaneId::PRIMARY,
        path,
        bounds: Rectangle {
            x,
            y,
            width,
            height,
        },
    }
}

#[test]
fn marquee_scroll_anchor_tracks_matching_absolute_offsets_without_drift() {
    let pane_id = BrowserPaneId::PRIMARY;
    let mut marquee = active_selection_marquee(Point::new(40.0, 60.0), HashSet::new(), false);
    marquee.start = Point::new(20.0, 30.0);
    marquee.scroll_anchor = SelectionMarqueeScrollAnchor::List {
        pane_id,
        offset_y: 10.0,
    };

    assert!(marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), 25.0));
    assert_eq!(marquee.start, Point::new(20.0, 15.0));
    assert!(!marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), 25.0));
    assert_eq!(marquee.start, Point::new(20.0, 15.0));
    assert!(marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), 5.0));
    assert_eq!(marquee.start, Point::new(20.0, 35.0));
    assert_eq!(marquee.current, Point::new(40.0, 60.0));
    assert!(marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), -5.0));
    assert_eq!(marquee.start, Point::new(20.0, 40.0));
    assert!(!marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), -10.0));
    assert_eq!(marquee.start, Point::new(20.0, 40.0));
    assert!(!marquee.sync_scroll_offset(&ScrollbarRegion::PaneList(pane_id), f32::NAN));
    assert_eq!(marquee.start, Point::new(20.0, 40.0));
}

#[test]
fn column_marquee_tracks_outer_and_source_column_scroll_only() {
    let pane_id = BrowserPaneId::PRIMARY;
    let directory = PathBuf::from("/workspace/project");
    let mut marquee = active_selection_marquee(Point::new(80.0, 90.0), HashSet::new(), false);
    marquee.start = Point::new(40.0, 50.0);
    marquee.scroll_anchor = SelectionMarqueeScrollAnchor::Column {
        pane_id,
        directory: directory.clone(),
        browser_offset_x: 10.0,
        directory_offset_y: 20.0,
    };

    assert!(marquee.sync_scroll_offset(&ScrollbarRegion::ColumnBrowser(pane_id), 35.0));
    assert_eq!(marquee.start, Point::new(15.0, 50.0));
    assert!(marquee.sync_scroll_offset(
        &ScrollbarRegion::Column {
            pane_id,
            directory: directory.clone(),
        },
        45.0,
    ));
    assert_eq!(marquee.start, Point::new(15.0, 25.0));
    assert!(!marquee.sync_scroll_offset(
        &ScrollbarRegion::Column {
            pane_id,
            directory: PathBuf::from("/workspace/other"),
        },
        100.0,
    ));
    assert!(!marquee.sync_scroll_offset(
        &ScrollbarRegion::Column {
            pane_id: BrowserPaneId(99),
            directory,
        },
        100.0,
    ));
    assert_eq!(marquee.start, Point::new(15.0, 25.0));
}

#[test]
fn selection_marquee_captures_view_specific_scroll_anchor() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    browser.current_dir = current_dir.clone();
    browser.cursor_position = Point::new(30.0, 40.0);
    browser.view_mode = BrowserViewMode::List;
    browser.column_viewports.insert(
        current_dir.clone(),
        crate::thumbnail_cache::ColumnViewport {
            offset_y: 70.0,
            height: 400.0,
        },
    );

    drop(browser.start_selection_marquee());

    assert!(matches!(
        browser
            .selection_marquee
            .as_ref()
            .map(|marquee| &marquee.scroll_anchor),
        Some(SelectionMarqueeScrollAnchor::List {
            pane_id: BrowserPaneId::PRIMARY,
            offset_y,
        }) if *offset_y == 70.0
    ));

    browser.selection_marquee = None;
    browser.view_mode = BrowserViewMode::Columns;
    browser.column_browser_viewport.offset_x = 55.0;
    browser.column_viewports.insert(
        current_dir.clone(),
        crate::thumbnail_cache::ColumnViewport {
            offset_y: 90.0,
            height: 400.0,
        },
    );
    drop(browser.start_column_blank_selection_marquee(current_dir.clone()));

    assert!(matches!(
        browser
            .selection_marquee
            .as_ref()
            .map(|marquee| &marquee.scroll_anchor),
        Some(SelectionMarqueeScrollAnchor::Column {
            pane_id: BrowserPaneId::PRIMARY,
            directory,
            browser_offset_x,
            directory_offset_y,
        }) if directory == &current_dir
            && *browser_offset_x == 55.0
            && *directory_offset_y == 90.0
    ));

    browser.selection_marquee = None;
    browser.view_mode = BrowserViewMode::Icons;
    drop(browser.handle_icon_grid_scrolled(BrowserPaneId::PRIMARY, 120.0, 600.0, 400.0));
    drop(browser.start_selection_marquee());

    assert!(matches!(
        browser
            .selection_marquee
            .as_ref()
            .map(|marquee| &marquee.scroll_anchor),
        Some(SelectionMarqueeScrollAnchor::Icons {
            pane_id: BrowserPaneId::PRIMARY,
            offset_y,
        }) if *offset_y == 120.0
    ));
}

#[test]
fn scroll_motion_preserves_waiting_marquee_phase() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let pane_id = BrowserPaneId::PRIMARY;
    browser.selection_marquee = Some(SelectionMarquee {
        gesture_origin: Point::new(20.0, 30.0),
        start: Point::new(20.0, 30.0),
        current: Point::new(20.0, 30.0),
        source: SelectionMarqueeSource::PaneBlank,
        phase: SelectionMarqueePhase::WaitingForMovement,
        scroll_anchor: SelectionMarqueeScrollAnchor::List {
            pane_id,
            offset_y: 0.0,
        },
        base_selection: HashSet::new(),
        preserve_existing: false,
    });

    drop(browser.update(crate::model::Message::ListScrolled(pane_id, 10.0, 400.0)));

    let marquee = browser
        .selection_marquee
        .as_ref()
        .expect("selection marquee");
    assert_eq!(marquee.phase, SelectionMarqueePhase::WaitingForMovement);
    assert_eq!(marquee.gesture_origin, Point::new(20.0, 30.0));
    assert_eq!(marquee.start, Point::new(20.0, 20.0));
    assert_eq!(marquee.current, Point::new(20.0, 30.0));

    assert!(!browser.update_selection_marquee(Point::new(21.0, 30.0)));
    assert_eq!(
        browser
            .selection_marquee
            .as_ref()
            .expect("selection marquee")
            .phase,
        SelectionMarqueePhase::WaitingForMovement
    );
    assert!(browser.update_selection_marquee(Point::new(30.0, 30.0)));
}

#[test]
fn file_view_scroll_messages_move_the_active_marquee_anchor() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let pane_id = BrowserPaneId::PRIMARY;
    let directory = PathBuf::from("/workspace/project");
    browser.selection_marquee = Some(active_selection_marquee(
        Point::new(80.0, 90.0),
        HashSet::new(),
        false,
    ));
    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee")
        .start = Point::new(40.0, 50.0);

    drop(browser.update(crate::model::Message::ListScrolled(pane_id, 15.0, 400.0)));
    assert_eq!(
        browser
            .selection_marquee
            .as_ref()
            .expect("selection marquee")
            .start,
        Point::new(40.0, 35.0)
    );

    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee")
        .scroll_anchor = SelectionMarqueeScrollAnchor::Icons {
        pane_id,
        offset_y: 5.0,
    };
    drop(browser.update(crate::model::Message::IconGridScrolled(
        pane_id, 25.0, 600.0, 400.0,
    )));
    assert_eq!(
        browser
            .selection_marquee
            .as_ref()
            .expect("selection marquee")
            .start,
        Point::new(40.0, 15.0)
    );

    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee")
        .scroll_anchor = SelectionMarqueeScrollAnchor::Column {
        pane_id,
        directory: directory.clone(),
        browser_offset_x: 10.0,
        directory_offset_y: 20.0,
    };
    drop(browser.update(crate::model::Message::ColumnBrowserScrolled(
        pane_id, 30.0, 600.0,
    )));
    drop(browser.update(crate::model::Message::ColumnScrolled(
        pane_id,
        directory.clone(),
        45.0,
        400.0,
    )));
    assert_eq!(
        browser
            .selection_marquee
            .as_ref()
            .expect("selection marquee")
            .start,
        Point::new(20.0, -10.0)
    );

    drop(browser.update(crate::model::Message::ColumnScrolled(
        pane_id,
        PathBuf::from("/workspace/other"),
        100.0,
        400.0,
    )));
    assert_eq!(
        browser
            .selection_marquee
            .as_ref()
            .expect("selection marquee")
            .start,
        Point::new(20.0, -10.0)
    );
}

#[test]
fn rectangles_intersect_only_when_areas_overlap() {
    let first = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 20.0,
        height: 20.0,
    };

    assert!(super::marquee::rectangles_intersect(
        first,
        Rectangle {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        }
    ));
    assert!(!super::marquee::rectangles_intersect(
        first,
        Rectangle {
            x: 20.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        }
    ));
}

#[test]
fn drag_target_falls_back_from_source_parent_to_pane_directory() {
    let source = PathBuf::from("/right/file.txt");
    let source_parent = PathBuf::from("/right");
    let fallback = PathBuf::from("/left");

    let target = super::resolve_file_drag_target(
        &[source],
        None,
        Some(FileDragTarget::Directory(source_parent)),
        Some(fallback.clone()),
    );

    assert!(matches!(target, Some(FileDragTarget::Directory(path)) if path == fallback));
}

#[test]
fn drag_target_keeps_hovered_directory_in_target_pane() {
    let source = PathBuf::from("/right/file.txt");
    let hovered = PathBuf::from("/left/folder");
    let fallback = PathBuf::from("/left");

    let target = super::resolve_file_drag_target(
        &[source],
        None,
        Some(FileDragTarget::Directory(hovered.clone())),
        Some(fallback),
    );

    assert!(matches!(target, Some(FileDragTarget::Directory(path)) if path == hovered));
}

#[test]
fn drag_target_prefers_release_column_over_stale_hover_target() {
    let source = PathBuf::from("/right/file.txt");
    let stale_hover_target = PathBuf::from("/right");
    let release_column = PathBuf::from("/left/actual-column");
    let fallback = PathBuf::from("/left");

    let target = super::resolve_file_drag_target(
        &[source],
        Some(release_column.clone()),
        Some(FileDragTarget::Directory(stale_hover_target)),
        Some(fallback),
    );

    assert!(matches!(target, Some(FileDragTarget::Directory(path)) if path == release_column));
}

#[test]
fn drag_target_uses_pane_directory_when_hover_target_missing() {
    let source = PathBuf::from("/right/file.txt");
    let fallback = PathBuf::from("/left");

    let target = super::resolve_file_drag_target(&[source], None, None, Some(fallback.clone()));

    assert!(matches!(target, Some(FileDragTarget::Directory(path)) if path == fallback));
}

#[test]
fn marquee_selection_uses_intersecting_bounds() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let inside = current_dir.join("inside.txt");
    let outside = current_dir.join("outside.txt");
    browser.current_dir = current_dir;
    browser.entries = vec![
        test_entry(inside.clone(), FileKind::File),
        test_entry(outside.clone(), FileKind::File),
    ]
    .into();
    browser.selection_marquee = Some(active_selection_marquee(
        Point::new(50.0, 50.0),
        HashSet::new(),
        false,
    ));

    let command = browser.update_selection_from_column_entry_bounds(vec![
        entry_bounds(inside.clone(), 10.0, 10.0, 20.0, 20.0),
        entry_bounds(outside.clone(), 70.0, 70.0, 20.0, 20.0),
    ]);
    drop(command);

    assert!(browser.is_path_selected(&inside));
    assert!(!browser.is_path_selected(&outside));
    assert_eq!(browser.selected.as_ref(), Some(&inside));
}

#[test]
fn marquee_selection_preserves_existing_selection_when_requested() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let preserved = current_dir.join("preserved.txt");
    let added = current_dir.join("added.txt");
    browser.current_dir = current_dir;
    browser.entries = vec![
        test_entry(preserved.clone(), FileKind::File),
        test_entry(added.clone(), FileKind::File),
    ]
    .into();
    browser.selection_marquee = Some(active_selection_marquee(
        Point::new(50.0, 50.0),
        HashSet::from([preserved.clone()]),
        true,
    ));

    let command = browser.update_selection_from_column_entry_bounds(vec![entry_bounds(
        added.clone(),
        10.0,
        10.0,
        20.0,
        20.0,
    )]);
    drop(command);

    assert!(browser.is_path_selected(&preserved));
    assert!(browser.is_path_selected(&added));
}

#[test]
fn clicking_current_column_blank_clears_existing_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(child_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser.selected = Some(second.clone());
    browser.selected_paths = HashSet::from([first, second]);
    browser.selection_anchor = Some(child_directory.clone());

    let command = browser.handle_column_blank_clicked(current_dir.clone());
    drop(command);

    assert!(browser.selected.is_none());
    assert!(browser.selected_paths.is_empty());
    assert!(browser.selection_anchor.is_none());
    assert_eq!(browser.current_dir, current_dir);
}

#[test]
fn clicking_child_column_blank_preserves_open_column_context() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(child_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser.selected = Some(second);
    browser.selected_paths = HashSet::from([first]);
    browser.address_editing = Some(AddressEditingSession::new(
        BrowserPaneId::PRIMARY,
        AddressEditingSessionId(7),
        &current_dir,
    ));
    browser
        .address_editing
        .as_mut()
        .expect("address editing session")
        .draft = "uncommitted draft".to_owned();

    let command = browser.handle_column_blank_clicked(child_directory.clone());
    drop(command);

    assert_eq!(
        browser.deepest_open_column_directory.as_ref(),
        Some(&child_directory)
    );
    assert_eq!(browser.selected.as_ref(), Some(&child_directory));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([child_directory.clone()])
    );
    assert_eq!(browser.selection_anchor.as_ref(), Some(&child_directory));
    assert_eq!(browser.current_dir, current_dir);
    assert_eq!(
        browser
            .address_editing
            .as_ref()
            .map(|session| session.draft.as_str()),
        Some("uncommitted draft")
    );
}

#[test]
fn clicking_column_placeholder_preserves_open_columns_and_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![test_entry(child_directory.clone(), FileKind::Directory)].into();
    browser.deepest_open_column_directory = Some(child_directory.clone());
    browser.selected = Some(child_directory.clone());
    browser.selected_paths = HashSet::from([child_directory.clone()]);
    browser.selection_anchor = Some(child_directory.clone());

    let command = browser.handle_column_placeholder_pressed();
    drop(command);

    assert_eq!(
        browser.deepest_open_column_directory.as_ref(),
        Some(&child_directory)
    );
    assert_eq!(browser.selected.as_ref(), Some(&child_directory));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([child_directory.clone()])
    );
    assert_eq!(browser.selection_anchor.as_ref(), Some(&child_directory));
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![browser.current_dir.clone(), child_directory.clone()]
    );
    assert_eq!(browser.current_dir, current_dir);
}

#[test]
fn pressing_current_column_blank_clears_selection_before_release() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(child_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser.selected = Some(second.clone());
    browser.selected_paths = HashSet::from([first, second]);
    browser.selection_anchor = Some(child_directory.clone());

    let command = browser.start_column_blank_selection_marquee(current_dir.clone());
    drop(command);

    assert!(browser.selected.is_none());
    assert!(browser.selected_paths.is_empty());
    assert!(browser.selection_anchor.is_none());
    assert!(browser.selection_marquee.is_some());
    assert_eq!(browser.current_dir, current_dir);
}

#[test]
fn pressing_child_column_blank_preserves_open_column_before_release() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(child_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser.selected = Some(second);
    browser.selected_paths = HashSet::from([first]);
    browser.deepest_open_column_directory = Some(child_directory.clone());

    let command = browser.start_column_blank_selection_marquee(child_directory.clone());
    drop(command);

    assert_eq!(
        browser.deepest_open_column_directory.as_ref(),
        Some(&child_directory)
    );
    assert_eq!(browser.selected.as_ref(), Some(&child_directory));
    assert_eq!(
        browser.selected_paths,
        HashSet::from([child_directory.clone()])
    );
    assert_eq!(browser.selection_anchor.as_ref(), Some(&child_directory));
    assert!(browser.selection_marquee.is_some());
    assert_eq!(browser.current_dir, current_dir);
}

#[test]
fn dragging_child_column_blank_preserves_open_column_context() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    let child_directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(child_directory.clone(), FileKind::Directory),
    ]
    .into();
    browser.selected = Some(second);
    browser.selected_paths = HashSet::from([first]);
    browser.deepest_open_column_directory = Some(child_directory.clone());

    let press_command = browser.start_column_blank_selection_marquee(child_directory.clone());
    drop(press_command);
    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee starts")
        .phase = SelectionMarqueePhase::Selecting;

    let drag_command = browser.update_selection_from_column_entry_bounds(Vec::new());
    drop(drag_command);

    assert_eq!(
        browser.deepest_open_column_directory.as_ref(),
        Some(&child_directory)
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![browser.current_dir.clone(), child_directory.clone()]
    );
    assert!(browser.selected.is_none());
    assert!(browser.selected_paths.is_empty());
    assert_eq!(browser.current_dir, current_dir);
}

#[test]
fn releasing_selected_item_without_drag_collapses_multi_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let first = current_dir.join("first.txt");
    let second = current_dir.join("second.txt");
    browser.current_dir = current_dir;
    browser.entries = vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
    ]
    .into();
    browser.selected = Some(first.clone());
    browser.selected_paths = HashSet::from([first.clone(), second.clone()]);

    let press_command = browser.handle_column_entry_clicked(second.clone());
    drop(press_command);
    assert_eq!(
        browser.selected_paths,
        HashSet::from([first, second.clone()])
    );

    let release_command = browser.finish_drag_selection(None);
    drop(release_command);

    assert_eq!(browser.selected.as_ref(), Some(&second));
    assert_eq!(browser.selected_paths, HashSet::from([second.clone()]));
    assert_eq!(browser.selection_anchor.as_ref(), Some(&second));
}

#[test]
fn right_clicking_directory_selects_menu_target_without_focusing_it() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let current_dir = PathBuf::from("/workspace");
    let directory = current_dir.join("project");
    browser.current_dir = current_dir.clone();
    browser.entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
    browser.selected = None;
    browser.selected_paths.clear();
    browser.expanded_directories.clear();

    let command = browser.handle_entry_right_clicked(directory.clone());
    drop(command);

    assert_eq!(browser.current_dir, current_dir);
    assert!(browser.selected.is_none());
    assert!(browser.is_path_selected(&directory));
    assert!(!browser.expanded_directories.contains_key(&directory));

    let ContextMenuState::FileArea(context_menu) =
        browser.context_menu.as_ref().expect("context menu opens")
    else {
        panic!("file context menu opens");
    };
    assert_eq!(context_menu.target.as_ref(), Some(&directory));
    assert!(context_menu.target_is_directory);
    assert_eq!(browser.paste_target_directory(), current_dir);
}

#[test]
fn iced_file_drag_keeps_sidebar_trash_non_drop_target() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/report.txt");
    let destination = PathBuf::from("/workspace/destination");
    browser.entries = vec![test_entry(source.clone(), FileKind::File)].into();
    browser.selected_paths.insert(source.clone());
    browser.cursor_position = Point::new(0.0, 0.0);
    browser.start_file_drag(
        source,
        crate::model::FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    );
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));

    drop(browser.handle_sidebar_hovered(destination.clone()));
    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.as_ref()),
        Some(FileDragTarget::Directory(directory)) if directory == &destination
    ));

    drop(browser.handle_sidebar_hovered(crate::model::trash_location_path()));
    assert!(browser
        .file_drop_session
        .as_ref()
        .is_some_and(|session| session.hovered_target.is_none()));
}

#[test]
fn symlink_menu_entry_gates_out_remote_mount_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let mount = PathBuf::from("/run/user/1000/gvfs/mtp:host=phone");
    let remote_file = mount.join("DCIM/photo.jpg");
    let local_file = PathBuf::from("/workspace/report.txt");
    browser.current_dir = mount.clone();
    browser.entries = vec![
        test_entry(remote_file.clone(), FileKind::File),
        test_entry(local_file.clone(), FileKind::File),
    ]
    .into();

    let connection = desktop_linux::NetworkConnection::new(
        desktop_linux::NetworkConnectionId::new("phone"),
        "Phone",
        desktop_linux::NetworkProtocol::Smb,
        "smb://phone/share",
    )
    .unwrap();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    browser
        .network_connections
        .accept_loaded(vec![(id, desktop_linux::NetworkMountState::Mounted(mount))]);

    // 远程选中:整份选中按远程处理,不提供创建符号链接。
    browser.selected_paths = std::iter::once(remote_file.clone()).collect();
    browser.selected = Some(remote_file.clone());
    drop(browser.handle_entry_right_clicked(remote_file.clone()));
    let ContextMenuState::FileArea(remote_menu) =
        browser.context_menu.as_ref().expect("context menu opens")
    else {
        panic!("file context menu opens");
    };
    assert!(!remote_menu.can_create_symlink);

    // 本地选中不受影响。
    browser.selected_paths = std::iter::once(local_file.clone()).collect();
    browser.selected = Some(local_file.clone());
    drop(browser.handle_entry_right_clicked(local_file.clone()));
    let ContextMenuState::FileArea(local_menu) =
        browser.context_menu.as_ref().expect("context menu opens")
    else {
        panic!("file context menu opens");
    };
    assert!(local_menu.can_create_symlink);
}
