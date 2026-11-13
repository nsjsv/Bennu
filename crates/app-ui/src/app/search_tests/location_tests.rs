//! 搜索是标签的一个位置：回信按会话号找回自己的标签，复制出的标签各搜各的，会话里存起点。

use super::*;
use crate::model::{pane_session_from_live, BrowserTab};

fn first_page(browser: &mut FileBrowser, request: IndexedSearchRequest, path: &str) {
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(indexed_batch(
            request.run.generation,
            vec![search_hit(PathBuf::from(path), SearchFileKind::File)],
            true,
        )),
    ));
}

#[test]
fn results_reach_their_own_tab_after_switching_tabs() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    let searching_tab = browser.active_tab_id();
    let other_tab = searching_tab + 100;
    browser.pane_mut().tabs.push(BrowserTab::directory(
        other_tab,
        PathBuf::from("/elsewhere"),
    ));
    browser.pane_mut().activate_tab(other_tab);

    first_page(&mut browser, request, "/workspace/report.txt");

    assert!(browser.tab().entries.is_empty());
    let searching = browser
        .pane()
        .tabs
        .iter()
        .find(|tab| tab.id == searching_tab)
        .unwrap();
    assert_eq!(
        searching.entries[0].path,
        PathBuf::from("/workspace/report.txt")
    );
}

#[test]
fn a_copied_search_tab_searches_again_under_its_own_session() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let original_request = pending_indexed_request(&browser);

    let mut copy = browser.tab().clone();
    copy.id = browser.active_tab_id() + 100;
    let copied_session = browser
        .rebind_copied_search(&mut copy)
        .expect("a search tab copies its search");
    // 副本不带原件在途的请求：原件的回信只归原件。
    assert!(copy
        .location
        .search()
        .unwrap()
        .run
        .pending_indexed_request
        .is_none());
    assert_ne!(copied_session, original_request.run.session_id);
    browser.pane_mut().tabs.push(copy);

    drop(browser.restart_copied_search(Some(copied_session)));
    first_page(&mut browser, original_request, "/workspace/original.txt");

    let copy = browser
        .search_tab(copied_session)
        .expect("the copy keeps its search");
    let copied_request = copy
        .location
        .search()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("the copy searches again");
    assert_eq!(copied_request.run.session_id, copied_session);
    assert!(copy.entries.is_empty());
    assert_eq!(
        browser.tab().entries[0].path,
        PathBuf::from("/workspace/original.txt")
    );
}

#[test]
fn a_saved_session_keeps_the_starting_folder_instead_of_the_results() {
    let root = tempdir().unwrap();
    let origin = root.path().join("docs");
    std::fs::create_dir(&origin).unwrap();
    let mut browser = browser_for_search_tests(origin.clone());
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    first_page(&mut browser, request, "/workspace/report.txt");
    browser.select_path(PathBuf::from("/workspace/report.txt"));

    let pane = pane_session_from_live(browser.pane());
    let saved = pane
        .tabs
        .iter()
        .find(|tab| tab.id == pane.active_tab_id)
        .unwrap();

    assert_eq!(saved.location, BrowserLocation::Directory(origin.clone()));
    assert!(saved.selected.is_none());
    assert!(saved.selected_paths.is_empty());
    assert_eq!(saved.back_stack, vec![origin]);
}
