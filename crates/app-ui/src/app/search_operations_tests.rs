//! 搜索结果当普通条目操作：选择、复制、剪切、删除、打开、右键菜单、拖放落点都走普通入口；
//! 没有目录可放的东西（粘贴、上一级）在搜索里不可用。

use std::path::{Path, PathBuf};

use file_search::{
    MatchSource, SearchFileKind, SearchHit, SearchOrder, SearchResultBatch, SearchSortDirection,
    SearchSortField,
};
use tempfile::tempdir;

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserLocation, BrowserViewMode, ContextMenuState, DestructiveActionConfirmation,
    FileAreaMenuItem, FileEntryMenuEntry, FileMenuPlace, HistoryEntry, IndexedSearchOutcome,
    ListColumnKind, PendingOperation,
};
use crate::shortcuts::ShortcutAction;
use file_operations::QueuedFileOperation;

fn search_hit(path: &Path, kind: SearchFileKind) -> SearchHit {
    SearchHit {
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        kind,
        size: 0,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        rank: 1.0,
        snippet: None,
        match_source: MatchSource::Name,
    }
}

struct SearchFixture {
    browser: FileBrowser,
    root: PathBuf,
    first: PathBuf,
    second: PathBuf,
    folder: PathBuf,
}

/// 一个真实文件夹里搜到两个文件和一个子文件夹，两个文件被选中。
fn searched_folder() -> SearchFixture {
    let root = tempdir().unwrap().keep();
    let folder = root.join("reports");
    let first = root.join("first.txt");
    let second = folder.join("second.txt");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(&first, "first").unwrap();
    std::fs::write(&second, "second").unwrap();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(root.clone());
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    drop(browser.update_search_input("report".to_owned()));
    drop(browser.restart_active_search());
    let request = browser
        .active_search()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("pending indexed request");
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(SearchResultBatch {
            query_id: request.run.generation,
            hits: vec![
                search_hit(&first, SearchFileKind::File),
                search_hit(&folder, SearchFileKind::Directory),
                search_hit(&second, SearchFileKind::File),
            ],
            next_cursor: None,
            finished: true,
        }),
    ));
    browser.select_path(first.clone());
    browser.tab_mut().selected_paths.insert(second.clone());

    SearchFixture {
        browser,
        root,
        first,
        second,
        folder,
    }
}

fn selected_in_result_order(fixture: &SearchFixture) -> Vec<PathBuf> {
    vec![fixture.first.clone(), fixture.second.clone()]
}

#[test]
fn copy_and_cut_take_the_selected_results() {
    let mut fixture = searched_folder();
    let expected = selected_in_result_order(&fixture);
    drop(fixture.browser.copy_selected());
    assert!(matches!(
        fixture.browser.pending_operation,
        Some(PendingOperation::Copy(ref paths)) if *paths == expected
    ));

    let mut fixture = searched_folder();
    let expected = selected_in_result_order(&fixture);
    drop(fixture.browser.move_selected());
    assert!(matches!(
        fixture.browser.pending_operation,
        Some(PendingOperation::Move(ref paths)) if *paths == expected
    ));
}

#[test]
fn trash_and_permanent_delete_take_the_selected_results() {
    let mut fixture = searched_folder();
    let expected = selected_in_result_order(&fixture);
    drop(fixture.browser.trash_selected());
    assert!(matches!(
        &fixture.browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::Trash { paths } if *paths == expected
    ));

    let mut fixture = searched_folder();
    let expected = selected_in_result_order(&fixture);
    drop(fixture.browser.delete_selected_permanently());
    assert!(matches!(
        fixture.browser.destructive_action_confirmation,
        Some(DestructiveActionConfirmation::DeletePermanently { ref paths }) if *paths == expected
    ));
}

#[test]
fn paste_and_going_up_have_no_folder_in_search() {
    let mut fixture = searched_folder();
    drop(fixture.browser.copy_selected());
    drop(fixture.browser.invoke_shortcut(ShortcutAction::Paste));
    drop(fixture.browser.invoke_shortcut(ShortcutAction::NavigateUp));

    assert!(fixture.browser.active_search().is_some());
    assert!(fixture.browser.operation_queue.tasks().is_empty());
}

#[test]
fn deleted_results_and_results_inside_deleted_folders_leave_the_list() {
    let mut fixture = searched_folder();
    drop(
        fixture
            .browser
            .remove_deleted_search_results(&[fixture.folder.clone()]),
    );

    let remaining = fixture
        .browser
        .tab()
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(remaining, vec![fixture.first.clone()]);
    assert!(!fixture
        .browser
        .tab()
        .selected_paths
        .contains(&fixture.second));
    assert!(fixture
        .browser
        .active_search()
        .unwrap()
        .detail(&fixture.second)
        .is_none());
}

#[test]
fn opening_a_missing_result_reports_it_and_removes_it() {
    let mut fixture = searched_folder();
    std::fs::remove_file(&fixture.first).unwrap();

    drop(fixture.browser.activate_path(fixture.first.clone()));

    assert!(fixture.browser.active_search().is_some());
    assert!(fixture
        .browser
        .tab()
        .entries
        .iter()
        .all(|entry| entry.path != fixture.first));
    assert!(fixture.browser.global_error_notification.is_some());
}

#[test]
fn opening_a_result_folder_records_the_search_and_back_searches_again() {
    let mut fixture = searched_folder();
    let session = fixture.browser.active_search().unwrap().session_id();

    drop(fixture.browser.activate_path(fixture.folder.clone()));
    assert_eq!(
        fixture.browser.tab().current_directory(),
        Some(fixture.folder.as_path())
    );
    assert!(matches!(
        fixture.browser.tab().back_stack.last(),
        Some(HistoryEntry::Search(recall)) if recall.input == "report" && recall.origin == fixture.root
    ));

    drop(fixture.browser.navigate_back());
    let search = fixture.browser.active_search().expect("back to the search");
    assert_ne!(search.session_id(), session);
    assert_eq!(search.input, "report");
    assert!(search.run.pending_indexed_request.is_some());
    assert!(matches!(
        fixture.browser.tab().forward_stack.last(),
        Some(HistoryEntry::Directory(directory)) if *directory == fixture.folder
    ));
}

#[test]
fn escape_and_close_return_to_the_starting_folder() {
    let mut fixture = searched_folder();
    drop(fixture.browser.invoke_shortcut(ShortcutAction::Escape));
    assert!(fixture.browser.active_search().is_none());
    assert_eq!(
        fixture.browser.tab().current_directory(),
        Some(fixture.root.as_path())
    );
}

#[test]
fn result_menu_opens_the_containing_folder_instead_of_pasting() {
    let mut fixture = searched_folder();
    drop(
        fixture
            .browser
            .handle_entry_right_clicked(fixture.second.clone()),
    );

    let Some(ContextMenuState::FileArea(menu)) = &fixture.browser.context_menu else {
        panic!("a result opens the file menu");
    };
    assert_eq!(menu.place, FileMenuPlace::SearchResults);
    let items = fixture
        .browser
        .user_config
        .context_menus
        .file_entry_menu_entries(menu.place, false, true, true)
        .into_iter()
        .flat_map(|entry| match entry {
            FileEntryMenuEntry::Item(item) => vec![item],
            FileEntryMenuEntry::Group { anchor, members } => {
                std::iter::once(anchor).chain(members).collect()
            }
        })
        .collect::<Vec<_>>();
    assert!(items.contains(&FileAreaMenuItem::OpenContainingFolder));
    assert!(!items.contains(&FileAreaMenuItem::Paste));
}

#[test]
fn only_folder_results_accept_drops() {
    let fixture = searched_folder();
    let pane_id = fixture.browser.active_pane_id();
    assert_eq!(
        fixture
            .browser
            .directory_drop_target_for_entry_in_pane(pane_id, &fixture.folder),
        Some(fixture.folder.clone())
    );
    // 文件结果来自各处，放到它上面没有明确的去处。
    assert_eq!(
        fixture
            .browser
            .directory_drop_target_for_entry_in_pane(pane_id, &fixture.first),
        None
    );
}

#[test]
fn columns_view_shows_search_results_as_a_list() {
    let mut fixture = searched_folder();
    fixture
        .browser
        .tab_mut()
        .choose_view_mode(BrowserViewMode::Columns);
    assert_eq!(fixture.browser.tab().view_mode(), BrowserViewMode::List);

    drop(fixture.browser.invoke_shortcut(ShortcutAction::Escape));
    assert_eq!(fixture.browser.tab().view_mode(), BrowserViewMode::Columns);
}

#[test]
fn clicking_a_header_resorts_all_results_without_touching_folder_sorting() {
    let mut fixture = searched_folder();
    let folder_sort = fixture.browser.user_config.list_view_preferences.sort();
    let pane_id = fixture.browser.active_pane_id();

    drop(
        fixture
            .browser
            .select_list_sort_column(pane_id, ListColumnKind::Location),
    );

    let search = fixture.browser.active_search().unwrap();
    let query = search.run.active_query.as_ref().unwrap();
    assert!(matches!(
        query.order,
        SearchOrder::Field {
            field: SearchSortField::Location,
            direction: SearchSortDirection::Ascending,
            ..
        }
    ));
    assert!(search.run.pending_indexed_request.unwrap().cursor.is_none());
    assert!(fixture.browser.tab().entries.is_empty());
    assert_eq!(
        fixture.browser.user_config.list_view_preferences.sort(),
        folder_sort
    );
}
