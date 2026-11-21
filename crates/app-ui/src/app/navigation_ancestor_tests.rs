//! 往上级走：列表、图标里选中刚出来的子文件夹，列表到了滚到它；多栏里那串栏和选中的都留着。

use std::path::{Path, PathBuf};

use super::FileBrowser;
use crate::config;
use crate::model::{BrowserLocation, BrowserViewMode, DirectoryCollectionPhase, NavigationMode};

fn browser_in(directory: &Path, view_mode: BrowserViewMode) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.tab_mut().choose_view_mode(view_mode);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser
}

#[test]
fn going_up_selects_the_folder_just_left() {
    for view_mode in [BrowserViewMode::List, BrowserViewMode::Icons] {
        let mut browser = browser_in(Path::new("/w/a/b"), view_mode);

        drop(browser.navigate_up());

        assert_eq!(browser.tab().current_directory(), Some(Path::new("/w/a")));
        assert_eq!(browser.tab().selected.as_deref(), Some(Path::new("/w/a/b")));
        assert_eq!(
            browser.pending_selection_reveal,
            Some((browser.active_pane_id(), PathBuf::from("/w/a/b")))
        );
    }
}

/// 跳好几级（点面包屑、地址栏）选中途经的那一个；去别处什么都不选。
#[test]
fn a_far_ancestor_selects_the_child_on_the_way_and_elsewhere_selects_nothing() {
    let mut browser = browser_in(Path::new("/w/a/b/c"), BrowserViewMode::List);
    drop(browser.navigate_to(PathBuf::from("/w"), NavigationMode::RecordHistory));
    assert_eq!(browser.tab().selected.as_deref(), Some(Path::new("/w/a")));

    drop(browser.navigate_to(PathBuf::from("/x"), NavigationMode::RecordHistory));
    assert_eq!(browser.tab().selected, None);
    assert_eq!(browser.pending_selection_reveal, None);
}

/// 后退回到上级同样选中刚出来的。
#[test]
fn going_back_to_the_parent_selects_the_folder_just_left() {
    let mut browser = browser_in(Path::new("/w/a"), BrowserViewMode::List);
    drop(browser.navigate_to(PathBuf::from("/w/a/b"), NavigationMode::RecordHistory));

    drop(browser.navigate_back());

    assert_eq!(browser.tab().current_directory(), Some(Path::new("/w/a")));
    assert_eq!(browser.tab().selected.as_deref(), Some(Path::new("/w/a/b")));
}

#[test]
fn going_up_in_columns_keeps_the_open_columns_and_the_selection() {
    let mut browser = browser_in(Path::new("/w/a"), BrowserViewMode::Columns);
    browser.tab_mut().deepest_open_column_directory = Some(PathBuf::from("/w/a/b"));
    browser.select_path(PathBuf::from("/w/a/b/f.txt"));

    drop(browser.navigate_up());

    let tab = browser.tab();
    assert_eq!(tab.current_directory(), Some(Path::new("/w")));
    assert_eq!(
        tab.deepest_open_column_directory.as_deref(),
        Some(Path::new("/w/a/b"))
    );
    assert_eq!(tab.selected.as_deref(), Some(Path::new("/w/a/b/f.txt")));
}

/// 列表到了才知道选中的子文件夹在哪一行：这时滚到它，挂着的等待撤掉。
#[tokio::test]
async fn the_folder_just_left_is_scrolled_to_once_the_list_arrives() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        std::fs::create_dir(temp.path().join(name)).unwrap();
    }
    let mut browser = browser_in(&temp.path().join("b"), BrowserViewMode::List);
    drop(browser.navigate_up());
    let request = browser.next_directory_load_request(temp.path().to_path_buf());
    let scan = file_core::scan_directory(
        temp.path(),
        browser.listing_options(browser.active_pane_id()),
    )
    .await
    .unwrap();

    drop(browser.accept_complete_directory_fixture(request, scan));

    assert_eq!(browser.pending_selection_reveal, None);
    assert_eq!(
        browser.tab().selected.as_deref(),
        Some(temp.path().join("b").as_path())
    );
}
