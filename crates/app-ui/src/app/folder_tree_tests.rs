use std::path::Path;

use super::*;
use crate::config;
use crate::model::BrowserLocation;

/// 主目录是临时目录，里面有 work/code；活动窗格在 work/code。
fn browser_in_home() -> (FileBrowser, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("work/code")).unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.home_dir = home.path().to_path_buf();
    browser.tab_mut().location = BrowserLocation::Directory(home.path().join("work/code"));
    (browser, home)
}

fn watches(browser: &FileBrowser, directory: &Path) -> bool {
    browser.watched_directories().contains(directory)
}

#[test]
fn the_tree_lives_only_while_its_panel_shows() {
    let (mut browser, home) = browser_in_home();
    drop(browser.sync_right_dock_panels());
    assert!(browser.folder_tree.is_none(), "the panel is not shown");
    assert!(!watches(&browser, home.path()));

    drop(browser.toggle_dock_panel(DockPanel::FolderTree));
    drop(browser.sync_right_dock_panels());

    let tree = browser.folder_tree.as_ref().expect("the panel shows");
    assert_eq!(tree.root(), home.path());
    assert_eq!(
        tree.current(),
        Some(home.path().join("work/code").as_path())
    );
    assert!(watches(&browser, home.path()));
    assert!(watches(&browser, &home.path().join("work")));

    drop(browser.toggle_dock_panel(DockPanel::FolderTree));
    drop(browser.sync_right_dock_panels());
    assert!(browser.folder_tree.is_none());
    assert!(!watches(&browser, &home.path().join("work")));
}

#[test]
fn turning_off_the_home_limit_starts_the_tree_at_the_top() {
    let (mut browser, _home) = browser_in_home();
    drop(browser.toggle_dock_panel(DockPanel::FolderTree));
    drop(browser.sync_right_dock_panels());

    drop(browser.handle_folder_tree_message(FolderTreeMessage::LimitToHomeToggled));
    drop(browser.sync_right_dock_panels());

    assert!(!browser.user_config.folder_tree_limit_to_home);
    assert_eq!(browser.folder_tree.as_ref().unwrap().root(), Path::new("/"));
}

#[test]
fn a_search_keeps_the_tree_where_it_was() {
    let (mut browser, home) = browser_in_home();
    drop(browser.toggle_dock_panel(DockPanel::FolderTree));
    drop(browser.sync_right_dock_panels());

    browser.tab_mut().location = BrowserLocation::Trash;
    drop(browser.sync_right_dock_panels());

    let tree = browser.folder_tree.as_ref().unwrap();
    assert_eq!(tree.root(), home.path());
    assert_eq!(
        tree.current(),
        Some(home.path().join("work/code").as_path())
    );
}

#[test]
fn a_level_lists_only_folders_in_name_order() {
    let directory = tempfile::tempdir().unwrap();
    for folder in ["b10", "b9", "A", ".hidden"] {
        std::fs::create_dir(directory.path().join(folder)).unwrap();
    }
    std::fs::write(directory.path().join("file.txt"), b"").unwrap();
    std::os::unix::fs::symlink(directory.path().join("A"), directory.path().join("link")).unwrap();
    std::os::unix::fs::symlink(
        directory.path().join("file.txt"),
        directory.path().join("file-link"),
    )
    .unwrap();

    let names = |include_hidden| {
        read_folders(directory.path(), include_hidden)
            .unwrap()
            .into_iter()
            .map(|folder| folder.name)
            .collect::<Vec<_>>()
    };

    assert_eq!(names(false), ["A", "b9", "b10", "link"]);
    assert_eq!(names(true), [".hidden", "A", "b9", "b10", "link"]);
    assert_eq!(read_folders(&directory.path().join("missing"), false), None);
}
