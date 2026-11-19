use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::{Point, Rectangle, Size};

use super::*;
use crate::config;
use crate::model::{
    FileDragHitTestBounds, FileDropLayoutState, SidebarFileDragTargetBounds, SidebarLocation,
};

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
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser
}

fn start_directory_file_drag(browser: &mut FileBrowser, path: &Path) {
    browser.tab_mut().entries = vec![directory_entry(path)].into();
    browser.tab_mut().selected_paths.clear();
    browser.tab_mut().selected_paths.insert(path.to_path_buf());
    browser.cursor_position.set(Some(Point::new(0.0, 0.0)));
    drop(browser.start_file_drag(
        path.to_path_buf(),
        crate::model::FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    ));
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
}

/// 侧栏量出来的样子：「收藏夹」标题 30 + 间距 2，收藏行 32 + 间距 2，末尾空白带 32。
const SECTION_TOP: f32 = 100.0;

fn first_row_top() -> f32 {
    SECTION_TOP + 32.0
}

fn measured_sidebar(favorites: &[&str]) -> FileDragHitTestBounds {
    let row = |index: usize| {
        Rectangle::new(
            Point::new(8.0, first_row_top() + index as f32 * 34.0),
            Size::new(160.0, SIDEBAR_LOCATION_ROW_HEIGHT),
        )
    };
    let section_bottom = first_row_top() + favorites.len() as f32 * 34.0 + 32.0;
    FileDragHitTestBounds {
        sidebar_directories: favorites
            .iter()
            .enumerate()
            .map(|(index, path)| SidebarFileDragTargetBounds {
                directory: PathBuf::from(path),
                favorite_index: Some(index),
                bounds: row(index),
            })
            .collect(),
        sidebar_favorites: Some(Rectangle::new(
            Point::new(8.0, SECTION_TOP),
            Size::new(160.0, section_bottom - SECTION_TOP),
        )),
        ..FileDragHitTestBounds::default()
    }
}

/// 应用内拖放（测试里没有 Wayland）的会话拿到布局测量结果。
fn measure_drop_layout(browser: &mut FileBrowser, measured: FileDragHitTestBounds) {
    let request = match browser
        .file_drop_session
        .as_ref()
        .map(|session| &session.layout)
    {
        Some(FileDropLayoutState::Pending(request)) => *request,
        other => panic!("in-app drag waits for its layout, got {other:?}"),
    };
    drop(browser.accept_drop_layout(request, measured));
}

fn hover_sidebar_at(browser: &mut FileBrowser, y: f32) -> Option<FileDropTarget> {
    browser.cursor_position.set(Some(Point::new(20.0, y)));
    drop(browser.follow_in_app_drag_over_sidebar());
    browser
        .file_drop_session
        .as_ref()
        .and_then(|session| session.hovered_target.clone())
}

fn insert_at(index: usize) -> FileDropTarget {
    FileDropTarget::SidebarBookmarkSlot(SidebarBookmarkDropSlot::Insert { index })
}

#[test]
fn every_point_of_the_favorites_section_has_a_target_in_order() {
    let measured = measured_sidebar(&["/home/user/alpha", "/home/user/beta"]);
    let rows = measured
        .sidebar_directories
        .iter()
        .map(|target| (target.directory.as_path(), target.bounds))
        .collect::<Vec<_>>();
    let section = measured.sidebar_favorites.expect("measured section");

    let mut seen: Vec<FileDropTarget> = Vec::new();
    let mut y = section.y;
    while y <= section.y + section.height {
        let target = favorite_drop_target(rows.iter().copied(), y);
        if seen.last() != Some(&target) {
            seen.push(target);
        }
        y += 0.25;
    }

    assert_eq!(
        seen,
        vec![
            insert_at(0),
            FileDropTarget::Directory(PathBuf::from("/home/user/alpha")),
            insert_at(1),
            FileDropTarget::Directory(PathBuf::from("/home/user/beta")),
            insert_at(2),
        ]
    );
    // 插入线画在两行之间的缝上：对着线松手就插在这里。
    let gap = first_row_top() + SIDEBAR_LOCATION_ROW_HEIGHT + SIDEBAR_ROW_SPACING / 2.0;
    assert_eq!(
        favorite_drop_target(rows.iter().copied(), gap),
        insert_at(1)
    );
}

#[test]
fn an_empty_favorites_section_takes_the_folder_anywhere() {
    let measured = measured_sidebar(&[]);
    let section = measured.sidebar_favorites.expect("measured section");

    for y in [
        section.y,
        section.y + section.height / 2.0,
        section.y + section.height,
    ] {
        assert_eq!(favorite_drop_target([], y), insert_at(0));
    }
}

#[test]
fn in_app_drag_over_the_sidebar_lands_where_native_drag_lands() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    start_directory_file_drag(&mut browser, Path::new("/home/user/projects"));
    measure_drop_layout(
        &mut browser,
        measured_sidebar(&["/home/user/alpha", "/home/user/beta"]),
    );

    let label = SECTION_TOP + 10.0;
    let gap = first_row_top() + SIDEBAR_LOCATION_ROW_HEIGHT + SIDEBAR_ROW_SPACING / 2.0;
    let beta_middle = first_row_top() + 34.0 + SIDEBAR_LOCATION_ROW_HEIGHT / 2.0;
    let below_list = first_row_top() + 2.0 * 34.0 + 16.0;
    assert_eq!(hover_sidebar_at(&mut browser, label), Some(insert_at(0)));
    assert_eq!(
        browser.sidebar_bookmark_drop_slot,
        Some(SidebarBookmarkDropSlot::Insert { index: 0 })
    );
    assert_eq!(hover_sidebar_at(&mut browser, gap), Some(insert_at(1)));
    assert_eq!(
        hover_sidebar_at(&mut browser, beta_middle),
        Some(FileDropTarget::Directory(PathBuf::from("/home/user/beta")))
    );
    assert_eq!(browser.sidebar_bookmark_drop_slot, None);
    assert_eq!(
        hover_sidebar_at(&mut browser, below_list),
        Some(insert_at(2))
    );
}

/// 交给原生拖放以后，侧栏收到的指针移动不改落点位置（原生拖放结束那一刻会来一次）。
#[test]
fn pointer_moves_after_the_native_handoff_leave_the_drop_position_alone() {
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    start_directory_file_drag(&mut browser, Path::new("/home/user/projects"));
    measure_drop_layout(&mut browser, measured_sidebar(&["/home/user/alpha"]));
    let gap = first_row_top() + SIDEBAR_LOCATION_ROW_HEIGHT + SIDEBAR_ROW_SPACING / 2.0;
    assert_eq!(hover_sidebar_at(&mut browser, gap), Some(insert_at(1)));

    let session_id = desktop_linux::WaylandDndController::new()
        .start_file_drag(
            vec![PathBuf::from("/home/user/projects")],
            desktop_linux::WaylandFileDragIcon::new(1, 1, vec![0, 0, 0, 255]).expect("test icon"),
        )
        .expect("source session");
    browser.file_drag.as_mut().expect("file drag").native_dnd =
        crate::model::FileDragNativeDndState::Started(session_id);
    hover_sidebar_at(&mut browser, SECTION_TOP + 10.0);

    let session = browser.file_drop_session.as_ref().expect("drop session");
    assert_eq!(session.position, Some(Point::new(20.0, gap)));
    assert_eq!(session.hovered_target, Some(insert_at(1)));
}

#[test]
fn a_folder_already_in_favorites_drags_like_any_folder() {
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    start_directory_file_drag(&mut browser, Path::new("/home/user/alpha"));

    assert!(!browser.can_drop_sidebar_bookmark());
    measure_drop_layout(&mut browser, measured_sidebar(&["/home/user/alpha"]));
    // 没有插入落点：标题处什么也不是，收藏行只是目录。
    assert_eq!(hover_sidebar_at(&mut browser, SECTION_TOP + 10.0), None);
    assert_eq!(browser.sidebar_bookmark_drop_slot, None);
}

#[test]
fn a_new_folder_can_become_a_favorite_while_dragged() {
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    start_directory_file_drag(&mut browser, Path::new("/home/user/projects"));

    assert!(browser.can_drop_sidebar_bookmark());
}

#[test]
fn dropped_directory_inserts_bookmark_at_slot_index() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");
    browser.tab_mut().entries = vec![directory_entry(&source)].into();

    drop(
        browser
            .insert_sidebar_bookmark(SidebarBookmarkDropSlot::Insert { index: 1 }, source.clone()),
    );

    let favorites = browser.sidebar_favorite_locations();
    assert_eq!(favorites[0].path, PathBuf::from("/home/user/alpha"));
    assert_eq!(favorites[1].path, source);
    assert_eq!(favorites[2].path, PathBuf::from("/home/user/beta"));
}

#[test]
fn duplicate_bookmark_is_not_inserted_again() {
    let existing = PathBuf::from("/home/user/alpha");
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    browser.tab_mut().entries = vec![directory_entry(&existing)].into();

    drop(browser.insert_sidebar_bookmark(SidebarBookmarkDropSlot::Insert { index: 0 }, existing));

    assert_eq!(browser.sidebar_favorite_locations().len(), 1);
}

#[test]
fn add_to_favorites_from_the_menu_appends_the_folder() {
    let mut browser = browser_with_bookmarks(vec![
        bookmark("/home/user/alpha"),
        bookmark("/home/user/beta"),
    ]);
    let source = PathBuf::from("/home/user/projects");

    drop(browser.add_sidebar_bookmark(source.clone()));

    let favorites = browser.sidebar_favorite_locations();
    assert_eq!(favorites.len(), 3);
    assert_eq!(favorites[2].path, source);
    assert_eq!(
        browser.user_config.sidebar_favorites.as_ref().map(Vec::len),
        Some(3)
    );
}

#[test]
fn bookmark_enter_does_not_replace_insert_target_during_file_drag() {
    let mut browser = browser_with_bookmarks(vec![bookmark("/home/user/alpha")]);
    start_directory_file_drag(&mut browser, Path::new("/home/user/projects"));
    measure_drop_layout(&mut browser, measured_sidebar(&["/home/user/alpha"]));
    assert_eq!(
        hover_sidebar_at(&mut browser, first_row_top() + 2.0),
        Some(insert_at(0))
    );

    drop(browser.handle_sidebar_bookmark_entered(PathBuf::from("/home/user/alpha")));

    assert_eq!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.clone()),
        Some(insert_at(0))
    );
}
