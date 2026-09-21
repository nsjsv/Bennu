use std::collections::HashSet;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use iced::Point;

use file_search::{
    MatchSource, SearchFileKind, SearchHit, SearchMatchMode, SearchResultBatch, SearchScope,
    SearchTextScope,
};
use tempfile::tempdir;

use super::FileBrowser;
use crate::config;
use crate::model::search::{SearchProvider, SEARCH_RESULT_WINDOW};
use crate::model::{
    ContextMenuState, DirectoryFallbackOutcome, IndexedSearchOutcome, IndexedSearchRequest,
    LastSearchScope, SearchDirectoryScope, SearchEntryTypeMenuState, SearchEntryTypePreset,
    SearchResultCompletion, SearchServiceDiagnosticKind, SearchSizePreset, SelectionMarquee,
    SelectionMarqueePhase, SelectionMarqueeScrollAnchor, SelectionMarqueeSource,
};

#[path = "search_tests/search_scope_tests.rs"]
mod search_scope_tests;

fn browser_for_search_tests(root: PathBuf) -> FileBrowser {
    let root = if root.is_dir() {
        root
    } else {
        tempdir().unwrap().keep()
    };
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    let pane_id = browser.active_pane_id();
    browser.pane_by_id_mut(pane_id).unwrap().current_dir = root;
    browser
}

fn search_hit(path: PathBuf, kind: SearchFileKind) -> SearchHit {
    SearchHit {
        display_name: path
            .file_name()
            .expect("search hit path should contain a file name")
            .to_string_lossy()
            .into_owned(),
        path,
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

fn indexed_batch(query_id: u64, hits: Vec<SearchHit>, finished: bool) -> SearchResultBatch {
    SearchResultBatch {
        query_id,
        hits,
        next_cursor: (!finished).then_some(file_search::SearchCursor {
            offset: SEARCH_RESULT_WINDOW,
        }),
        finished,
    }
}

fn pending_indexed_request(browser: &FileBrowser) -> IndexedSearchRequest {
    browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("pending indexed request")
}

fn stabilize_search_input(browser: &mut FileBrowser, value: &str) {
    if browser.search_workspace.is_none() {
        drop(browser.submit_search());
    }
    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input(value.to_owned());
    drop(browser.accept_search_input_stabilization(request));
}

fn start_default_directory_fallback(
    browser: &mut FileBrowser,
    generation: u64,
    unavailable_message: &str,
) {
    drop(browser.start_verified_directory_fallback(
        generation,
        unavailable_message.to_owned(),
        Ok((
            file_search::VersionedSearchPathPreferences {
                revision: 0,
                preferences: file_search::SearchPathPreferences::default(),
            },
            file_search::SearchPathConfigurationStatus::default(),
        )),
    ));
}

#[test]
fn explicit_search_submission_records_history_but_internal_restart_does_not() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");

    assert!(browser.user_config.search_history.entries().is_empty());
    drop(browser.submit_search());
    assert!(browser.user_config.search_history.entries().is_empty());

    drop(browser.submit_search_input());
    assert_eq!(browser.user_config.search_history.entries(), ["report"]);

    browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input_immediately("   ".to_owned());
    drop(browser.submit_search_input());
    assert_eq!(browser.user_config.search_history.entries(), ["report"]);
}

#[test]
fn real_query_records_last_scope_and_restores_it_on_next_workspace() {
    let roots = tempdir().unwrap();
    let downloads = roots.path().join("downloads");
    let elsewhere = roots.path().join("elsewhere");
    std::fs::create_dir(&downloads).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    let mut browser = browser_for_search_tests(downloads.clone());
    browser.home_dir = roots.path().join("home");
    stabilize_search_input(&mut browser, "report");
    assert_eq!(
        browser.user_config.last_search_scope,
        Some(LastSearchScope::Directory(downloads.clone()))
    );

    drop(browser.close_search_workspace());
    browser.current_dir = elsewhere.clone();
    let pane_id = browser.active_pane_id();
    browser.pane_by_id_mut(pane_id).unwrap().current_dir = elsewhere;
    stabilize_search_input(&mut browser, "report");

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(
        workspace.root.selected_scope(),
        SearchDirectoryScope::LastLocation
    );
    assert!(matches!(
        workspace.run.active_query.as_ref().unwrap().scope,
        SearchScope::Directory(ref root) if *root == downloads,
    ));
}

#[test]
fn switching_to_global_scope_records_global_last_scope() {
    let mut browser = browser_for_search_tests(PathBuf::from("/downloads"));
    browser.home_dir = PathBuf::from("/home/test");
    stabilize_search_input(&mut browser, "report");
    drop(browser.select_search_directory_scope(SearchDirectoryScope::AllIndexedLocations));
    assert_eq!(
        browser.user_config.last_search_scope,
        Some(LastSearchScope::Global)
    );
}

#[test]
fn opening_and_closing_without_a_query_does_not_record_last_scope() {
    let mut browser = browser_for_search_tests(PathBuf::from("/downloads"));
    browser.home_dir = PathBuf::from("/home/test");

    drop(browser.submit_search());
    assert!(browser.search_workspace.is_some());
    drop(browser.close_search_workspace());
    assert_eq!(browser.user_config.last_search_scope, None);
}

#[test]
fn selecting_history_preserves_root_and_invalidates_stable_input_request() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace-a"));
    drop(browser.submit_search());
    let frozen_root = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .root
        .path()
        .to_path_buf();
    let stale = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("rep".to_owned());
    browser
        .user_config
        .search_history
        .record_submission("report");
    browser.current_dir = tempdir().unwrap().keep();

    drop(browser.select_search_history_keyword("report".to_owned()));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.root.path(), frozen_root);
    assert_eq!(workspace.input, "report");
    assert!(!workspace.accepts_input_stabilization(&stale));
    assert_eq!(browser.user_config.search_history.entries(), ["report"]);
}

#[test]
fn history_removal_and_clear_leave_the_active_search_unchanged() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "active");
    let generation = browser.search_workspace.as_ref().unwrap().run.generation;
    browser
        .user_config
        .search_history
        .record_submission("report");
    browser
        .user_config
        .search_history
        .record_submission("images");

    drop(browser.remove_search_history_keyword("report"));
    assert_eq!(browser.user_config.search_history.entries(), ["images"]);
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        generation
    );
    assert_eq!(browser.search_workspace.as_ref().unwrap().input, "active");

    drop(browser.clear_search_history());
    assert!(browser.user_config.search_history.entries().is_empty());
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        generation
    );
    assert_eq!(browser.search_workspace.as_ref().unwrap().input, "active");
}

#[test]
fn opening_search_workspace_clears_file_view_pointer_interactions() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    let pane_id = browser.active_pane_id();
    browser.selection_marquee = Some(SelectionMarquee {
        gesture_origin: Point::new(10.0, 10.0),
        start: Point::new(10.0, 10.0),
        current: Point::new(40.0, 40.0),
        source: SelectionMarqueeSource::PaneBlank,
        phase: SelectionMarqueePhase::Selecting,
        scroll_anchor: SelectionMarqueeScrollAnchor::List {
            pane_id,
            offset_y: 0.0,
        },
        base_selection: HashSet::new(),
        preserve_existing: false,
    });
    browser.drag_selection_anchor = Some(browser.current_dir.join("anchor.txt"));

    drop(browser.submit_search());

    assert!(browser.search_workspace.is_some());
    assert!(browser.selection_marquee.is_none());
    assert!(browser.drag_selection_anchor.is_none());
}

#[test]
fn service_status_is_independent_from_search_workspace_lifecycle() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    browser
        .search_service
        .observe_query_transport_failure("daemon exited");

    assert!(browser.search_workspace.is_none());
    assert_eq!(
        browser.search_service.incidents[0].kind,
        SearchServiceDiagnosticKind::EndpointUnavailable
    );
}

#[test]
fn workspace_freezes_root_while_nonempty_input_stays_scoped() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace-a"));
    let frozen_root = browser.current_dir.clone();
    drop(browser.submit_search());
    let empty_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    assert!(browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .active_query
        .is_none());

    browser.current_dir = tempdir().unwrap().keep();
    stabilize_search_input(&mut browser, "later");

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.root.path(), frozen_root);
    assert_eq!(workspace.input, "later");
    assert!(workspace.run.generation > empty_generation);
    assert!(matches!(
        workspace.run.active_query.as_ref().unwrap().scope,
        SearchScope::Directory(ref root) if root == &frozen_root
    ));
}

#[test]
fn empty_and_whitespace_input_leave_search_workspace_idle() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());
    let first_generation = browser.search_workspace.as_ref().unwrap().run.generation;

    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("  \t  ".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.input, "  \t  ");
    assert!(workspace.run.generation > first_generation);
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.provider.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    assert!(!workspace.window.is_loading);
    assert!(workspace.window.hits.is_empty());
    assert!(workspace.window.failure.is_none());
    assert!(workspace.window.completion.is_none());
}

#[test]
fn clearing_search_rejects_old_indexed_and_fallback_messages() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let stale_request = pending_indexed_request(&browser);
    let stale_generation = stale_request.generation;
    drop(browser.accept_search_results(
        stale_request,
        IndexedSearchOutcome::ProviderUnavailable("index is starting".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, stale_generation, "index is starting");

    let stale_path = PathBuf::from("/workspace/stale.txt");
    drop(browser.accept_directory_search_batch(
        stale_generation,
        vec![search_hit(stale_path.clone(), SearchFileKind::File)],
    ));
    drop(browser.press_search_result(stale_path.clone()));
    assert_eq!(browser.active_search_selection(), Some(vec![stale_path]));

    let clear_request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("   ".to_owned());
    drop(browser.accept_search_input_stabilization(clear_request));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.generation > stale_generation);
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.provider.is_none());
    assert!(workspace.run.next_cursor.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    assert!(workspace.window.hits.is_empty());
    assert!(!workspace.window.is_loading);
    assert!(workspace.window.failure.is_none());
    assert!(workspace.window.completion.is_none());
    assert!(browser.active_search_selection().unwrap().is_empty());

    drop(browser.accept_search_results(
        stale_request,
        IndexedSearchOutcome::Batch(indexed_batch(
            stale_generation,
            vec![search_hit(
                PathBuf::from("/workspace/late-indexed.txt"),
                SearchFileKind::File,
            )],
            true,
        )),
    ));
    drop(browser.accept_directory_search_batch(
        stale_generation,
        vec![search_hit(
            PathBuf::from("/workspace/late-fallback.txt"),
            SearchFileKind::File,
        )],
    ));
    drop(
        browser.accept_directory_search_finished(
            stale_generation,
            DirectoryFallbackOutcome::Cancelled,
        ),
    );

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.window.hits.is_empty());
    assert!(workspace.window.completion.is_none());
    assert!(workspace.run.active_query.is_none());
}

#[test]
fn indexed_window_truth_comes_from_provider_completion() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    let generation = request.generation;
    let hits = (0..SEARCH_RESULT_WINDOW)
        .map(|index| {
            search_hit(
                PathBuf::from(format!("/workspace/result-{index}.txt")),
                SearchFileKind::File,
            )
        })
        .collect();
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(indexed_batch(generation, hits, true)),
    ));

    assert_eq!(
        browser.search_workspace.as_ref().unwrap().window.completion,
        Some(SearchResultCompletion::Complete)
    );
}

#[test]
fn approaching_loaded_end_requests_and_appends_one_indexed_page() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let first_request = pending_indexed_request(&browser);
    let generation = first_request.generation;
    let first_hits = (0..SEARCH_RESULT_WINDOW)
        .map(|index| {
            search_hit(
                PathBuf::from(format!("/workspace/result-{index}.txt")),
                SearchFileKind::File,
            )
        })
        .collect();
    drop(browser.accept_search_results(
        first_request,
        IndexedSearchOutcome::Batch(indexed_batch(generation, first_hits, false)),
    ));

    let viewport_height = crate::view::SEARCH_RESULT_ROW_HEIGHT * 4.0;
    let offset_y = crate::view::SEARCH_RESULT_ROW_HEIGHT * 90.0;
    drop(browser.update_search_results_viewport(offset_y, viewport_height));
    let next_request = pending_indexed_request(&browser);
    assert_eq!(
        next_request.cursor,
        Some(file_search::SearchCursor {
            offset: SEARCH_RESULT_WINDOW,
        })
    );
    drop(browser.update_search_results_viewport(offset_y, viewport_height));
    assert_eq!(pending_indexed_request(&browser), next_request);

    let second_hits = (SEARCH_RESULT_WINDOW..SEARCH_RESULT_WINDOW * 2)
        .map(|index| {
            search_hit(
                PathBuf::from(format!("/workspace/result-{index}.txt")),
                SearchFileKind::File,
            )
        })
        .collect();
    drop(browser.accept_search_results(
        next_request,
        IndexedSearchOutcome::Batch(indexed_batch(generation, second_hits, true)),
    ));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.window.hits.len(), SEARCH_RESULT_WINDOW * 2);
    assert_eq!(
        workspace.window.completion,
        Some(SearchResultCompletion::Complete)
    );
    assert!(workspace.run.pending_indexed_request.is_none());
}

#[test]
fn unavailable_before_first_batch_switches_to_directory_fallback() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);

    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::ProviderUnavailable("index is starting".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, request.generation, "index is starting");

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(
        workspace.run.provider,
        Some(SearchProvider::DirectoryFallback)
    );
    assert!(workspace.window.is_loading);
    assert!(workspace.window.failure.is_none());
}

#[test]
fn unavailable_after_indexed_batch_does_not_mix_providers() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
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
        IndexedSearchOutcome::Batch(indexed_batch(
            generation,
            vec![search_hit(
                PathBuf::from("/workspace/report.txt"),
                SearchFileKind::File,
            )],
            false,
        )),
    ));
    drop(browser.update_search_results_viewport(f32::MAX, crate::view::SEARCH_RESULT_ROW_HEIGHT));
    let next_request = browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("next indexed request");
    drop(browser.accept_search_results(
        next_request,
        IndexedSearchOutcome::TransportUnavailable("socket closed".to_owned()),
    ));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.run.provider, Some(SearchProvider::Indexed));
    assert_eq!(workspace.window.hits.len(), 1);
    assert_eq!(workspace.window.failure.as_deref(), Some("socket closed"));
}

#[test]
fn fallback_budget_and_overflow_have_distinct_truthful_states() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "partial");
    let request = pending_indexed_request(&browser);
    let generation = request.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::ProviderUnavailable("not ready".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, generation, "not ready");
    drop(browser.accept_directory_search_batch(
        generation,
        vec![search_hit(
            PathBuf::from("/workspace/partial.txt"),
            SearchFileKind::File,
        )],
    ));
    drop(browser.accept_directory_search_finished(
        generation,
        DirectoryFallbackOutcome::Completed(
            file_search::DirectoryFallbackCompletion::EntryBudgetReached {
                inspected_entries: 50_000,
            },
        ),
    ));
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().window.completion,
        Some(SearchResultCompletion::Partial {
            inspected_entries: 50_000,
        })
    );

    stabilize_search_input(&mut browser, "overflow");
    let request = pending_indexed_request(&browser);
    let generation = request.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::ProviderUnavailable("not ready".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, generation, "not ready");
    let hits = (0..=SEARCH_RESULT_WINDOW)
        .map(|index| {
            search_hit(
                PathBuf::from(format!("/workspace/result-{index}.txt")),
                SearchFileKind::File,
            )
        })
        .collect();
    drop(browser.accept_directory_search_batch(generation, hits));
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().window.hits.len(),
        SEARCH_RESULT_WINDOW + 1
    );
}

#[test]
fn unavailable_frozen_root_cancels_the_run_without_switching_scope() {
    let root = tempdir().unwrap().keep();
    let mut browser = browser_for_search_tests(root.clone());
    drop(browser.submit_search());
    let first_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    std::fs::remove_dir(&root).unwrap();

    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("later".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    {
        let workspace = browser.search_workspace.as_ref().unwrap();
        assert_eq!(workspace.root.path(), root);
        assert!(workspace.run.generation > first_generation);
        // 根目录校验已移入后台：查询此时处于待校验状态而非直接失败。
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
        .is_some_and(|message| message.starts_with("Search root is unavailable:")));
}

#[test]
fn stale_generation_cannot_pollute_restarted_or_closed_workspace() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "first");
    let stale = pending_indexed_request(&browser);
    stabilize_search_input(&mut browser, "second");
    drop(browser.accept_search_results(
        stale,
        IndexedSearchOutcome::Batch(indexed_batch(
            stale.generation,
            vec![search_hit(
                PathBuf::from("/workspace/stale.txt"),
                SearchFileKind::File,
            )],
            true,
        )),
    ));
    assert!(browser
        .search_workspace
        .as_ref()
        .unwrap()
        .window
        .hits
        .is_empty());

    drop(browser.close_search_workspace());
    drop(browser.accept_search_results(
        stale,
        IndexedSearchOutcome::Batch(indexed_batch(stale.generation, Vec::new(), true)),
    ));
    assert!(browser.search_workspace.is_none());
}

#[test]
fn immediate_search_actions_invalidate_pending_input_stabilization() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());

    let submit_request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("submit".to_owned());
    drop(browser.submit_search());
    let submit_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(submit_request));
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        submit_generation
    );

    let filter_request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("filter".to_owned());
    drop(browser.toggle_search_entry_type(SearchEntryTypePreset::Images));
    let filter_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(filter_request));
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        filter_generation
    );

    let reset_request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("reset".to_owned());
    drop(browser.reset_search_filters());
    let reset_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(reset_request));
    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        reset_generation
    );
}

#[test]
fn reopening_workspace_rejects_old_session_and_clears_type_menu() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());
    let stale = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_input("old session".to_owned());
    browser.context_menu = Some(ContextMenuState::SearchEntryTypes(
        SearchEntryTypeMenuState {
            position: iced::Point::ORIGIN,
        },
    ));

    drop(browser.close_search_workspace());
    assert!(browser.context_menu.is_none());
    drop(browser.submit_search());
    let reopened_generation = browser.search_workspace.as_ref().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(stale));

    assert_eq!(
        browser.search_workspace.as_ref().unwrap().run.generation,
        reopened_generation
    );
}

#[test]
fn search_selection_does_not_mutate_browser_selection() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    browser
        .selected_paths
        .insert(PathBuf::from("/workspace/browser.txt"));
    stabilize_search_input(&mut browser, "selection");
    let request = pending_indexed_request(&browser);
    let generation = request.generation;
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(indexed_batch(
            generation,
            vec![
                search_hit(first.clone(), SearchFileKind::File),
                search_hit(second.clone(), SearchFileKind::File),
            ],
            true,
        )),
    ));
    drop(browser.press_search_result(first.clone()));
    browser.keyboard_modifiers = iced::keyboard::Modifiers::CTRL;
    drop(browser.press_search_result(second.clone()));

    assert_eq!(
        browser.active_search_selection().unwrap(),
        vec![first, second]
    );
    assert_eq!(browser.selected_paths.len(), 1);
    assert!(browser
        .selected_paths
        .contains(Path::new("/workspace/browser.txt")));
}

#[test]
fn opening_containing_directory_closes_workspace_and_reveals_result() {
    let root = tempdir().unwrap();
    let parent = root.path().join("folder");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("report.txt");
    std::fs::write(&path, "report").unwrap();
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    drop(browser.submit_search());

    drop(browser.open_search_containing_directory(path.clone()));

    assert!(browser.search_workspace.is_none());
    assert_eq!(browser.current_dir, parent);
    assert_eq!(browser.selected.as_ref(), Some(&path));
    assert!(browser.selected_paths.contains(&path));
}

#[test]
fn missing_containing_directory_keeps_workspace_and_reports_failure() {
    let root = tempdir().unwrap();
    let path = root.path().join("missing/report.txt");
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    drop(browser.submit_search());

    drop(browser.open_search_containing_directory(path));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace
        .window
        .failure
        .as_deref()
        .is_some_and(|message| { message.contains("Containing directory is unavailable") }));
}

#[cfg(unix)]
#[test]
fn activating_non_utf8_directory_preserves_native_path_and_closes_workspace() {
    let root = tempdir().unwrap();
    let path = root
        .path()
        .join(OsString::from_vec(b"directory-\x80".to_vec()));
    std::fs::create_dir(&path).unwrap();
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    stabilize_search_input(&mut browser, "directory");
    let request = pending_indexed_request(&browser);
    let generation = request.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(indexed_batch(
            generation,
            vec![search_hit(path.clone(), SearchFileKind::Directory)],
            true,
        )),
    ));

    drop(browser.activate_search_path(path.clone()));

    assert_eq!(browser.current_dir, path);
    assert!(browser.search_workspace.is_none());
}

#[test]
fn toggling_regex_mode_submits_name_only_query() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "^report-\\d+");
    drop(browser.toggle_search_regex());

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.filters.match_mode, SearchMatchMode::Regex);
    let query = workspace.run.active_query.as_ref().unwrap();
    assert_eq!(query.match_mode, SearchMatchMode::Regex);
    assert_eq!(query.text_scope, SearchTextScope::NameOnly);
    assert_eq!(query.terms, "^report-\\d+");
}

#[test]
fn toggling_regex_off_restores_selected_text_scope() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    drop(browser.toggle_search_regex());
    drop(browser.toggle_search_regex());

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.filters.match_mode, SearchMatchMode::Plain);
    let query = workspace.run.active_query.as_ref().unwrap();
    assert_eq!(query.match_mode, SearchMatchMode::Plain);
    assert_eq!(query.text_scope, SearchTextScope::NameAndContent);
}

#[test]
fn invalid_regex_is_rejected_without_submitting_a_query() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "foo(");
    drop(browser.toggle_search_regex());

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.filters.match_mode, SearchMatchMode::Regex);
    assert!(workspace.run.active_query.is_none());
    let failure = workspace.window.failure.as_deref().unwrap();
    assert!(failure.starts_with("Invalid regular expression: "));
}

#[test]
fn empty_keyword_with_valid_custom_size_submits_a_query() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    // Custom 全空输入 = 不限：空关键词仍是空闲态，不发查询。
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.active_query.is_none());

    // 至少一侧可解析后，空关键词 + 有效大小条件必须放行查询。
    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_custom_size("16kb".to_owned(), String::new());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.search_workspace.as_ref().unwrap();
    let query = workspace
        .run
        .active_query
        .as_ref()
        .expect("size-only query");
    assert_eq!(query.terms, "");
    assert_eq!(
        query.filters.size,
        Some(file_search::SizeRange {
            min_bytes: 16 * 1024,
            max_bytes: None,
        })
    );
}

#[test]
fn invalid_custom_size_is_rejected_then_recovers_after_fixing_the_input() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    // 最小值大于最大值：稳定化接受后 restart 走 reject 路径，不发 provider。
    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_custom_size("2MB".to_owned(), "1MB".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    let failure = workspace.window.failure.as_deref().unwrap();
    assert!(failure.starts_with("Invalid size range: "));

    // 修正输入后恢复查询，报错清空。
    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_custom_size("2MB".to_owned(), "2GB".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.search_workspace.as_ref().unwrap();
    assert!(workspace.run.active_query.is_some());
    assert!(workspace.window.failure.is_none());
}

#[test]
fn switching_size_preset_clears_custom_inputs_and_restarts() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(browser.submit_search());
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    let request = browser
        .search_workspace
        .as_mut()
        .unwrap()
        .replace_custom_size("1MB".to_owned(), String::new());
    drop(browser.accept_search_input_stabilization(request));
    assert!(browser
        .search_workspace
        .as_ref()
        .unwrap()
        .run
        .active_query
        .is_some());

    // 从 Custom 切走：清空两侧输入；空关键词回到空闲态。
    drop(browser.select_search_size_preset(SearchSizePreset::Medium));
    let workspace = browser.search_workspace.as_ref().unwrap();
    assert_eq!(workspace.filters.size_preset, SearchSizePreset::Medium);
    assert!(workspace.filters.custom_size_min.is_empty());
    assert!(workspace.filters.custom_size_max.is_empty());
    assert!(workspace.run.active_query.is_none());
}
