use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::Point;

use super::*;
use crate::config;
use crate::model::{FileDropTarget as FileDragTarget, SidebarLocation};

fn bookmark(path: &str) -> SidebarLocation {
    SidebarLocation {
        label: sidebar_bookmark_label(Path::new(path)),
        path: PathBuf::from(path),
        kind: SidebarLocationKind::Bookmark,
    }
}

fn directory_entry(path: &Path) -> DirectoryEntry {
    DirectoryEntry::new(
        path.to_path_buf(),
        FileKind::Directory,
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

fn browser_with_bookmarks(bookmarks: Vec<SidebarLocation>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.sidebar_locations = bookmarks;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser
}

fn start_directory_file_drag(browser: &mut FileBrowser, path: &Path) {
    browser.entries = vec![directory_entry(path)].into();
    browser.selected_paths.clear();
    browser.selected_paths.insert(path.to_path_buf());
    browser.cursor_position = Point::new(0.0, 0.0);
    drop(browser.start_file_drag(
        path.to_path_buf(),
        crate::model::FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    ));
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
}

#[test]
fn bookmark_pointer_top_edge_targets_first_insert_slot() {
    let favorites = vec![bookmark("/home/user/alpha"), bookmark("/home/user/beta")];

    let target =
        sidebar_bookmark_pointer_target(SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT, 0.0, &favorites);

    assert_eq!(
        target,
        SidebarBookmarkPointerTarget::Insert(SidebarBookmarkDropSlot::Insert { index: 0 })
    );
}

#[test]
fn bookmark_pointer_between_rows_targets_insert_slot() {
    let favorites = vec![bookmark("/home/user/alpha"), bookmark("/home/user/beta")];
    let first_row_bottom = SIDEBAR_LOCATION_ROW_HEIGHT;

    let target = sidebar_bookmark_pointer_target(
        first_row_bottom + SIDEBAR_ROW_SPACING / 2.0,
        0.0,
        &favorites,
    );

    assert_eq!(
        target,
        SidebarBookmarkPointerTarget::Insert(SidebarBookmarkDropSlot::Insert { index: 1 })
    );
}

#[test]
fn bookmark_pointer_middle_targets_bookmark_directory() {
    let favorites = vec![bookmark("/home/user/alpha"), bookmark("/home/user/beta")];

    let target =
        sidebar_bookmark_pointer_target(SIDEBAR_LOCATION_ROW_HEIGHT / 2.0, 0.0, &favorites);

    assert_eq!(
        target,
        SidebarBookmarkPointerTarget::Directory(PathBuf::from("/home/user/alpha"))
    );
}

#[test]
fn bookmark_pointer_empty_favorites_targets_first_insert_slot() {
    let target = sidebar_bookmark_pointer_target(0.0, 0.0, &[]);

    assert_eq!(
        target,
        SidebarBookmarkPointerTarget::Insert(SidebarBookmarkDropSlot::Insert { index: 0 })
    );
}

#[test]
fn dropped_directory_inserts_bookmark_at_slot_index() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");
    browser.entries = vec![directory_entry(&source)].into();

    drop(browser.insert_sidebar_bookmark_from_drag(
        SidebarBookmarkDropSlot::Insert { index: 1 },
        source.clone(),
    ));

    let favorites = browser.sidebar_favorite_locations();
    assert_eq!(favorites[0].path, PathBuf::from("/home/user/alpha"));
    assert_eq!(favorites[1].path, source);
    assert_eq!(favorites[2].path, PathBuf::from("/home/user/beta"));
}

#[test]
fn duplicate_bookmark_is_not_inserted_again() {
    let existing = PathBuf::from("/home/user/alpha");
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    browser.entries = vec![directory_entry(&existing)].into();

    drop(
        browser.insert_sidebar_bookmark_from_drag(
            SidebarBookmarkDropSlot::Insert { index: 0 },
            existing,
        ),
    );

    assert_eq!(browser.sidebar_favorite_locations().len(), 1);
}

#[test]
fn sidebar_pointer_middle_sets_directory_drag_target() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");
    start_directory_file_drag(&mut browser, &source);

    drop(browser.update_sidebar_bookmark_drop_slot(Point::new(
        1.0,
        browser.sidebar_favorite_first_row_top() + SIDEBAR_LOCATION_ROW_HEIGHT / 2.0,
    )));

    assert_eq!(browser.sidebar_bookmark_drop_slot, None);
    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.as_ref()),
        Some(FileDragTarget::Directory(path)) if path == Path::new("/home/user/alpha")
    ));
}

#[test]
fn sidebar_pointer_edge_sets_bookmark_insert_target() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");
    start_directory_file_drag(&mut browser, &source);

    drop(browser.update_sidebar_bookmark_drop_slot(Point::new(
        1.0,
        browser.sidebar_favorite_first_row_top() + SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT / 2.0,
    )));

    let slot = SidebarBookmarkDropSlot::Insert { index: 0 };
    assert_eq!(browser.sidebar_bookmark_drop_slot, Some(slot));
    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.as_ref()),
        Some(FileDragTarget::SidebarBookmarkSlot(target)) if *target == slot
    ));
}

#[test]
fn sidebar_pointer_gap_clears_previous_drag_target() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");
    start_directory_file_drag(&mut browser, &source);

    drop(browser.update_sidebar_bookmark_drop_slot(Point::new(
        1.0,
        browser.sidebar_favorite_first_row_top() + SIDEBAR_LOCATION_ROW_HEIGHT / 2.0,
    )));
    drop(browser.update_sidebar_bookmark_drop_slot(Point::new(
        1.0,
        browser.sidebar_favorite_first_row_top() - SIDEBAR_ROW_SPACING * 2.0,
    )));

    assert_eq!(browser.sidebar_bookmark_drop_slot, None);
    assert!(browser
        .file_drop_session
        .as_ref()
        .is_some_and(|session| session.hovered_target.is_none()));
}

#[test]
fn bookmark_enter_does_not_replace_insert_target_during_file_drag() {
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    let source = PathBuf::from("/home/user/projects");
    start_directory_file_drag(&mut browser, &source);
    drop(browser.update_sidebar_bookmark_drop_slot(Point::new(
        1.0,
        browser.sidebar_favorite_first_row_top() + SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT / 2.0,
    )));

    drop(browser.handle_sidebar_bookmark_entered(PathBuf::from("/home/user/alpha")));

    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.as_ref()),
        Some(FileDragTarget::SidebarBookmarkSlot(
            SidebarBookmarkDropSlot::Insert { index: 0 }
        ))
    ));
}
