use std::path::{Path, PathBuf};

use file_search::{MatchSource, SearchFileKind, SearchHit, SearchResultBatch};
use tempfile::tempdir;

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserViewMode, ContextMenuState, DestructiveActionConfirmation, IndexedSearchOutcome,
    NavigationMode, PendingOperation,
};
use crate::operation_queue::QueuedFileOperation;
use crate::shortcuts::ShortcutAction;

fn search_hit(path: PathBuf) -> SearchHit {
    SearchHit {
        display_name: path
            .file_name()
            .expect("search hit path should contain a name")
            .to_string_lossy()
            .into_owned(),
        path,
        kind: SearchFileKind::File,
        size: 0,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        rank: 1.0,
        snippet: None,
        match_source: MatchSource::Name,
    }
}

fn browser_with_search_selection() -> (FileBrowser, Vec<PathBuf>) {
    let root = tempdir().unwrap().keep();
    let first = root.join("first.txt");
    let second = root.join("second.txt");
    std::fs::write(&first, "first").unwrap();
    std::fs::write(&second, "second").unwrap();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.sync_active_tab_state();
    browser
        .selected_paths
        .insert(PathBuf::from("/underlying-browser-selection"));
    drop(browser.update_search_input("selection".to_owned()));
    drop(browser.submit_search());
    let request = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("pending indexed request");
    let generation = request.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(SearchResultBatch {
            query_id: generation,
            hits: vec![search_hit(first.clone()), search_hit(second.clone())],
            next_cursor: None,
            finished: true,
        }),
    ));
    drop(browser.press_search_result(first.clone()));
    browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;
    drop(browser.press_search_result(second.clone()));
    browser.keyboard_modifiers = iced::keyboard::Modifiers::default();

    (browser, vec![first, second])
}

#[test]
fn search_shortcut_context_blocks_hidden_pane_actions_and_routes_supported_commands() {
    let (mut browser, search_paths) = browser_with_search_selection();
    let underlying_path = search_paths[0].parent().unwrap().join("underlying.txt");
    std::fs::write(&underlying_path, "underlying").unwrap();
    browser.selected = Some(underlying_path.clone());
    browser.selected_paths.insert(underlying_path);
    let current_dir = browser.current_dir.clone();

    for action in [
        ShortcutAction::RenameSelected,
        ShortcutAction::FileProperties,
        ShortcutAction::Preview,
        ShortcutAction::Paste,
        ShortcutAction::NavigateUp,
    ] {
        drop(browser.invoke_shortcut(action));
    }
    assert!(browser.renaming.is_none());
    assert!(browser.properties_window.is_none());
    assert!(browser.preview_window.is_none());
    assert_eq!(browser.current_dir, current_dir);

    drop(browser.invoke_shortcut(ShortcutAction::Copy));
    assert!(matches!(
        browser.pending_operation.as_ref(),
        Some(PendingOperation::Copy(paths)) if paths == &search_paths
    ));

    let generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.invoke_shortcut(ShortcutAction::Refresh));
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.generation > generation);
    assert!(browser.active_search_selection().unwrap().is_empty());

    drop(browser.invoke_shortcut(ShortcutAction::Escape));
    assert!(browser.search_workspace.is_none());
}

#[test]
fn navigation_behind_workspace_keeps_the_frozen_root_until_explicit_close() {
    let root = tempdir().unwrap();
    let first_directory = root.path().join("first");
    let second_directory = root.path().join("second");
    std::fs::create_dir(&first_directory).unwrap();
    std::fs::create_dir(&second_directory).unwrap();
    let selected = first_directory.join("selected.txt");
    std::fs::write(&selected, "selected").unwrap();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = first_directory.clone();
    browser.view_mode = BrowserViewMode::List;
    browser.selected = Some(selected.clone());
    browser.selected_paths.insert(selected.clone());
    browser.sync_active_tab_state();
    drop(browser.submit_search());

    drop(browser.navigate_to(second_directory.clone(), NavigationMode::RecordHistory));
    assert!(browser.search_workspace.is_some());
    drop(browser.update_search_input("after navigation".to_owned()));
    drop(browser.submit_search());
    let query = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .active_query
        .as_ref()
        .unwrap();
    assert!(matches!(
        query.scope,
        file_search::SearchScope::Directory(ref path) if path == &first_directory
    ));

    drop(browser.close_search_workspace());
    assert_eq!(browser.current_dir, second_directory);
    assert_eq!(browser.view_mode, BrowserViewMode::List);
}

#[test]
fn copy_and_cut_reuse_the_existing_clipboard_operation_state() {
    let (mut copy_browser, expected_paths) = browser_with_search_selection();
    drop(copy_browser.copy_selected());
    assert!(matches!(
        copy_browser.pending_operation,
        Some(PendingOperation::Copy(ref paths)) if paths == &expected_paths
    ));

    let (mut cut_browser, expected_paths) = browser_with_search_selection();
    drop(cut_browser.move_selected());
    assert!(matches!(
        cut_browser.pending_operation,
        Some(PendingOperation::Move(ref paths)) if paths == &expected_paths
    ));
}

#[test]
fn trash_and_permanent_delete_reuse_queue_and_confirmation_boundaries() {
    let (mut trash_browser, expected_paths) = browser_with_search_selection();
    drop(trash_browser.trash_selected());
    assert!(matches!(
        &trash_browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::Trash { paths } if paths == &expected_paths
    ));

    let (mut delete_browser, expected_paths) = browser_with_search_selection();
    drop(delete_browser.delete_search_selection_permanently());
    assert!(matches!(
        delete_browser.destructive_action_confirmation,
        Some(DestructiveActionConfirmation::DeletePermanently { ref paths })
            if paths == &expected_paths
    ));
}

#[test]
fn shift_delete_menu_action_requests_permanent_delete_confirmation() {
    // 菜单的 Shift+删除变体:对当前选中(搜索结果或普通选中)请求永久删除确认,
    // 不直接入队,且同时收起右键菜单。
    let (mut browser, expected_paths) = browser_with_search_selection();
    drop(browser.delete_selected_permanently());
    assert!(browser.context_menu.is_none());
    assert!(matches!(
        browser.destructive_action_confirmation,
        Some(DestructiveActionConfirmation::DeletePermanently { ref paths })
            if paths == &expected_paths
    ));

    // 空选中:直接空操作,不弹确认框。
    let (mut empty_browser, _) = FileBrowser::new(config::default_user_config());
    drop(empty_browser.delete_selected_permanently());
    assert!(empty_browser.destructive_action_confirmation.is_none());
}

#[test]
fn right_click_targets_search_selection_without_mutating_underlying_selection() {
    let (mut browser, expected_paths) = browser_with_search_selection();
    let third = browser.current_dir.join("third.txt");
    drop(browser.update_search_input("third".to_owned()));
    drop(browser.submit_search());
    let request = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("new indexed request");
    let generation = request.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(SearchResultBatch {
            query_id: generation,
            hits: vec![
                search_hit(expected_paths[0].clone()),
                search_hit(expected_paths[1].clone()),
                search_hit(third.clone()),
            ],
            next_cursor: None,
            finished: true,
        }),
    ));

    drop(browser.right_click_search_result(third.clone()));

    assert_eq!(browser.active_search_selection(), Some(vec![third.clone()]));
    assert!(matches!(
        browser.context_menu,
        Some(ContextMenuState::Search(ref menu)) if menu.target == third
    ));
    assert_eq!(browser.selected_paths.len(), 1);
    assert!(browser
        .selected_paths
        .contains(Path::new("/underlying-browser-selection")));
}
