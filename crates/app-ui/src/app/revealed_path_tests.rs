//! 点名路径：要去的隐藏目录在上一级栏里看得见，别的隐藏条目不列；规则贯穿扫描、增量、监听，
//! 各入口（地址栏、超级框、关掉隐藏文件、会话恢复）都走它。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use file_core::{discover_directory_with_progress, DiscoveredDirectoryEntry, ResolvedEntryChange};
use file_search::{MatchSource, SearchFileKind, SearchHit};
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserLocation, BrowserPaneId, BrowserTabSession, BrowserViewMode, DirectoryCollectionPhase,
};

/// 多栏、不显示隐藏文件、第一栏是 `home` 的浏览器。`home` 下有 `.config/{fish,.hidden}`、`.ssh`、
/// `.bashrc`、`Documents`。
fn home_in_columns() -> (tempfile::TempDir, PathBuf, FileBrowser) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".config/fish")).unwrap();
    std::fs::create_dir_all(home.join(".config/.hidden")).unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    std::fs::write(home.join(".bashrc"), b"").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.options.include_hidden = false;
    browser.tab_mut().location = BrowserLocation::Directory(home.clone());
    browser.tab_mut().choose_view_mode(BrowserViewMode::Columns);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    (temp, home, browser)
}

/// 按活动标签此刻的规则真去读一遍 `directory`。
async fn listed_names(browser: &FileBrowser, directory: &Path) -> Vec<String> {
    let discovery = discover_directory_with_progress(
        directory,
        browser.listing_options(browser.active_pane_id()),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let mut names = discovery
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn type_address(browser: &mut FileBrowser, address: &Path) {
    let pane_id = BrowserPaneId::PRIMARY;
    drop(browser.begin_address_editing(pane_id));
    drop(browser.update_address_draft(pane_id, address.to_string_lossy().into_owned()));
    drop(browser.submit_address_editing(pane_id));
}

#[tokio::test]
async fn typed_hidden_folder_opens_beside_the_first_column_and_is_its_only_hidden_entry() {
    let (_temp, home, mut browser) = home_in_columns();
    let config_dir = home.join(".config");
    let generation_before = browser.pane().directory_load_generation;

    type_address(&mut browser, &config_dir);

    let tab = browser.tab();
    assert_eq!(tab.current_directory(), Some(home.as_path()));
    assert_eq!(
        tab.deepest_open_column_directory.as_ref(),
        Some(&config_dir)
    );
    assert_eq!(tab.selected.as_ref(), Some(&config_dir));
    assert!(tab.expanded_directories.contains_key(&config_dir));
    // 第一栏按新规则重读：旧列表留着，新结果落地整体替换。
    assert!(browser.pane().directory_load_generation > generation_before);
    assert_eq!(
        tab.directory_collection_phase,
        DirectoryCollectionPhase::Refreshing
    );
    assert_eq!(
        listed_names(&browser, &home).await,
        [".config", "Documents"]
    );
    assert_eq!(listed_names(&browser, &config_dir).await, ["fish"]);
}

#[tokio::test]
async fn typed_folder_outside_the_first_column_becomes_the_first_column() {
    let (_temp, home, mut browser) = home_in_columns();
    browser.tab_mut().location = BrowserLocation::Directory(home.join("Documents"));

    type_address(&mut browser, &home.join(".config"));

    let tab = browser.tab();
    assert_eq!(
        tab.current_directory(),
        Some(home.join(".config").as_path())
    );
    assert_eq!(tab.deepest_open_column_directory, None);
    assert_eq!(tab.revealed_path, None);
}

#[test]
fn typed_path_climbing_with_dot_dot_becomes_the_first_column() {
    let (_temp, home, mut browser) = home_in_columns();
    let climbing = home.join("Documents/../.config");

    type_address(&mut browser, &climbing);

    assert_eq!(browser.tab().current_directory(), Some(climbing.as_path()));
    assert_eq!(browser.tab().deepest_open_column_directory, None);
}

#[tokio::test]
async fn revealed_entry_stays_while_clicking_around_and_goes_with_the_first_column() {
    let (_temp, home, mut browser) = home_in_columns();
    type_address(&mut browser, &home.join(".config/fish"));

    browser.set_deepest_open_column_directory(Some(home.join("Documents")));
    assert_eq!(
        listed_names(&browser, &home).await,
        [".config", "Documents"]
    );

    drop(browser.navigate_to(
        home.join("Documents"),
        crate::model::NavigationMode::RecordHistory,
    ));
    drop(browser.navigate_to(home.clone(), crate::model::NavigationMode::RecordHistory));
    assert_eq!(browser.tab().revealed_path, None);
    assert_eq!(listed_names(&browser, &home).await, ["Documents"]);
}

fn discovered(path: &Path) -> DiscoveredDirectoryEntry {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    DiscoveredDirectoryEntry::with_complete_filesystem_metadata(
        path.to_path_buf(),
        path.file_name().unwrap().to_os_string(),
        file_core::FileKind::Directory,
        true,
        false,
        &metadata,
    )
}

fn root_names(browser: &FileBrowser) -> Vec<String> {
    let mut names = browser
        .tab()
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// 增量与重扫一致：外面删掉、再建回点名的隐藏目录，上一级列表跟着变；别的隐藏条目的变化不进列表。
#[tokio::test]
async fn disk_changes_to_the_revealed_entry_follow_and_other_hidden_changes_do_not() {
    let (_temp, home, mut browser) = home_in_columns();
    let config_dir = home.join(".config");
    browser.tab_mut().revealed_path = Some(config_dir.join("fish"));
    let discovery = discover_directory_with_progress(
        &home,
        browser.listing_options(BrowserPaneId::PRIMARY),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    browser.set_entries(crate::model::display_entries_in_discovery_order(&discovery).into());
    browser.tab_mut().directory_discovery = Some(discovery);
    assert_eq!(root_names(&browser), [".config", "Documents"]);

    let removed = ResolvedEntryChange::Removed {
        path: config_dir.clone(),
    };
    drop(browser.apply_entry_changes(&home, &[removed]));
    assert_eq!(root_names(&browser), ["Documents"]);

    let created = ResolvedEntryChange::Added(discovered(&config_dir));
    drop(browser.apply_entry_changes(&home, &[created]));
    assert_eq!(root_names(&browser), [".config", "Documents"]);

    let other = ResolvedEntryChange::Added(discovered(&home.join(".ssh")));
    drop(browser.apply_entry_changes(&home, &[other]));
    assert_eq!(root_names(&browser), [".config", "Documents"]);
}

#[test]
fn watchers_pass_hidden_changes_only_where_a_revealed_entry_is_listed() {
    let (_temp, home, mut browser) = home_in_columns();
    browser.tab_mut().revealed_path = Some(home.join(".config/fish"));

    // 临时目录本身可能是隐藏的（/tmp/.tmpXXX），它的上一级也算，所以只看 home 这一层。
    let revealing = browser.directories_listing_revealed_entries();
    assert!(revealing.contains(home.as_path()));
    assert!(!revealing.contains(home.join(".config").as_path()));
    assert!(!revealing.contains(home.join("Documents").as_path()));
}

#[tokio::test]
async fn turning_hidden_files_off_keeps_the_open_column_chain_listed() {
    let (_temp, home, mut browser) = home_in_columns();
    browser.options.include_hidden = true;
    browser.tab_mut().deepest_open_column_directory = Some(home.join(".config/fish"));

    drop(browser.toggle_show_hidden_files());

    assert!(!browser.options.include_hidden);
    assert_eq!(
        listed_names(&browser, &home).await,
        [".config", "Documents"]
    );
    assert_eq!(
        listed_names(&browser, &home.join(".config")).await,
        ["fish"]
    );
}

#[test]
fn restored_column_chain_is_revealed() {
    let deepest = PathBuf::from("/home/user/.config/fish");
    let session = BrowserTabSession {
        id: 1,
        location: BrowserLocation::Directory(PathBuf::from("/home/user")),
        selected: None,
        selected_paths: HashSet::new(),
        deepest_open_column_directory: Some(deepest.clone()),
        expanded_directories: Vec::new(),
        view_mode: BrowserViewMode::Columns,
        back_stack: Vec::new(),
        forward_stack: Vec::new(),
    };

    assert_eq!(session.to_browser_tab().revealed_path, Some(deepest));
}

#[tokio::test]
async fn found_hidden_file_is_listed_and_selected_in_its_folder() {
    let (_temp, home, mut browser) = home_in_columns();
    browser.tab_mut().choose_view_mode(BrowserViewMode::List);
    let bashrc = home.join(".bashrc");
    let hit = SearchHit {
        path: bashrc.clone(),
        display_name: ".bashrc".to_owned(),
        kind: SearchFileKind::File,
        size: 0,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        rank: 1.0,
        snippet: None,
        match_source: MatchSource::Name,
    };

    drop(browser.open_super_box_found(hit));

    assert_eq!(browser.tab().current_directory(), Some(home.as_path()));
    assert_eq!(browser.tab().selected.as_ref(), Some(&bashrc));
    assert_eq!(
        listed_names(&browser, &home).await,
        [".bashrc", "Documents"]
    );
}
