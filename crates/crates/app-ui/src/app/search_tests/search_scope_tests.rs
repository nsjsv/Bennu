use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use file_search::{SearchFileKind, SearchScope, SearchTextScope};
use tempfile::tempdir;

use super::*;
use crate::model::{
    BrowserViewMode, ExpandedDirectory, ExpandedDirectoryStatus, SearchDateField, SearchDatePreset,
    SearchDirectoryScope, SearchEntryTypePreset,
};

#[test]
fn columns_search_uses_last_pointer_clicked_column_instead_of_keyboard_focus() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let root_file = root.path().join("root.txt");
    let child_file = project.join("child.txt");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(&root_file, "root").unwrap();
    std::fs::write(&child_file, "child").unwrap();

    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    browser.view_mode = BrowserViewMode::Columns;
    browser.entries = vec![
        DirectoryEntry::new(
            root_file.clone(),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        ),
        DirectoryEntry::new(
            project.clone(),
            FileKind::Directory,
            EntryMetadata::default(),
            false,
            false,
            false,
        ),
    ]
    .into();
    browser.expanded_directories.insert(
        project.clone(),
        ExpandedDirectory {
            entries: vec![DirectoryEntry::new(
                child_file.clone(),
                FileKind::File,
                EntryMetadata::default(),
                false,
                false,
                false,
            )],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );
    browser.deepest_open_column_directory = Some(project.clone());

    drop(browser.handle_column_entry_clicked(root_file));
    drop(browser.select_path_from_keyboard(child_file));
    drop(browser.handle_blank_area_right_clicked(project.clone()));
    drop(browser.submit_search());
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().root.path(),
        root.path()
    );

    drop(browser.close_search_workspace());
    drop(browser.handle_column_blank_right_clicked(project.clone()));
    drop(browser.submit_search());
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().root.path(),
        project.as_path()
    );

    drop(browser.close_search_workspace());
    drop(
        browser
            .select_browser_view_mode(crate::model::BrowserPaneId::PRIMARY, BrowserViewMode::List),
    );
    drop(browser.select_browser_view_mode(
        crate::model::BrowserPaneId::PRIMARY,
        BrowserViewMode::Columns,
    ));
    drop(browser.submit_search());
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().root.path(),
        root.path()
    );
}

#[test]
fn switching_directory_scope_restarts_with_frozen_roots_and_rejects_stale_results() {
    let roots = tempdir().unwrap();
    let current_folder = roots.path().join("current");
    let home = roots.path().join("home");
    let navigated_folder = roots.path().join("navigated");
    std::fs::create_dir(&current_folder).unwrap();
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&navigated_folder).unwrap();
    let mut browser = browser_for_search_tests(current_folder.clone());
    browser.home_dir = home.clone();

    stabilize_search_input(&mut browser, "report");
    drop(browser.select_search_text_scope(SearchTextScope::NameOnly));
    drop(browser.toggle_search_entry_type(SearchEntryTypePreset::Images));
    drop(browser.select_search_date_field(SearchDateField::Created));
    drop(browser.select_search_date_preset(SearchDatePreset::PastSevenDays));
    let stale_current_request = pending_indexed_request(&browser);
    let stale_current_generation = stale_current_request.generation;
    drop(browser.accept_search_results(
        stale_current_request,
        IndexedSearchOutcome::ProviderUnavailable("index is starting".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, stale_current_generation, "index is starting");
    let old_current_hit = current_folder.join("old-current.txt");
    drop(browser.accept_directory_search_batch(
        stale_current_generation,
        vec![search_hit(old_current_hit.clone(), SearchFileKind::File)],
    ));
    drop(browser.press_search_result(old_current_hit));

    browser.current_dir = navigated_folder;
    drop(browser.select_search_directory_scope(SearchDirectoryScope::Home));
    let stale_home_request = pending_indexed_request(&browser);
    {
        let workspace = browser.search_workspace.as_ref().unwrap();
        let query = workspace.run.active_query.as_ref().unwrap();
        assert_eq!(workspace.root.selected_scope(), SearchDirectoryScope::Home);
        assert_eq!(workspace.root.path(), home);
        assert_eq!(workspace.input, "report");
        assert_eq!(workspace.filters.text_scope, SearchTextScope::NameOnly);
        assert!(workspace
            .filters
            .entry_type_is_selected(SearchEntryTypePreset::Images));
        assert_eq!(workspace.filters.date_field, SearchDateField::Created);
        assert_eq!(
            workspace.filters.date_preset,
            SearchDatePreset::PastSevenDays
        );
        assert!(query.recursive);
        assert_eq!(query.text_scope, SearchTextScope::NameOnly);
        assert!(query.filters.created.is_some());
        assert!(matches!(
            query.scope,
            SearchScope::Directory(ref root) if root == &home
        ));
        assert!(workspace.window.hits.is_empty());
        assert!(workspace.selected_paths_in_result_order().is_empty());
    }

    drop(browser.accept_search_results(
        stale_current_request,
        IndexedSearchOutcome::Batch(indexed_batch(
            stale_current_generation,
            vec![search_hit(
                current_folder.join("late-indexed.txt"),
                SearchFileKind::File,
            )],
            true,
        )),
    ));
    drop(browser.accept_directory_search_batch(
        stale_current_generation,
        vec![search_hit(
            current_folder.join("late-fallback.txt"),
            SearchFileKind::File,
        )],
    ));
    assert!(browser
        .search_workspace
        .as_ref()
        .unwrap()
        .window
        .hits
        .is_empty());

    drop(browser.select_search_directory_scope(SearchDirectoryScope::CurrentFolder));
    let current_again_request = pending_indexed_request(&browser);
    drop(browser.accept_search_results(
        stale_home_request,
        IndexedSearchOutcome::Batch(indexed_batch(
            stale_home_request.generation,
            vec![search_hit(home.join("late-home.txt"), SearchFileKind::File)],
            true,
        )),
    ));
    let workspace = browser.search_workspace.as_ref().unwrap();
    let query = workspace.run.active_query.as_ref().unwrap();
    assert!(current_again_request.generation > stale_home_request.generation);
    assert_eq!(
        workspace.root.selected_scope(),
        SearchDirectoryScope::CurrentFolder
    );
    assert_eq!(workspace.root.path(), current_folder);
    assert!(matches!(
        query.scope,
        SearchScope::Directory(ref root) if root == &current_folder
    ));
    assert!(workspace.window.hits.is_empty());
}

#[test]
fn all_indexed_locations_uses_global_query_without_directory_fallback() {
    let roots = tempdir().unwrap();
    let current_folder = roots.path().join("current");
    let home = roots.path().join("home");
    std::fs::create_dir(&current_folder).unwrap();
    std::fs::create_dir(&home).unwrap();
    let mut browser = browser_for_search_tests(current_folder);
    browser.home_dir = home;

    stabilize_search_input(&mut browser, "report");
    let stale_request = pending_indexed_request(&browser);
    drop(browser.select_search_directory_scope(SearchDirectoryScope::AllIndexedLocations));
    let global_request = pending_indexed_request(&browser);
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(global_request.generation > stale_request.generation);
    assert_eq!(
        workspace.root.selected_scope(),
        SearchDirectoryScope::AllIndexedLocations
    );
    assert!(matches!(
        workspace.run.active_query.as_ref().unwrap().scope,
        SearchScope::Global
    ));

    drop(browser.accept_search_results(
        global_request,
        IndexedSearchOutcome::ProviderUnavailable("index unavailable".to_owned()),
    ));
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.window.failure.is_some());
}

#[test]
fn empty_input_scope_selection_is_used_later_and_unavailable_home_fails_closed() {
    let roots = tempdir().unwrap();
    let current_folder = roots.path().join("current");
    let home = roots.path().join("home");
    std::fs::create_dir(&current_folder).unwrap();
    std::fs::create_dir(&home).unwrap();
    let mut browser = browser_for_search_tests(current_folder.clone());
    browser.home_dir = home.clone();

    drop(browser.submit_search());
    let empty_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.select_search_directory_scope(SearchDirectoryScope::Home));
    {
        let workspace = browser.search_workspace.as_ref().unwrap();
        assert_eq!(workspace.root.selected_scope(), SearchDirectoryScope::Home);
        assert_eq!(workspace.root.path(), home);
        assert!(workspace.run.generation > empty_generation);
        assert!(workspace.run.active_query.is_none());
    }

    stabilize_search_input(&mut browser, "later");
    assert!(matches!(
        browser
            .search_workspace
            .as_ref()
            .unwrap()
            .run
            .active_query
            .as_ref()
            .unwrap()
            .scope,
        SearchScope::Directory(ref root) if root == &home
    ));

    drop(browser.select_search_directory_scope(SearchDirectoryScope::CurrentFolder));
    let current_request = pending_indexed_request(&browser);
    drop(browser.accept_search_results(
        current_request,
        IndexedSearchOutcome::Batch(indexed_batch(
            current_request.generation,
            vec![search_hit(
                current_folder.join("current.txt"),
                SearchFileKind::File,
            )],
            true,
        )),
    ));
    std::fs::remove_dir(&home).unwrap();

    drop(browser.select_search_directory_scope(SearchDirectoryScope::Home));
    {
        let workspace = browser.search_workspace.as_ref().unwrap();
        assert_eq!(workspace.root.selected_scope(), SearchDirectoryScope::Home);
        assert_eq!(workspace.root.path(), home);
        assert!(workspace.run.generation > current_request.generation);
        // 根目录校验已移入后台：查询先进入待校验状态。
        assert!(workspace.run.active_query.is_some());
    }

    // 以与消息处理器相同的入口注入“根不可用”的校验结果。
    let request = pending_indexed_request(&browser);
    let query = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .active_query
        .clone()
        .expect("active query");
    drop(browser.accept_search_scope_root_validation(
        request,
        query,
        tokio_util::sync::CancellationToken::new(),
        false,
    ));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.window.hits.is_empty());
    assert!(workspace
        .window
        .failure
        .as_deref()
        .is_some_and(|message| message.contains(&home.display().to_string())));
}

#[test]
fn opening_search_from_home_exposes_home_and_global_scopes() {
    let home = tempdir().unwrap().keep();
    let mut browser = browser_for_search_tests(home.clone());
    browser.home_dir = home.clone();

    stabilize_search_input(&mut browser, "report");
    let generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.select_search_directory_scope(SearchDirectoryScope::CurrentFolder));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.root.selected_scope(), SearchDirectoryScope::Home);
    assert_eq!(
        workspace.root.available_scopes(),
        [
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
    assert_eq!(workspace.run.generation, generation);
    assert!(matches!(
        workspace.run.active_query.as_ref().unwrap().scope,
        SearchScope::Directory(ref root) if root == &home
    ));
}
