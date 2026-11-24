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

/// 悬停开着、选中了 a.txt、面板显示着。
fn hover_following_browser() -> FileBrowser {
    let mut browser = browser();
    browser.user_config.info_panel.follow_hover = true;
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());
    browser
}

fn point_at(browser: &mut FileBrowser, path: Option<&str>) {
    browser.hovered_entry = path.map(PathBuf::from);
    drop(browser.sync_info_panel());
}

/// 最近一次起的悬停定时到点。
fn latest_hover_timer_fires(browser: &mut FileBrowser, message: fn(u64) -> InfoPanelMessage) {
    let generation = browser.info_panel.hover_generation;
    drop(browser.handle_info_panel_message(message(generation)));
    drop(browser.sync_info_panel());
}

fn settled(generation: u64) -> InfoPanelMessage {
    InfoPanelMessage::HoverSettled { generation }
}

fn released(generation: u64) -> InfoPanelMessage {
    InfoPanelMessage::HoverReleased { generation }
}

fn assert_shows_entry(browser: &FileBrowser, path: &str) {
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Entry(PathBuf::from(path)))
    );
}

#[test]
fn by_default_the_panel_follows_only_the_selection() {
    let mut browser = browser();
    assert!(!browser.user_config.info_panel.follow_hover);
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_info_panel());

    for hovered in ["/workspace/first.mp4", "/workspace/second.mp4"] {
        point_at(&mut browser, Some(hovered));
        // 就算有定时号到点，也没有悬停在等。
        drop(browser.handle_info_panel_message(settled(browser.info_panel.hover_generation)));
        drop(browser.sync_info_panel());
        assert!(browser.info_panel.hover_timer.is_none());
        assert_shows_entry(&browser, "/workspace/a.txt");
    }
    point_at(&mut browser, None);
    assert_shows_entry(&browser, "/workspace/a.txt");
}

#[test]
fn a_hovered_item_shows_only_after_the_pointer_rests_on_it() {
    let mut browser = hover_following_browser();

    point_at(&mut browser, Some("/workspace/first.mp4"));
    let first = browser.info_panel.hover_generation;
    point_at(&mut browser, Some("/workspace/second.mp4"));
    assert_shows_entry(&browser, "/workspace/a.txt");

    // 划过去的那个的定时作废。
    drop(browser.handle_info_panel_message(settled(first)));
    drop(browser.sync_info_panel());
    assert_shows_entry(&browser, "/workspace/a.txt");

    latest_hover_timer_fires(&mut browser, settled);
    assert_shows_entry(&browser, "/workspace/second.mp4");
}

#[test]
fn leaving_a_shown_item_keeps_it_until_the_release_timer_fires() {
    let mut browser = hover_following_browser();
    point_at(&mut browser, Some("/workspace/first.mp4"));
    latest_hover_timer_fires(&mut browser, settled);
    assert_shows_entry(&browser, "/workspace/first.mp4");

    // 行间空隙、空白处：一离开不回选中项。
    point_at(&mut browser, None);
    point_at(&mut browser, None);
    let release = browser.info_panel.hover_generation;
    assert_shows_entry(&browser, "/workspace/first.mp4");

    // 号对不上的到点不算。
    drop(browser.handle_info_panel_message(released(release - 1)));
    drop(browser.sync_info_panel());
    assert_shows_entry(&browser, "/workspace/first.mp4");

    latest_hover_timer_fires(&mut browser, released);
    assert_shows_entry(&browser, "/workspace/a.txt");
}

#[test]
fn settling_on_another_item_before_the_release_goes_straight_over() {
    let mut browser = hover_following_browser();
    point_at(&mut browser, Some("/workspace/first.mp4"));
    latest_hover_timer_fires(&mut browser, settled);

    point_at(&mut browser, None);
    let stale_release = browser.info_panel.hover_generation;
    point_at(&mut browser, Some("/workspace/second.mp4"));
    assert_shows_entry(&browser, "/workspace/first.mp4");
    latest_hover_timer_fires(&mut browser, settled);
    assert_shows_entry(&browser, "/workspace/second.mp4");

    drop(browser.handle_info_panel_message(released(stale_release)));
    drop(browser.sync_info_panel());
    assert_shows_entry(&browser, "/workspace/second.mp4");
}

#[test]
fn passing_over_items_and_coming_back_does_not_restart_the_release() {
    let mut browser = hover_following_browser();
    point_at(&mut browser, Some("/workspace/first.mp4"));
    latest_hover_timer_fires(&mut browser, settled);

    // 离开、划过别的条目（没停稳）、又回到显示着的那个：先前离开的定时作废，不会回选中项。
    point_at(&mut browser, None);
    let stale_release = browser.info_panel.hover_generation;
    point_at(&mut browser, Some("/workspace/second.mp4"));
    point_at(&mut browser, Some("/workspace/first.mp4"));
    assert!(browser.info_panel.hover_timer.is_none());
    drop(browser.handle_info_panel_message(released(stale_release)));
    drop(browser.handle_info_panel_message(settled(stale_release + 1)));
    drop(browser.sync_info_panel());
    assert_shows_entry(&browser, "/workspace/first.mp4");

    // 划过别的条目又离开（没停稳）：重新数离开的时间。
    point_at(&mut browser, Some("/workspace/second.mp4"));
    point_at(&mut browser, None);
    let later_release = browser.info_panel.hover_generation;
    assert!(later_release > stale_release);
    drop(browser.handle_info_panel_message(released(stale_release)));
    drop(browser.sync_info_panel());
    assert_shows_entry(&browser, "/workspace/first.mp4");
    latest_hover_timer_fires(&mut browser, released);
    assert_shows_entry(&browser, "/workspace/a.txt");
}

#[test]
fn turning_hover_off_while_an_item_is_shown_goes_back_to_the_selection() {
    let mut browser = hover_following_browser();
    point_at(&mut browser, Some("/workspace/first.mp4"));
    latest_hover_timer_fires(&mut browser, settled);
    assert_shows_entry(&browser, "/workspace/first.mp4");

    drop(browser.handle_info_panel_message(InfoPanelMessage::FollowHoverToggled));
    drop(browser.sync_info_panel());

    assert!(!browser.user_config.info_panel.follow_hover);
    assert_shows_entry(&browser, "/workspace/a.txt");
}

#[test]
fn a_shown_item_is_forgotten_while_the_panel_is_not_showing() {
    let mut browser = hover_following_browser();
    point_at(&mut browser, Some("/workspace/first.mp4"));
    latest_hover_timer_fires(&mut browser, settled);

    browser.right_dock.toggle_visible();
    drop(browser.sync_info_panel());
    browser.right_dock.toggle_visible();
    point_at(&mut browser, None);

    assert_shows_entry(&browser, "/workspace/a.txt");
}

#[test]
fn a_toggled_field_is_kept_in_the_settings() {
    let mut browser = browser();
    assert!(browser.user_config.info_panel.shows(InfoField::Owner));
    drop(browser.handle_info_panel_message(InfoPanelMessage::FieldToggled(InfoField::Owner)));
    assert!(!browser.user_config.info_panel.shows(InfoField::Owner));
}
