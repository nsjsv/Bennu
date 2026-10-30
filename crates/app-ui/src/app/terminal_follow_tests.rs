use std::path::{Path, PathBuf};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode, NavigationMode,
    SplitAxis,
};
use crate::operation_history::CompletedTransfer;

// 终端面板展开时，它的 shell 跟随文件区的目标目录。目标由「活动窗格的活动
// 标签」和多栏下悬停的栏决定，这些用例逐个断言引起目标变化的转换都已经
// 更新了跟随目标，而不是靠事后统一检查。

/// 列表视图：跟随目标就是标签当前目录（多栏视图改看悬停的栏）。
fn list_tab(id: usize, directory: &str) -> BrowserTab {
    let mut tab = BrowserTab::directory(id, PathBuf::from(directory));
    tab.view_mode = BrowserViewMode::List;
    tab
}

fn browser_with_expanded_terminal(directory: &str) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    *browser.tab_mut() = list_tab(browser.active_tab_id(), directory);
    browser.terminal_panel.expand_without_shell();
    browser
}

fn followed(browser: &FileBrowser) -> Option<&Path> {
    browser.terminal_panel.followed_directory()
}

#[test]
fn navigation_moves_the_followed_directory() {
    let mut browser = browser_with_expanded_terminal("/workspace/a");

    drop(browser.navigate_to(PathBuf::from("/workspace/b"), NavigationMode::KeepHistory));

    assert_eq!(followed(&browser), Some(Path::new("/workspace/b")));
}

#[test]
fn collapsed_panel_does_not_track_navigation() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().view_mode = BrowserViewMode::List;

    drop(browser.navigate_to(PathBuf::from("/workspace/b"), NavigationMode::KeepHistory));

    assert_eq!(followed(&browser), None);
}

#[test]
fn switching_browser_tabs_retargets_the_terminal() {
    let mut browser = browser_with_expanded_terminal("/workspace/a");
    browser.pane_mut().tabs.push(list_tab(1, "/workspace/b"));

    drop(browser.select_tab(1));

    assert_eq!(followed(&browser), Some(Path::new("/workspace/b")));
}

#[test]
fn activating_another_pane_retargets_the_terminal() {
    let mut browser = browser_with_expanded_terminal("/workspace/a");
    browser.panes.push(BrowserPane::new(
        BrowserPaneId(1),
        list_tab(7, "/workspace/b"),
    ));
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };

    browser.activate_pane(BrowserPaneId(1));

    assert_eq!(followed(&browser), Some(Path::new("/workspace/b")));
}

#[test]
fn hovering_a_column_retargets_the_terminal() {
    let mut browser = browser_with_expanded_terminal("/workspace/a");
    browser.tab_mut().view_mode = BrowserViewMode::Columns;
    assert_eq!(followed(&browser), None);

    drop(browser.handle_entry_hovered(PathBuf::from("/workspace/a/file.txt")));

    assert_eq!(followed(&browser), Some(Path::new("/workspace/a")));
}

#[test]
fn moving_the_current_directory_retargets_the_terminal() {
    let mut browser = browser_with_expanded_terminal("/workspace/a");

    drop(
        browser.accept_file_operation_moves_renamed(vec![CompletedTransfer {
            source: PathBuf::from("/workspace/a"),
            target: PathBuf::from("/workspace/renamed"),
        }]),
    );

    assert_eq!(followed(&browser), Some(Path::new("/workspace/renamed")));
}
