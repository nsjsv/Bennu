use std::path::PathBuf;

use file_search::{
    MatchSource, SearchFileKind, SearchHit, SearchMatchMode, SearchQuery, SearchResultBatch,
    SearchScope, SearchTextScope,
};

use super::*;

fn directory_query(query_id: u64, terms: &str) -> SearchQuery {
    SearchQuery {
        query_id,
        terms: terms.to_owned(),
        text_scope: SearchTextScope::NameAndContent,
        match_mode: SearchMatchMode::Plain,
        scope: SearchScope::Directory(PathBuf::from("/workspace")),
        recursive: true,
        filters: Default::default(),
        limit: SEARCH_RESULT_WINDOW,
        cursor: None,
    }
}

fn hit(index: usize) -> SearchHit {
    SearchHit {
        path: PathBuf::from(format!("/workspace/result-{index}.txt")),
        display_name: format!("result-{index}.txt"),
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

fn assert_query_is_idle(workspace: &SearchWorkspaceState, generation: u64) {
    assert_eq!(workspace.run.generation, generation);
    assert!(workspace.run.active_query.is_none());
    assert!(workspace.run.provider.is_none());
    assert!(workspace.run.next_cursor.is_none());
    assert!(workspace.run.pending_indexed_request.is_none());
    assert!(workspace.run.cancellation.is_none());
    assert!(workspace.window.hits.is_empty());
    assert!(!workspace.window.is_loading);
    assert!(workspace.window.failure.is_none());
    assert!(workspace.window.completion.is_none());
    assert_eq!(workspace.window.viewport_offset_y, 0.0);
    assert_eq!(workspace.window.viewport_height, 0.0);
    assert!(workspace.selected_paths_in_result_order().is_empty());
}

fn search_workspace_for_tests(current_folder: &str, session_id: u64) -> SearchWorkspaceState {
    SearchWorkspaceState::new(
        PathBuf::from(current_folder),
        PathBuf::from("/home/test"),
        None,
        SearchWorkspaceSessionId(session_id),
    )
}

#[test]
fn workspace_root_is_frozen_while_empty_input_is_a_valid_state() {
    let mut workspace = search_workspace_for_tests("/workspace-a", 1);

    assert_eq!(workspace.root.path(), Path::new("/workspace-a"));
    assert_eq!(
        workspace.root.selected_scope(),
        SearchDirectoryScope::CurrentFolder
    );
    assert_eq!(
        workspace.root.available_scopes(),
        [
            SearchDirectoryScope::CurrentFolder,
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
    assert!(workspace.root.select_scope(SearchDirectoryScope::Home));
    assert_eq!(workspace.root.path(), Path::new("/home/test"));
    assert!(workspace
        .root
        .select_scope(SearchDirectoryScope::AllIndexedLocations));
    assert_eq!(workspace.root.query_scope(), SearchScope::Global);
    assert_eq!(workspace.root.path(), Path::new("/home/test"));
    workspace.clear_query();
    assert_eq!(workspace.root.path(), Path::new("/home/test"));
    assert!(workspace.input.is_empty());
}

#[test]
fn home_root_normalizes_to_one_home_scope() {
    let mut workspace = SearchWorkspaceState::new(
        PathBuf::from("/home/test"),
        PathBuf::from("/home/test"),
        None,
        SearchWorkspaceSessionId(1),
    );

    assert_eq!(workspace.root.path(), Path::new("/home/test"));
    assert_eq!(workspace.root.selected_scope(), SearchDirectoryScope::Home);
    assert_eq!(
        workspace.root.available_scopes(),
        [
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
    assert!(!workspace
        .root
        .select_scope(SearchDirectoryScope::CurrentFolder));
    assert_eq!(workspace.root.path(), Path::new("/home/test"));
}

#[test]
fn last_search_scope_global_restores_all_indexed_locations() {
    let workspace = SearchRootSnapshot::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Global),
    );

    assert_eq!(
        workspace.selected_scope(),
        SearchDirectoryScope::AllIndexedLocations
    );
    assert_eq!(workspace.query_scope(), SearchScope::Global);
    // 全局记录与“所有索引位置”档合并，不产生重复选项。
    assert_eq!(
        workspace.available_scopes(),
        [
            SearchDirectoryScope::CurrentFolder,
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
}

#[test]
fn last_search_scope_directory_restores_last_location_scope() {
    let workspace = SearchRootSnapshot::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Directory(PathBuf::from("/downloads"))),
    );

    assert_eq!(
        workspace.selected_scope(),
        SearchDirectoryScope::LastLocation
    );
    assert_eq!(
        workspace.query_scope(),
        SearchScope::Directory(PathBuf::from("/downloads"))
    );
    assert_eq!(workspace.path(), Path::new("/downloads"));
    assert_eq!(
        workspace.last_location_directory(),
        Some(Path::new("/downloads"))
    );
    // “上次位置”档插在当前文件夹之后；选中后仍还原同一条持久化事实。
    assert_eq!(
        workspace.available_scopes(),
        [
            SearchDirectoryScope::CurrentFolder,
            SearchDirectoryScope::LastLocation,
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
    assert_eq!(
        workspace.last_search_scope(),
        LastSearchScope::Directory(PathBuf::from("/downloads"))
    );
}

#[test]
fn last_search_scope_directory_merges_into_current_folder_or_home() {
    let current_folder_workspace = SearchRootSnapshot::new(
        PathBuf::from("/downloads"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Directory(PathBuf::from("/downloads"))),
    );
    assert_eq!(
        current_folder_workspace.selected_scope(),
        SearchDirectoryScope::CurrentFolder
    );
    assert!(!current_folder_workspace
        .available_scopes()
        .contains(&SearchDirectoryScope::LastLocation));

    let home_workspace = SearchRootSnapshot::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Directory(PathBuf::from("/home/test"))),
    );
    assert_eq!(home_workspace.selected_scope(), SearchDirectoryScope::Home);
    assert!(!home_workspace
        .available_scopes()
        .contains(&SearchDirectoryScope::LastLocation));
}

#[test]
fn last_search_scope_directory_offers_last_location_from_home_too() {
    let workspace = SearchRootSnapshot::new(
        PathBuf::from("/home/test"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Directory(PathBuf::from("/downloads"))),
    );

    assert_eq!(
        workspace.selected_scope(),
        SearchDirectoryScope::LastLocation
    );
    assert_eq!(
        workspace.available_scopes(),
        [
            SearchDirectoryScope::LastLocation,
            SearchDirectoryScope::Home,
            SearchDirectoryScope::AllIndexedLocations,
        ]
    );
}

#[test]
fn missing_last_search_scope_keeps_legacy_default() {
    let workspace = SearchRootSnapshot::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        None,
    );
    assert_eq!(
        workspace.selected_scope(),
        SearchDirectoryScope::CurrentFolder
    );

    let home_workspace = SearchRootSnapshot::new(
        PathBuf::from("/home/test"),
        PathBuf::from("/home/test"),
        None,
    );
    assert_eq!(home_workspace.selected_scope(), SearchDirectoryScope::Home);
}

#[test]
fn selecting_last_location_scope_round_trips_persisted_scope() {
    let mut workspace = SearchRootSnapshot::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        Some(LastSearchScope::Directory(PathBuf::from("/downloads"))),
    );
    assert!(workspace.select_scope(SearchDirectoryScope::CurrentFolder));
    assert_eq!(
        workspace.last_search_scope(),
        LastSearchScope::Directory(PathBuf::from("/workspace"))
    );
    assert!(workspace.select_scope(SearchDirectoryScope::LastLocation));
    assert_eq!(
        workspace.query_scope(),
        SearchScope::Directory(PathBuf::from("/downloads"))
    );
    assert_eq!(
        workspace.last_search_scope(),
        LastSearchScope::Directory(PathBuf::from("/downloads"))
    );
}

#[test]
fn replacing_or_dropping_workspace_cancels_inflight_work() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (_, first) = workspace.begin_indexed_query(directory_query(1, "first"));
    let (_, second) = workspace.begin_indexed_query(directory_query(2, "second"));

    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    drop(workspace);
    assert!(second.is_cancelled());
}

#[test]
fn clearing_indexed_query_cancels_pending_page_and_restores_idle_state() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (first_request, _) = workspace.begin_indexed_query(directory_query(1, "report"));
    workspace.apply_indexed_batch(
        first_request,
        SearchResultBatch {
            query_id: 1,
            hits: vec![hit(1)],
            next_cursor: Some(SearchCursor { offset: 1 }),
            finished: false,
        },
    );
    let selected_path = workspace.window.hits[0].path.clone();
    workspace.selection.select(
        &workspace.window.hits,
        &selected_path,
        SearchSelectionGesture::Plain,
    );
    workspace.update_viewport(24.0, 48.0);
    let (stale_request, _, cancellation) = workspace
        .begin_next_indexed_page()
        .expect("pending indexed page");

    workspace.clear_query();

    assert!(cancellation.is_cancelled());
    assert_query_is_idle(&workspace, 2);
    assert!(!workspace.accepts_indexed_outcome(stale_request));
}

#[test]
fn clearing_directory_fallback_cancels_scan_and_restores_idle_state() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (_, indexed_cancellation) = workspace.begin_indexed_query(directory_query(7, "report"));
    let fallback_cancellation = workspace.begin_directory_fallback();
    workspace.apply_directory_batch(vec![hit(1)]);
    let selected_path = workspace.window.hits[0].path.clone();
    workspace.selection.select(
        &workspace.window.hits,
        &selected_path,
        SearchSelectionGesture::Plain,
    );
    workspace.update_viewport(24.0, 48.0);

    workspace.clear_query();

    assert!(indexed_cancellation.is_cancelled());
    assert!(fallback_cancellation.is_cancelled());
    assert_query_is_idle(&workspace, 8);
    assert!(!workspace.accepts_directory_fallback(7));
}

#[test]
fn input_stabilization_accepts_only_the_current_revision() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let stale = workspace.replace_input("rep".to_owned());
    let current = workspace.replace_input("report".to_owned());

    assert!(!workspace.accepts_input_stabilization(&stale));
    assert!(workspace.accepts_input_stabilization(&current));

    workspace.invalidate_input_stabilization();
    assert!(!workspace.accepts_input_stabilization(&current));
}

#[test]
fn input_stabilization_cannot_cross_workspace_sessions() {
    let mut first = search_workspace_for_tests("/workspace", 1);
    let stale = first.replace_input("report".to_owned());
    let mut reopened = search_workspace_for_tests("/workspace", 2);
    let _ = reopened.replace_input("report".to_owned());

    assert!(!reopened.accepts_input_stabilization(&stale));
}

#[test]
fn custom_extension_stabilization_is_isolated_from_terms_stabilization() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let stale = workspace.replace_custom_extensions("pdf".to_owned());
    let current = workspace.replace_custom_extensions("pdf docx".to_owned());

    assert!(!workspace.accepts_custom_extensions_stabilization(&stale));
    assert!(workspace.accepts_custom_extensions_stabilization(&current));

    // 关键词侧的稳定化请求不能被自定义后缀侧接受，反之亦然。
    let terms = workspace.replace_input("report".to_owned());
    assert!(!workspace.accepts_custom_extensions_stabilization(&terms));
    assert!(workspace.accepts_input_stabilization(&terms));
    assert!(!workspace.accepts_input_stabilization(&current));

    workspace.invalidate_input_stabilization();
    assert!(!workspace.accepts_custom_extensions_stabilization(&current));
}

#[test]
fn custom_size_stabilization_is_isolated_from_terms_and_extension_stabilization() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let stale = workspace.replace_custom_size("1MB".to_owned(), String::new());
    let current = workspace.replace_custom_size("1MB".to_owned(), "2GB".to_owned());

    assert!(!workspace.accepts_custom_size_stabilization(&stale));
    assert!(workspace.accepts_custom_size_stabilization(&current));

    // 关键词/后缀侧的稳定化请求不能被自定义大小侧接受，反之亦然。
    let terms = workspace.replace_input("report".to_owned());
    assert!(!workspace.accepts_custom_size_stabilization(&terms));
    assert!(workspace.accepts_input_stabilization(&terms));
    assert!(!workspace.accepts_input_stabilization(&current));
    let extensions = workspace.replace_custom_extensions("pdf".to_owned());
    assert!(!workspace.accepts_custom_size_stabilization(&extensions));
    assert!(workspace.accepts_custom_extensions_stabilization(&extensions));
    assert!(!workspace.accepts_custom_extensions_stabilization(&current));

    // restart 边界作废全部三路 pending。
    workspace.invalidate_input_stabilization();
    assert!(!workspace.accepts_custom_size_stabilization(&current));
}

#[test]
fn custom_size_side_edit_invalidates_the_other_side_pending_request() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let pending = workspace.replace_custom_size("1MB".to_owned(), String::new());
    // 另一侧输入推进同一 revision：上一请求作废。
    let latest = workspace.replace_custom_size("1MB".to_owned(), "2GB".to_owned());
    assert!(!workspace.accepts_custom_size_stabilization(&pending));
    assert!(workspace.accepts_custom_size_stabilization(&latest));
}

#[test]
fn search_history_popup_visibility_uses_current_focus_and_pointer_facts() {
    let history = SearchHistory::from_persisted(vec!["report".to_owned()]);
    let empty_history = SearchHistory::default();
    let mut interaction = SearchHistoryInteraction::default();
    let first = interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::KeyboardTraversal);
    interaction.accept_input_focus_check(first, SearchInputFocus::Focused);
    assert!(interaction.popup_is_visible(&history));
    assert!(!interaction.popup_is_visible(&empty_history));

    let second =
        interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::KeyboardTraversal);
    interaction.enter_popup();
    interaction.accept_input_focus_check(second, SearchInputFocus::Unfocused);
    assert!(interaction.popup_is_visible(&history));

    interaction.exit_popup();
    assert!(!interaction.popup_is_visible(&history));
}

#[test]
fn dismissing_popup_rejects_late_focus_but_a_new_interaction_can_reopen_it() {
    let history = SearchHistory::from_persisted(vec!["report".to_owned()]);
    let mut interaction = SearchHistoryInteraction::default();
    let initial =
        interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::KeyboardTraversal);
    interaction.accept_input_focus_check(initial, SearchInputFocus::Focused);
    let stale = interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::Pointer);

    interaction.dismiss_popup();
    interaction.accept_input_focus_check(stale, SearchInputFocus::Focused);
    assert!(!interaction.popup_is_visible(&history));

    let next_interaction =
        interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::KeyboardTraversal);
    interaction.accept_input_focus_check(next_interaction, SearchInputFocus::Focused);
    assert!(interaction.popup_is_visible(&history));
}

#[test]
fn resetting_history_interaction_rejects_late_focus_results() {
    let history = SearchHistory::from_persisted(vec!["report".to_owned()]);
    let mut interaction = SearchHistoryInteraction::default();
    let stale = interaction.begin_input_focus_check(SearchInputFocusCheckOrigin::Pointer);

    interaction.reset();
    interaction.accept_input_focus_check(stale, SearchInputFocus::Focused);

    assert!(!interaction.popup_is_visible(&history));
}

#[test]
fn content_degradation_is_derived_from_provider_and_active_text_scope() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let _ = workspace.begin_indexed_query(directory_query(1, "report"));
    workspace.begin_directory_fallback();
    assert!(workspace.content_search_is_degraded());

    let mut name_only = directory_query(2, "report");
    name_only.text_scope = SearchTextScope::NameOnly;
    let _ = workspace.begin_indexed_query(name_only);
    workspace.begin_directory_fallback();
    assert!(!workspace.content_search_is_degraded());
}

#[test]
fn indexed_completion_distinguishes_exact_window_from_real_overflow() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (first_request, _) = workspace.begin_indexed_query(directory_query(1, ""));
    workspace.apply_indexed_batch(
        first_request,
        SearchResultBatch {
            query_id: 1,
            hits: (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            next_cursor: None,
            finished: true,
        },
    );
    assert_eq!(
        workspace.window.completion,
        Some(SearchResultCompletion::Complete)
    );

    let (second_request, _) = workspace.begin_indexed_query(directory_query(2, ""));
    workspace.apply_indexed_batch(
        second_request,
        SearchResultBatch {
            query_id: 2,
            hits: (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            next_cursor: Some(file_search::SearchCursor {
                offset: SEARCH_RESULT_WINDOW,
            }),
            finished: false,
        },
    );
    assert_eq!(
        workspace.window.completion,
        Some(SearchResultCompletion::MoreAvailable)
    );
}

#[test]
fn indexed_pages_append_once_and_advance_the_cursor() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (first_request, _) = workspace.begin_indexed_query(directory_query(1, ""));
    workspace.apply_indexed_batch(
        first_request,
        SearchResultBatch {
            query_id: 1,
            hits: (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            next_cursor: Some(SearchCursor {
                offset: SEARCH_RESULT_WINDOW,
            }),
            finished: false,
        },
    );

    let (second_request, second_query, _) = workspace.begin_next_indexed_page().unwrap();
    assert_eq!(second_query.cursor, second_request.cursor);
    assert!(workspace.begin_next_indexed_page().is_none());
    assert!(workspace.accepts_indexed_outcome(second_request));
    workspace.apply_indexed_batch(
        second_request,
        SearchResultBatch {
            query_id: 1,
            hits: (SEARCH_RESULT_WINDOW..SEARCH_RESULT_WINDOW * 2)
                .map(hit)
                .collect(),
            next_cursor: None,
            finished: true,
        },
    );

    assert_eq!(workspace.window.hits.len(), SEARCH_RESULT_WINDOW * 2);
    assert_eq!(
        workspace.window.hits[SEARCH_RESULT_WINDOW].path,
        hit(SEARCH_RESULT_WINDOW).path
    );
    assert_eq!(
        workspace.window.completion,
        Some(SearchResultCompletion::Complete)
    );
    assert!(!workspace.accepts_indexed_outcome(second_request));
    assert!(workspace.begin_next_indexed_page().is_none());
}

#[test]
fn fallback_keeps_hits_beyond_the_indexed_page_size() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let _ = workspace.begin_indexed_query(directory_query(1, ""));
    let cancellation = workspace.begin_directory_fallback();
    workspace.apply_directory_batch((0..=SEARCH_RESULT_WINDOW).map(hit).collect());

    assert_eq!(workspace.window.hits.len(), SEARCH_RESULT_WINDOW + 1);
    assert_eq!(workspace.window.completion, None);
    assert!(!cancellation.is_cancelled());
}

#[test]
fn fallback_budget_completion_preserves_hits_as_partial_results() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let _ = workspace.begin_indexed_query(directory_query(1, "rare"));
    workspace.begin_directory_fallback();
    workspace.apply_directory_batch(vec![hit(1)]);
    workspace.finish_directory_fallback(DirectoryFallbackOutcome::Completed(
        file_search::DirectoryFallbackCompletion::EntryBudgetReached {
            inspected_entries: 50_000,
        },
    ));

    assert_eq!(workspace.window.hits.len(), 1);
    assert_eq!(
        workspace.window.completion,
        Some(SearchResultCompletion::Partial {
            inspected_entries: 50_000,
        })
    );
}

#[test]
fn search_selection_supports_plain_toggle_range_and_result_order() {
    let hits = (0..5).map(hit).collect::<Vec<_>>();
    let mut selection = SearchResultSelection::new();
    selection.select(&hits, &hits[1].path, SearchSelectionGesture::Plain);
    selection.select(&hits, &hits[3].path, SearchSelectionGesture::Range);
    assert_eq!(
        selection.selected_paths_in_result_order(&hits),
        hits[1..=3]
            .iter()
            .map(|hit| hit.path.clone())
            .collect::<Vec<_>>()
    );

    selection.select(&hits, &hits[2].path, SearchSelectionGesture::Toggle);
    assert!(!selection.is_selected(&hits[2].path));
    selection.select(&hits, &hits[4].path, SearchSelectionGesture::AdditiveRange);
    assert!(selection.is_selected(&hits[4].path));
}

#[test]
fn keyboard_selection_moves_focus_and_select_all_uses_result_order() {
    let hits = (0..3).map(hit).collect::<Vec<_>>();
    let mut selection = SearchResultSelection::new();
    assert_eq!(
        selection.move_focus(
            &hits,
            SearchSelectionStep::Next,
            SearchKeyboardSelection::Replace,
        ),
        Some(hits[0].path.clone())
    );
    assert_eq!(
        selection.move_focus(
            &hits,
            SearchSelectionStep::Next,
            SearchKeyboardSelection::Extend,
        ),
        Some(hits[1].path.clone())
    );
    assert_eq!(selection.selected_paths_in_result_order(&hits).len(), 2);

    selection.select_all(&hits);
    assert_eq!(selection.selected_paths_in_result_order(&hits).len(), 3);
}

#[test]
fn directory_batches_are_capped_at_total_limit_and_report_truncation() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let _ = workspace.begin_indexed_query(directory_query(1, "report"));
    workspace.begin_directory_fallback();
    let total = SEARCH_RESULT_TOTAL_LIMIT + SEARCH_RESULT_WINDOW;
    workspace.apply_directory_batch((0..total).map(hit).collect());

    assert_eq!(workspace.window.hits.len(), SEARCH_RESULT_TOTAL_LIMIT);
    assert!(workspace.window.truncated);
    // 丢弃的是最旧页：窗口首条应为被挤出之后的第一个结果。
    let excess = total - SEARCH_RESULT_TOTAL_LIMIT;
    assert_eq!(workspace.window.hits.first(), Some(&hit(excess)));
}

#[test]
fn indexed_pagination_stops_at_total_limit_and_first_page_resets_truncation() {
    let mut workspace = search_workspace_for_tests("/workspace", 1);
    let (first_request, _) = workspace.begin_indexed_query(directory_query(1, "report"));
    workspace.apply_indexed_batch(
        first_request,
        SearchResultBatch {
            query_id: 1,
            hits: (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            next_cursor: Some(file_search::SearchCursor {
                offset: SEARCH_RESULT_WINDOW,
            }),
            finished: false,
        },
    );
    // 首页为整窗替换语义：截断标志复位。
    assert!(!workspace.window.truncated);

    let pages = SEARCH_RESULT_TOTAL_LIMIT / SEARCH_RESULT_WINDOW;
    for page in 1..pages {
        let (request, _, _) = workspace
            .begin_next_indexed_page()
            .expect("pending indexed page");
        workspace.apply_indexed_batch(
            request,
            SearchResultBatch {
                query_id: 1,
                hits: (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
                next_cursor: Some(file_search::SearchCursor {
                    offset: (page + 1) * SEARCH_RESULT_WINDOW,
                }),
                finished: false,
            },
        );
    }

    assert_eq!(workspace.window.hits.len(), SEARCH_RESULT_TOTAL_LIMIT);
    assert!(!workspace.window.truncated);
    // 总量达上界后翻页门关闭，不再发起新的索引分页请求。
    assert!(!workspace.indexed_next_page_is_available());
}
