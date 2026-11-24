use std::collections::HashSet;
use std::path::PathBuf;

use super::*;
use crate::config;
use crate::model::BrowserLocation;

/// 停靠区显示着（默认布局里信息面板是当前标签），活动窗格在 /workspace。
fn browser() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.right_dock.toggle_visible();
    browser
}

fn select(browser: &mut FileBrowser, paths: &[&str]) {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    *browser.tab_mut().selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
    browser.tab_mut().selected = paths.last().cloned();
}

fn snapshot_of(path: &str) -> FilePropertiesSnapshot {
    FilePropertiesSnapshot {
        name: PathBuf::from(path).file_name().unwrap().to_owned(),
        location: PathBuf::from("/workspace"),
        created: None,
        modified: None,
        accessed: None,
        changed: None,
        size_bytes: 1,
        disk_size_bytes: 4096,
        details: crate::model::FilePropertiesKindDetails::Other,
        owner: crate::model::FilePropertiesOwner {
            user: "me".to_owned(),
            group: "me".to_owned(),
        },
        volume: Err("none".to_owned()),
        permissions: None,
    }
}

#[test]
fn the_target_is_the_folder_one_item_or_the_selection() {
    let mut browser = browser();
    drop(browser.sync_info_panel());
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Folder(PathBuf::from("/workspace")))
    );

    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Entry(PathBuf::from("/workspace/a.txt")))
    );

    let request = browser.info_panel.request;
    select(&mut browser, &["/workspace/a.txt", "/workspace/b.txt"]);
    drop(browser.sync_info_panel());
    assert_eq!(browser.info_panel.target(), Some(&InfoTarget::Selection));
    assert_eq!(
        browser.info_panel.request, request,
        "several selected read nothing"
    );
    assert!(browser.info_panel.facts().is_none());
}

#[test]
fn nothing_is_read_while_the_info_panel_is_not_showing() {
    let mut browser = browser();
    // 叠到预览那格后面：不是当前标签。
    browser
        .right_dock
        .move_panel(DockPanel::Info, crate::right_dock::DropTarget::Join(0));
    browser.right_dock.select(DockPanel::Preview);
    select(&mut browser, &["/workspace/a.txt"]);

    drop(browser.sync_info_panel());

    assert_eq!(browser.info_panel.request, 0);
    assert_eq!(browser.info_panel.target(), None);
}

#[test]
fn a_result_for_an_earlier_target_is_dropped() {
    let mut browser = browser();
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    let first = browser.info_panel.request;
    select(&mut browser, &["/workspace/b.txt"]);
    drop(browser.sync_info_panel());

    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: first,
        outcome: Ok(Box::new(snapshot_of("/workspace/a.txt"))),
    }));
    assert!(browser.info_panel.facts().unwrap().snapshot.is_none());

    drop(browser.handle_info_panel_message(InfoPanelMessage::Loaded {
        request: browser.info_panel.request,
        outcome: Ok(Box::new(snapshot_of("/workspace/b.txt"))),
    }));
    let facts = browser.info_panel.facts().unwrap();
    assert_eq!(facts.path, PathBuf::from("/workspace/b.txt"));
    assert!(facts.snapshot.is_some());
}

#[test]
fn a_toggled_field_is_kept_in_the_settings() {
    let mut browser = browser();
    assert!(browser.user_config.info_panel.shows(InfoField::Owner));
    drop(browser.handle_info_panel_message(InfoPanelMessage::FieldToggled(InfoField::Owner)));
    assert!(!browser.user_config.info_panel.shows(InfoField::Owner));
}
