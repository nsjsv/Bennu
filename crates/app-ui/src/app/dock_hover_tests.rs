use std::collections::HashSet;
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::app::info_panel::InfoTarget;
use crate::app::right_preview_panel::PreviewLoadSurface;
use crate::config;
use crate::model::{BrowserLocation, PreviewState};

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

fn select(browser: &mut FileBrowser, paths: &[&str]) {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    *browser.tab_mut().selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
    browser.tab_mut().selected = paths.last().cloned();
}

/// 停靠区显示着（默认布局：预览在上、信息在下），选中了 /workspace 里的 a.txt；悬停开不开由调用的人定。
fn browser_with_selection(shows_hovered_item: bool) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.user_config.dock_shows_hovered_item = shows_hovered_item;
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = vec![
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/b.txt", FileKind::File),
        entry("/workspace/first.txt", FileKind::File),
        entry("/workspace/second.txt", FileKind::File),
        entry("/workspace/Projects", FileKind::Directory),
    ]
    .into();
    browser.right_dock.toggle_visible();
    select(&mut browser, &["/workspace/a.txt"]);
    drop(browser.sync_right_dock_panels());
    browser
}

fn point_at(browser: &mut FileBrowser, path: Option<&str>) {
    browser.hovered_entry = path.map(PathBuf::from);
    drop(browser.sync_right_dock_panels());
}

/// 最近一次起的定时到点。
fn latest_timer_fires(browser: &mut FileBrowser, message: fn(u64) -> DockHoverMessage) {
    let generation = browser.dock_hover.generation;
    deliver(browser, message(generation));
}

fn deliver(browser: &mut FileBrowser, message: DockHoverMessage) {
    drop(browser.handle_dock_hover_message(message));
    drop(browser.sync_right_dock_panels());
}

fn settled(generation: u64) -> DockHoverMessage {
    DockHoverMessage::Settled { generation }
}

fn released(generation: u64) -> DockHoverMessage {
    DockHoverMessage::Released { generation }
}

/// 两个面板显示的是同一个条目。
fn assert_panels_show(browser: &FileBrowser, path: &str) {
    assert_eq!(
        browser.info_panel.target(),
        Some(&InfoTarget::Entry(PathBuf::from(path)))
    );
    assert_eq!(
        browser.preview_engine.preview_shown_path,
        Some(PathBuf::from(path))
    );
}

#[test]
fn by_default_both_panels_follow_only_the_selection() {
    let mut browser = browser_with_selection(false);
    assert!(!browser.user_config.dock_shows_hovered_item);

    for hovered in ["/workspace/first.txt", "/workspace/second.txt"] {
        point_at(&mut browser, Some(hovered));
        // 就算有定时号到点，也没有悬停在等。
        latest_timer_fires(&mut browser, settled);
        assert!(browser.dock_hover.timer.is_none());
        assert_panels_show(&browser, "/workspace/a.txt");
    }
    point_at(&mut browser, None);
    assert_panels_show(&browser, "/workspace/a.txt");
}

#[test]
fn a_hovered_item_shows_in_both_panels_only_after_the_pointer_rests_on_it() {
    let mut browser = browser_with_selection(true);

    point_at(&mut browser, Some("/workspace/first.txt"));
    let first = browser.dock_hover.generation;
    point_at(&mut browser, Some("/workspace/second.txt"));
    assert_panels_show(&browser, "/workspace/a.txt");

    // 划过去的那个的定时作废。
    deliver(&mut browser, settled(first));
    assert_panels_show(&browser, "/workspace/a.txt");

    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/second.txt");
}

#[test]
fn leaving_a_shown_item_keeps_it_in_both_panels_until_the_release_timer_fires() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/first.txt");

    // 行间空隙、空白处：一离开不回选中项。
    point_at(&mut browser, None);
    point_at(&mut browser, None);
    let release = browser.dock_hover.generation;
    assert_panels_show(&browser, "/workspace/first.txt");

    // 号对不上的到点不算。
    deliver(&mut browser, released(release - 1));
    assert_panels_show(&browser, "/workspace/first.txt");

    latest_timer_fires(&mut browser, released);
    assert_panels_show(&browser, "/workspace/a.txt");
}

#[test]
fn settling_on_another_item_before_the_release_goes_straight_over() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);

    point_at(&mut browser, None);
    let stale_release = browser.dock_hover.generation;
    point_at(&mut browser, Some("/workspace/second.txt"));
    assert_panels_show(&browser, "/workspace/first.txt");
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/second.txt");

    deliver(&mut browser, released(stale_release));
    assert_panels_show(&browser, "/workspace/second.txt");
}

#[test]
fn passing_over_items_and_coming_back_does_not_restart_the_release() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);

    // 离开、划过别的条目（没停稳）、又回到显示着的那个：先前离开的定时作废，不会回选中项。
    point_at(&mut browser, None);
    let stale_release = browser.dock_hover.generation;
    point_at(&mut browser, Some("/workspace/second.txt"));
    point_at(&mut browser, Some("/workspace/first.txt"));
    assert!(browser.dock_hover.timer.is_none());
    deliver(&mut browser, released(stale_release));
    deliver(&mut browser, settled(stale_release + 1));
    assert_panels_show(&browser, "/workspace/first.txt");

    // 划过别的条目又离开（没停稳）：重新数离开的时间。
    point_at(&mut browser, Some("/workspace/second.txt"));
    point_at(&mut browser, None);
    let later_release = browser.dock_hover.generation;
    assert!(later_release > stale_release);
    deliver(&mut browser, released(stale_release));
    assert_panels_show(&browser, "/workspace/first.txt");
    latest_timer_fires(&mut browser, released);
    assert_panels_show(&browser, "/workspace/a.txt");
}

#[test]
fn turning_the_setting_off_while_an_item_is_shown_puts_both_panels_back_on_the_selection() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/first.txt");

    deliver(&mut browser, DockHoverMessage::ShowHoveredItemToggled);

    assert!(!browser.user_config.dock_shows_hovered_item);
    assert_panels_show(&browser, "/workspace/a.txt");

    // 再开：指针还停在那个条目上，重新停够才显示。
    deliver(&mut browser, DockHoverMessage::ShowHoveredItemToggled);
    assert!(browser.user_config.dock_shows_hovered_item);
    assert_panels_show(&browser, "/workspace/a.txt");
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/first.txt");
}

#[test]
fn a_shown_item_is_forgotten_while_the_dock_is_hidden() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);

    browser.right_dock.toggle_visible();
    drop(browser.sync_right_dock_panels());
    browser.right_dock.toggle_visible();
    point_at(&mut browser, None);

    assert_panels_show(&browser, "/workspace/a.txt");
}

#[test]
fn a_shown_item_is_forgotten_while_only_other_panels_are_showing() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);

    // 只剩文件夹树显示着：信息、预览都不显示，没有谁要看悬停项。
    browser.right_dock.close(DockPanel::Preview);
    browser.right_dock.close(DockPanel::Info);
    browser.right_dock.open(DockPanel::FolderTree);
    drop(browser.sync_right_dock_panels());

    assert!(browser.dock_hovered_item().is_none());
    assert!(browser.dock_hover.timer.is_none());
}

#[test]
fn a_hovered_item_wins_over_a_multi_selection_until_the_pointer_leaves() {
    let mut browser = browser_with_selection(true);
    select(&mut browser, &["/workspace/a.txt", "/workspace/b.txt"]);
    drop(browser.sync_right_dock_panels());
    assert_eq!(browser.info_panel.target(), Some(&InfoTarget::Selection));

    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/first.txt");

    point_at(&mut browser, None);
    latest_timer_fires(&mut browser, released);
    assert_eq!(browser.info_panel.target(), Some(&InfoTarget::Selection));
    assert_eq!(
        browser.preview_engine.preview_shown_path,
        Some(PathBuf::from("/workspace/b.txt")),
        "the preview follows the focused item of a multi-selection"
    );
}

#[test]
fn a_hovered_folder_empties_the_preview_and_is_shown_by_the_info_panel() {
    let mut browser = browser_with_selection(true);
    assert!(browser.preview_engine.preview.is_some());

    point_at(&mut browser, Some("/workspace/Projects"));
    latest_timer_fires(&mut browser, settled);

    assert_panels_show(&browser, "/workspace/Projects");
    assert!(browser.preview_engine.preview.is_none());

    point_at(&mut browser, None);
    latest_timer_fires(&mut browser, released);
    assert_panels_show(&browser, "/workspace/a.txt");
    assert!(browser.preview_engine.preview.is_some());
}

#[test]
fn hovering_never_changes_what_an_open_standalone_preview_window_shows() {
    let mut browser = browser_with_selection(true);
    point_at(&mut browser, Some("/workspace/first.txt"));
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/first.txt");

    // 空格：独立窗口按选中项（a.txt）打开，悬停项随即放掉。
    drop(browser.request_preview());
    drop(browser.sync_right_dock_panels());
    assert!(browser.preview_engine.preview_window.is_some());
    assert_eq!(
        browser.preview_engine.preview_load_surface,
        PreviewLoadSurface::StandaloneWindow
    );
    assert_panels_show(&browser, "/workspace/a.txt");
    assert!(browser.preview_engine.preview.is_some());

    // 窗口开着：指针在别的条目上停多久、定时到不到点，窗口都不变。
    for hovered in ["/workspace/second.txt", "/workspace/Projects"] {
        point_at(&mut browser, Some(hovered));
        latest_timer_fires(&mut browser, settled);
        assert!(browser.dock_hovered_item().is_none());
        assert_panels_show(&browser, "/workspace/a.txt");
        assert_eq!(
            browser.preview_engine.preview_load_surface,
            PreviewLoadSurface::StandaloneWindow
        );
        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(_))
        ));
    }

    // 窗口关了才重新跟悬停：指针此刻在 Projects 上，停够才显示。
    drop(browser.close_preview_window());
    drop(browser.sync_right_dock_panels());
    assert_panels_show(&browser, "/workspace/a.txt");
    latest_timer_fires(&mut browser, settled);
    assert_panels_show(&browser, "/workspace/Projects");
}
