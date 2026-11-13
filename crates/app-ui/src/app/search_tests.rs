use std::collections::HashSet;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use iced::Point;

use file_search::{
    MatchSource, SearchFileKind, SearchHit, SearchMatchMode, SearchResultBatch, SearchScope,
    SearchTextScope,
};
use tempfile::tempdir;

use iced::Task;

use super::FileBrowser;
use crate::config;
use crate::model::search::{SearchProvider, SearchRunId, SEARCH_RESULT_WINDOW};
use crate::model::BrowserLocation;
use crate::model::{
    BrowserViewMode, ContextMenuState, DirectoryFallbackOutcome, HistoryEntry,
    IndexedSearchOutcome, IndexedSearchRequest, LastSearchScope, Message, SearchDirectoryScope,
    SearchEntryTypeMenuState, SearchEntryTypePreset, SearchResultCompletion,
    SearchServiceDiagnosticKind, SearchSizePreset, SelectionMarquee, SelectionMarqueePhase,
    SelectionMarqueeScrollAnchor, SelectionMarqueeSource,
};

#[path = "search_tests/location_tests.rs"]
mod location_tests;
#[path = "search_tests/ranked_snapshot_tests.rs"]
mod ranked_snapshot_tests;
#[path = "search_tests/search_scope_tests.rs"]
mod search_scope_tests;

fn browser_for_search_tests(root: PathBuf) -> FileBrowser {
    let root = if root.is_dir() {
        root
    } else {
        tempdir().unwrap().keep()
    };
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(root.clone());
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
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
        next_cursor: (!finished).then_some(file_search::SearchCursor::Offset {
            offset: SEARCH_RESULT_WINDOW,
        }),
        finished,
    }
}

/// 进入搜索（还不在搜索里时）并按当前意图重搜；不记搜索历史，与回车区分。
fn submit_search(browser: &mut FileBrowser) -> Task<Message> {
    let entered = browser
        .ensure_active_search()
        .expect("search starts from a local folder");
    Task::batch([entered, browser.restart_active_search()])
}

/// 结果列表滚到 `offset_y`：离末尾不到一屏时取下一页。
fn scroll_search_list_to(browser: &mut FileBrowser, offset_y: f32) {
    browser.tab_mut().choose_view_mode(BrowserViewMode::List);
    browser.pane_mut().list_viewport = Some(crate::thumbnail_cache::ColumnViewport {
        offset_y,
        height: crate::list_view::LIST_ROW_HEIGHT * 4.0,
    });
    let pane_id = browser.active_pane_id();
    drop(browser.load_more_search_results_near_end(pane_id));
}

fn pending_indexed_request(browser: &FileBrowser) -> IndexedSearchRequest {
    browser
        .active_search()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("pending indexed request")
}

fn stabilize_search_input(browser: &mut FileBrowser, value: &str) {
    if browser.active_search().is_none() {
        drop(submit_search(browser));
    }
    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_input(value.to_owned());
    drop(browser.accept_search_input_stabilization(request));
}

fn start_default_directory_fallback(
    browser: &mut FileBrowser,
    run: SearchRunId,
    unavailable_message: &str,
) {
    drop(browser.start_verified_directory_fallback(
        run,
        unavailable_message.to_owned(),
        Ok(file_search::SearchPathPreferences::default()),
    ));
}

#[test]
fn explicit_search_submission_records_history_but_internal_restart_does_not() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");

    assert!(browser.user_config.search_history.entries().is_empty());
    drop(submit_search(&mut browser));
    assert!(browser.user_config.search_history.entries().is_empty());

    drop(browser.submit_search_input());
    assert_eq!(browser.user_config.search_history.entries(), ["report"]);

    browser
        .active_search_mut()
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
    browser.tab_mut().location = BrowserLocation::Directory(elsewhere);
    stabilize_search_input(&mut browser, "report");

    let workspace = browser.active_search().unwrap();
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

    drop(submit_search(&mut browser));
    assert!(browser.active_search().is_some());
    drop(browser.close_search_workspace());
    assert_eq!(browser.user_config.last_search_scope, None);
}

#[test]
fn selecting_history_preserves_root_and_invalidates_stable_input_request() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace-a"));
    drop(submit_search(&mut browser));
    let frozen_root = browser.active_search().unwrap().root.path().to_path_buf();
    let stale = browser
        .active_search_mut()
        .unwrap()
        .replace_input("rep".to_owned());
    browser
        .user_config
        .search_history
        .record_submission("report");

    drop(browser.select_search_history_keyword("report".to_owned()));

    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.root.path(), frozen_root);
    assert_eq!(workspace.input, "report");
    assert!(!workspace.accepts_input_stabilization(&stale));
    assert_eq!(browser.user_config.search_history.entries(), ["report"]);
}

#[test]
fn history_removal_and_clear_leave_the_active_search_unchanged() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "active");
    let generation = browser.active_search().unwrap().run.generation;
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
    assert_eq!(browser.active_search().unwrap().run.generation, generation);
    assert_eq!(browser.active_search().unwrap().input, "active");

    drop(browser.clear_search_history());
    assert!(browser.user_config.search_history.entries().is_empty());
    assert_eq!(browser.active_search().unwrap().run.generation, generation);
    assert_eq!(browser.active_search().unwrap().input, "active");
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
    browser.drag_selection_anchor = Some(
        browser
            .tab()
            .current_directory()
            .unwrap()
            .join("anchor.txt"),
    );

    drop(submit_search(&mut browser));

    assert!(browser.active_search().is_some());
    assert!(browser.selection_marquee.is_none());
    assert!(browser.drag_selection_anchor.is_none());
}

#[test]
fn service_status_is_independent_from_search_workspace_lifecycle() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    browser
        .search_service
        .observe_query_transport_failure("daemon exited");

    assert!(browser.active_search().is_none());
    assert_eq!(
        browser.search_service.incidents[0].kind,
        SearchServiceDiagnosticKind::EndpointUnavailable
    );
}

#[test]
fn search_scope_is_the_folder_it_started_from() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace-a"));
    let origin = browser.tab().current_directory().unwrap().to_path_buf();
    drop(submit_search(&mut browser));
    let empty_generation = browser.active_search().unwrap().run.generation;
    assert!(browser.active_search().unwrap().run.active_query.is_none());
    assert!(browser.tab().current_directory().is_none());

    stabilize_search_input(&mut browser, "later");

    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.origin(), origin);
    assert_eq!(workspace.input, "later");
    assert!(workspace.run.generation > empty_generation);
    assert!(matches!(
        workspace.run.active_query.as_ref().unwrap().scope,
        SearchScope::Directory(ref root) if root == &origin
    ));
}

#[test]
fn empty_and_whitespace_input_leave_search_workspace_idle() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    let first_generation = browser.active_search().unwrap().run.generation;

    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("  \t  ".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.input, "  \t  ");
    assert!(workspace.run.generation > first_generation);
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.provider.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    assert!(!workspace.window.is_loading);
    assert!(browser.tab().entries.is_empty());
    assert!(workspace.window.failure.is_none());
    assert!(workspace.window.completion.is_none());
}

#[test]
fn clearing_search_rejects_old_indexed_and_fallback_messages() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let stale_request = pending_indexed_request(&browser);
    let stale_run = stale_request.run;
    let stale_generation = stale_run.generation;
    drop(browser.accept_search_results(
        stale_request,
        IndexedSearchOutcome::ProviderUnavailable("index is starting".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, stale_run, "index is starting");

    let stale_path = PathBuf::from("/workspace/stale.txt");
    drop(browser.accept_directory_search_batch(
        stale_run,
        vec![search_hit(stale_path.clone(), SearchFileKind::File)],
    ));
    browser.select_path(stale_path.clone());
    assert!(browser.tab().selected_paths.contains(&stale_path));

    let clear_request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("   ".to_owned());
    drop(browser.accept_search_input_stabilization(clear_request));

    let workspace = browser.active_search().unwrap();
    assert!(workspace.run.generation > stale_generation);
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.provider.is_none());
    assert!(workspace.run.next_cursor.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    assert!(browser.tab().entries.is_empty());
    assert!(!workspace.window.is_loading);
    assert!(workspace.window.failure.is_none());
    assert!(workspace.window.completion.is_none());
    assert!(browser.tab().selected_paths.is_empty());

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
        stale_run,
        vec![search_hit(
            PathBuf::from("/workspace/late-fallback.txt"),
            SearchFileKind::File,
        )],
    ));
    drop(browser.accept_directory_search_finished(stale_run, DirectoryFallbackOutcome::Cancelled));

    let workspace = browser.active_search().unwrap();
    assert!(browser.tab().entries.is_empty());
    assert!(workspace.window.completion.is_none());
    assert!(workspace.run.active_query.is_none());
}

#[test]
fn indexed_window_truth_comes_from_provider_completion() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    let generation = request.run.generation;
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
        browser.active_search().unwrap().window.completion,
        Some(SearchResultCompletion::Complete)
    );
}

#[test]
fn approaching_loaded_end_requests_and_appends_one_indexed_page() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let first_request = pending_indexed_request(&browser);
    let generation = first_request.run.generation;
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

    let near_end = crate::list_view::LIST_ROW_HEIGHT * 96.0;
    scroll_search_list_to(&mut browser, near_end);
    let next_request = pending_indexed_request(&browser);
    assert_eq!(
        next_request.cursor,
        Some(file_search::SearchCursor::Offset {
            offset: SEARCH_RESULT_WINDOW,
        })
    );
    scroll_search_list_to(&mut browser, near_end);
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

    let workspace = browser.active_search().unwrap();
    assert_eq!(browser.tab().entries.len(), SEARCH_RESULT_WINDOW * 2);
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
    start_default_directory_fallback(&mut browser, request.run, "index is starting");

    let workspace = browser.active_search().unwrap();
    assert_eq!(
        workspace.run.provider,
        Some(SearchProvider::DirectoryFallback)
    );
    assert!(workspace.window.is_loading);
    assert!(workspace.window.failure.is_none());
}

/// 没有搜索服务也没有 socket：回退经正式读取函数直接读配置目录里服务同一份文件。
#[tokio::test]
async fn directory_fallback_starts_without_the_search_service() {
    let config_directory = tempdir().unwrap();
    let preferences =
        crate::commands::read_directory_fallback_preferences(config_directory.path().to_path_buf())
            .await;
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::TransportUnavailable("search service is not running".to_owned()),
    ));

    drop(browser.start_verified_directory_fallback(
        request.run,
        "search service is not running".to_owned(),
        preferences,
    ));

    let workspace = browser.active_search().unwrap();
    assert_eq!(
        workspace.run.provider,
        Some(SearchProvider::DirectoryFallback)
    );
    assert!(workspace.window.failure.is_none());
}

/// 跳过规则读不懂时不回退：不能扫进用户排除的目录。
#[tokio::test]
async fn unreadable_search_exclusions_stop_the_directory_fallback() {
    let config_directory = tempdir().unwrap();
    let sidecar = file_search::search_path_file_in(config_directory.path());
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    std::fs::write(&sidecar, b"{").unwrap();
    let preferences =
        crate::commands::read_directory_fallback_preferences(config_directory.path().to_path_buf())
            .await;
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    let request = pending_indexed_request(&browser);
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::TransportUnavailable("search service is not running".to_owned()),
    ));

    drop(browser.start_verified_directory_fallback(
        request.run,
        "search service is not running".to_owned(),
        preferences,
    ));

    let workspace = browser.active_search().unwrap();
    assert!(workspace
        .window
        .failure
        .as_deref()
        .is_some_and(|failure| failure.contains("could not read search exclusions")));
}

#[test]
fn unavailable_after_indexed_batch_does_not_mix_providers() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "report");
    drop(submit_search(&mut browser));
    let request = browser
        .active_search()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("new indexed request");
    let generation = request.run.generation;
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
    scroll_search_list_to(&mut browser, f32::MAX);
    let next_request = browser
        .active_search()
        .unwrap()
        .run
        .pending_indexed_request
        .expect("next indexed request");
    drop(browser.accept_search_results(
        next_request,
        IndexedSearchOutcome::TransportUnavailable("socket closed".to_owned()),
    ));

    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.run.provider, Some(SearchProvider::Indexed));
    assert_eq!(browser.tab().entries.len(), 1);
    assert_eq!(workspace.window.failure.as_deref(), Some("socket closed"));
}

#[test]
fn fallback_budget_and_overflow_have_distinct_truthful_states() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "partial");
    let request = pending_indexed_request(&browser);
    let run = request.run;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::ProviderUnavailable("not ready".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, run, "not ready");
    drop(browser.accept_directory_search_batch(
        run,
        vec![search_hit(
            PathBuf::from("/workspace/partial.txt"),
            SearchFileKind::File,
        )],
    ));
    drop(browser.accept_directory_search_finished(
        run,
        DirectoryFallbackOutcome::Completed(
            file_search::DirectoryFallbackCompletion::EntryBudgetReached {
                inspected_entries: 50_000,
            },
        ),
    ));
    assert_eq!(
        browser.active_search().unwrap().window.completion,
        Some(SearchResultCompletion::Partial {
            inspected_entries: 50_000,
        })
    );

    stabilize_search_input(&mut browser, "overflow");
    let request = pending_indexed_request(&browser);
    let run = request.run;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::ProviderUnavailable("not ready".to_owned()),
    ));
    start_default_directory_fallback(&mut browser, run, "not ready");
    let hits = (0..=SEARCH_RESULT_WINDOW)
        .map(|index| {
            search_hit(
                PathBuf::from(format!("/workspace/result-{index}.txt")),
                SearchFileKind::File,
            )
        })
        .collect();
    drop(browser.accept_directory_search_batch(run, hits));
    assert_eq!(browser.tab().entries.len(), SEARCH_RESULT_WINDOW + 1);
}

#[test]
fn unavailable_frozen_root_cancels_the_run_without_switching_scope() {
    let root = tempdir().unwrap().keep();
    let mut browser = browser_for_search_tests(root.clone());
    drop(submit_search(&mut browser));
    let first_generation = browser.active_search().unwrap().run.generation;
    std::fs::remove_dir(&root).unwrap();

    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("later".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    {
        let workspace = browser.active_search().unwrap();
        assert_eq!(workspace.root.path(), root);
        assert!(workspace.run.generation > first_generation);
        // 根目录校验已移入后台：查询此时处于待校验状态而非直接失败。
        assert!(workspace.run.active_query.is_some());
    }

    // 以与消息处理器相同的入口注入“根不可用”的校验结果。
    let request = pending_indexed_request(&browser);
    let query = browser
        .active_search()
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

    let workspace = browser.active_search().unwrap();
    assert!(workspace.run.active_query.is_none());
    assert!(browser.tab().entries.is_empty());
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
            stale.run.generation,
            vec![search_hit(
                PathBuf::from("/workspace/stale.txt"),
                SearchFileKind::File,
            )],
            true,
        )),
    ));
    assert!(browser.tab().entries.is_empty());

    drop(browser.close_search_workspace());
    drop(browser.accept_search_results(
        stale,
        IndexedSearchOutcome::Batch(indexed_batch(stale.run.generation, Vec::new(), true)),
    ));
    assert!(browser.active_search().is_none());
}

#[test]
fn immediate_search_actions_invalidate_pending_input_stabilization() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));

    let submit_request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("submit".to_owned());
    drop(submit_search(&mut browser));
    let submit_generation = browser.active_search().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(submit_request));
    assert_eq!(
        browser.active_search().unwrap().run.generation,
        submit_generation
    );

    let filter_request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("filter".to_owned());
    drop(browser.toggle_search_entry_type(SearchEntryTypePreset::Images));
    let filter_generation = browser.active_search().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(filter_request));
    assert_eq!(
        browser.active_search().unwrap().run.generation,
        filter_generation
    );

    let reset_request = browser
        .active_search_mut()
        .unwrap()
        .replace_input("reset".to_owned());
    drop(browser.reset_search_filters());
    let reset_generation = browser.active_search().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(reset_request));
    assert_eq!(
        browser.active_search().unwrap().run.generation,
        reset_generation
    );
}

#[test]
fn reopening_workspace_rejects_old_session_and_clears_type_menu() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    let stale = browser
        .active_search_mut()
        .unwrap()
        .replace_input("old session".to_owned());
    browser.context_menu = Some(ContextMenuState::SearchEntryTypes(
        SearchEntryTypeMenuState {
            position: iced::Point::ORIGIN,
        },
    ));

    drop(browser.close_search_workspace());
    assert!(browser.context_menu.is_none());
    drop(submit_search(&mut browser));
    let reopened_generation = browser.active_search().unwrap().run.generation;
    drop(browser.accept_search_input_stabilization(stale));

    assert_eq!(
        browser.active_search().unwrap().run.generation,
        reopened_generation
    );
}

#[test]
fn results_replace_the_folder_entries_and_use_the_normal_selection() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    browser
        .tab_mut()
        .selected_paths
        .insert(PathBuf::from("/workspace/browser.txt"));
    stabilize_search_input(&mut browser, "selection");
    assert!(browser.tab().selected_paths.is_empty());
    let request = pending_indexed_request(&browser);
    let generation = request.run.generation;
    let first = PathBuf::from("/workspace/b/first.txt");
    let second = PathBuf::from("/workspace/a/second.txt");
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

    // 顺序就是搜索服务给的顺序，不按文件夹排序重排。
    let entries = browser
        .tab()
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![first.clone(), second.clone()]);

    browser.select_path(first.clone());
    browser.tab_mut().selected_paths.insert(second.clone());
    assert_eq!(browser.tab().selected_paths.len(), 2);
}

#[test]
fn opening_containing_directory_closes_workspace_and_reveals_result() {
    let root = tempdir().unwrap();
    let parent = root.path().join("folder");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("report.txt");
    std::fs::write(&path, "report").unwrap();
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    drop(submit_search(&mut browser));

    drop(browser.open_search_containing_directory(path.clone()));

    assert!(browser.active_search().is_none());
    assert!(matches!(
        browser.tab().back_stack.last(),
        Some(HistoryEntry::Search(_))
    ));
    assert_eq!(browser.tab().current_directory().unwrap(), parent);
    assert_eq!(browser.tab().selected.as_ref(), Some(&path));
    assert!(browser.tab().selected_paths.contains(&path));
}

#[test]
fn missing_containing_directory_keeps_workspace_and_reports_failure() {
    let root = tempdir().unwrap();
    let path = root.path().join("missing/report.txt");
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    drop(submit_search(&mut browser));

    drop(browser.open_search_containing_directory(path));

    let workspace = browser.active_search().unwrap();
    assert!(workspace
        .window
        .failure
        .as_deref()
        .is_some_and(|message| { message.contains("Containing directory is unavailable") }));
}

#[cfg(unix)]
#[test]
fn activating_non_utf8_directory_result_preserves_native_path() {
    let root = tempdir().unwrap();
    let path = root
        .path()
        .join(OsString::from_vec(b"directory-\x80".to_vec()));
    std::fs::create_dir(&path).unwrap();
    let mut browser = browser_for_search_tests(root.path().to_path_buf());
    stabilize_search_input(&mut browser, "directory");
    let request = pending_indexed_request(&browser);
    let generation = request.run.generation;
    drop(browser.accept_search_results(
        request,
        IndexedSearchOutcome::Batch(indexed_batch(
            generation,
            vec![search_hit(path.clone(), SearchFileKind::Directory)],
            true,
        )),
    ));

    drop(browser.activate_path(path.clone()));

    assert_eq!(browser.tab().current_directory().unwrap(), path);
    assert!(browser.active_search().is_none());
}

#[test]
fn toggling_regex_mode_submits_name_only_query() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    stabilize_search_input(&mut browser, "^report-\\d+");
    drop(browser.toggle_search_regex());

    let workspace = browser.active_search().unwrap();
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

    let workspace = browser.active_search().unwrap();
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

    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.filters.match_mode, SearchMatchMode::Regex);
    assert!(workspace.run.active_query.is_none());
    let failure = workspace.window.failure.as_deref().unwrap();
    assert!(failure.starts_with("Invalid regular expression: "));
}

#[test]
fn empty_keyword_with_valid_custom_size_submits_a_query() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    // Custom 全空输入 = 不限：空关键词仍是空闲态，不发查询。
    let workspace = browser.active_search().unwrap();
    assert!(workspace.run.active_query.is_none());

    // 至少一侧可解析后，空关键词 + 有效大小条件必须放行查询。
    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_custom_size("16kb".to_owned(), String::new());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.active_search().unwrap();
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
    drop(submit_search(&mut browser));
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    // 最小值大于最大值：稳定化接受后 restart 走 reject 路径，不发 provider。
    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_custom_size("2MB".to_owned(), "1MB".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.active_search().unwrap();
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    let failure = workspace.window.failure.as_deref().unwrap();
    assert!(failure.starts_with("Invalid size range: "));

    // 修正输入后恢复查询，报错清空。
    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_custom_size("2MB".to_owned(), "2GB".to_owned());
    drop(browser.accept_search_input_stabilization(request));

    let workspace = browser.active_search().unwrap();
    assert!(workspace.run.active_query.is_some());
    assert!(workspace.window.failure.is_none());
}

#[test]
fn switching_size_preset_clears_custom_inputs_and_restarts() {
    let mut browser = browser_for_search_tests(PathBuf::from("/workspace"));
    drop(submit_search(&mut browser));
    drop(browser.select_search_size_preset(SearchSizePreset::Custom));
    let request = browser
        .active_search_mut()
        .unwrap()
        .replace_custom_size("1MB".to_owned(), String::new());
    drop(browser.accept_search_input_stabilization(request));
    assert!(browser.active_search().unwrap().run.active_query.is_some());

    // 从 Custom 切走：清空两侧输入；空关键词回到空闲态。
    drop(browser.select_search_size_preset(SearchSizePreset::Medium));
    let workspace = browser.active_search().unwrap();
    assert_eq!(workspace.filters.size_preset, SearchSizePreset::Medium);
    assert!(workspace.filters.custom_size_min.is_empty());
    assert!(workspace.filters.custom_size_max.is_empty());
    assert!(workspace.run.active_query.is_none());
}
