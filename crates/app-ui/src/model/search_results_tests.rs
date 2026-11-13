use std::path::PathBuf;

use file_operations::CompletedPathMigration;
use file_search::{
    MatchSource, SearchCursor, SearchFileKind, SearchHit, SearchMatchMode, SearchOrder,
    SearchQuery, SearchResultBatch, SearchScope, SearchTextScope,
};

use super::super::search::{
    DirectoryFallbackOutcome, SearchResultCompletion, SearchWorkspaceSessionId,
    SearchWorkspaceState, SEARCH_RESULT_TOTAL_LIMIT, SEARCH_RESULT_WINDOW,
};
use super::super::{BrowserLocation, BrowserTab};

fn query(query_id: u64, terms: &str) -> SearchQuery {
    SearchQuery {
        query_id,
        terms: terms.to_owned(),
        text_scope: SearchTextScope::NameAndContent,
        match_mode: SearchMatchMode::Plain,
        scope: SearchScope::Directory(PathBuf::from("/workspace")),
        recursive: true,
        filters: Default::default(),
        order: SearchOrder::Relevance,
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

fn page(query_id: u64, hits: Vec<SearchHit>, next_offset: Option<usize>) -> SearchResultBatch {
    SearchResultBatch {
        query_id,
        hits,
        next_cursor: next_offset.map(|offset| SearchCursor::Offset { offset }),
        finished: next_offset.is_none(),
    }
}

fn search_tab() -> BrowserTab {
    let mut tab = BrowserTab::directory(1, PathBuf::from("/workspace"));
    tab.location = BrowserLocation::Search(Box::new(SearchWorkspaceState::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        None,
        SearchWorkspaceSessionId(1),
    )));
    tab
}

fn search(tab: &BrowserTab) -> &SearchWorkspaceState {
    tab.location.search().unwrap()
}

fn entry_paths(tab: &BrowserTab) -> Vec<PathBuf> {
    tab.entries.iter().map(|entry| entry.path.clone()).collect()
}

#[test]
fn clearing_the_query_cancels_the_pending_page_and_empties_the_results() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "report")).unwrap();
    assert!(tab.accept_search_batch(first, page(1, vec![hit(1)], Some(1))));
    tab.selected_paths.insert(hit(1).path);
    let (stale, _, cancellation) = tab.begin_next_search_page().expect("next page");

    tab.clear_search_query();

    assert!(cancellation.is_cancelled());
    assert!(tab.entries.is_empty());
    assert!(tab.selected_paths.is_empty());
    let search = search(&tab);
    assert!(search.details.is_empty());
    assert!(search.run.active_query.is_none());
    assert!(search.run.pending_indexed_request.is_none());
    assert!(!search.window.is_loading);
    assert!(!search.accepts_indexed_outcome(stale));
    assert!(!tab.accept_search_batch(stale, page(1, vec![hit(2)], None)));
    assert!(tab.entries.is_empty());
}

#[test]
fn clearing_the_query_stops_the_directory_fallback() {
    let mut tab = search_tab();
    let (_, indexed) = tab.begin_search_query(query(7, "report")).unwrap();
    let fallback = tab
        .location
        .search_mut()
        .unwrap()
        .begin_directory_fallback();
    let run = search(&tab).run_id();
    tab.accept_fallback_hits(run, vec![hit(1)]);
    assert_eq!(entry_paths(&tab), vec![hit(1).path]);

    tab.clear_search_query();

    assert!(indexed.is_cancelled());
    assert!(fallback.is_cancelled());
    assert!(tab.entries.is_empty());
    tab.accept_fallback_hits(run, vec![hit(2)]);
    assert!(tab.entries.is_empty());
}

#[test]
fn completion_tells_an_exact_page_from_more_results() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "")).unwrap();
    tab.accept_search_batch(
        first,
        page(1, (0..SEARCH_RESULT_WINDOW).map(hit).collect(), None),
    );
    assert_eq!(
        search(&tab).window.completion,
        Some(SearchResultCompletion::Complete)
    );

    let (second, _) = tab.begin_search_query(query(2, "")).unwrap();
    tab.accept_search_batch(
        second,
        page(
            2,
            (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            Some(SEARCH_RESULT_WINDOW),
        ),
    );
    assert_eq!(
        search(&tab).window.completion,
        Some(SearchResultCompletion::MoreAvailable)
    );
}

#[test]
fn later_pages_append_once_in_service_order() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "")).unwrap();
    tab.accept_search_batch(
        first,
        page(
            1,
            (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            Some(SEARCH_RESULT_WINDOW),
        ),
    );

    let (second, second_query, _) = tab.begin_next_search_page().unwrap();
    assert_eq!(second_query.cursor, second.cursor);
    assert!(tab.begin_next_search_page().is_none());
    tab.accept_search_batch(
        second,
        page(
            1,
            (SEARCH_RESULT_WINDOW..SEARCH_RESULT_WINDOW * 2)
                .map(hit)
                .collect(),
            None,
        ),
    );

    assert_eq!(tab.entries.len(), SEARCH_RESULT_WINDOW * 2);
    assert_eq!(
        tab.entries[SEARCH_RESULT_WINDOW].path,
        hit(SEARCH_RESULT_WINDOW).path
    );
    assert!(!tab.accept_search_batch(second, page(1, vec![hit(9999)], None)));
    assert!(tab.begin_next_search_page().is_none());
}

#[test]
fn fallback_keeps_partial_results_when_the_scan_budget_runs_out() {
    let mut tab = search_tab();
    let _ = tab.begin_search_query(query(1, "rare")).unwrap();
    tab.location
        .search_mut()
        .unwrap()
        .begin_directory_fallback();
    let run = search(&tab).run_id();
    tab.accept_fallback_hits(run, (0..=SEARCH_RESULT_WINDOW).map(hit).collect());
    tab.finish_fallback_search(
        run,
        DirectoryFallbackOutcome::Completed(
            file_search::DirectoryFallbackCompletion::EntryBudgetReached {
                inspected_entries: 50_000,
            },
        ),
    );

    assert_eq!(tab.entries.len(), SEARCH_RESULT_WINDOW + 1);
    assert_eq!(
        search(&tab).window.completion,
        Some(SearchResultCompletion::Partial {
            inspected_entries: 50_000,
        })
    );
}

#[test]
fn results_beyond_the_total_limit_drop_the_oldest_with_their_details() {
    let mut tab = search_tab();
    let _ = tab.begin_search_query(query(1, "report")).unwrap();
    tab.location
        .search_mut()
        .unwrap()
        .begin_directory_fallback();
    let run = search(&tab).run_id();
    let total = SEARCH_RESULT_TOTAL_LIMIT + SEARCH_RESULT_WINDOW;
    tab.accept_fallback_hits(run, (0..total).map(hit).collect());

    assert_eq!(tab.entries.len(), SEARCH_RESULT_TOTAL_LIMIT);
    let search = search(&tab);
    assert!(search.window.truncated);
    assert_eq!(search.details.len(), SEARCH_RESULT_TOTAL_LIMIT);
    let excess = total - SEARCH_RESULT_TOTAL_LIMIT;
    assert_eq!(tab.entries[0].path, hit(excess).path);
    assert!(search.detail(&hit(0).path).is_none());
}

#[test]
fn paging_stops_at_the_total_limit() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "report")).unwrap();
    tab.accept_search_batch(
        first,
        page(
            1,
            (0..SEARCH_RESULT_WINDOW).map(hit).collect(),
            Some(SEARCH_RESULT_WINDOW),
        ),
    );
    for page_index in 1..SEARCH_RESULT_TOTAL_LIMIT / SEARCH_RESULT_WINDOW {
        let (request, _, _) = tab.begin_next_search_page().expect("next page");
        let start = page_index * SEARCH_RESULT_WINDOW;
        tab.accept_search_batch(
            request,
            page(
                1,
                (start..start + SEARCH_RESULT_WINDOW).map(hit).collect(),
                Some(start + SEARCH_RESULT_WINDOW),
            ),
        );
    }

    assert_eq!(tab.entries.len(), SEARCH_RESULT_TOTAL_LIMIT);
    assert!(!search(&tab).window.truncated);
    assert!(tab.begin_next_search_page().is_none());
}

#[test]
fn details_keep_the_snippet_and_where_the_words_matched() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "report")).unwrap();
    let mut content_hit = hit(1);
    content_hit.snippet = Some("the quarterly Report is due".to_owned());
    content_hit.match_source = MatchSource::Content;
    let mut name_hit = hit(2);
    name_hit.path = PathBuf::from("/workspace/Report-2026.txt");
    name_hit.display_name = "Report-2026.txt".to_owned();
    tab.accept_search_batch(
        first,
        page(1, vec![content_hit.clone(), name_hit.clone()], None),
    );

    let search = search(&tab);
    let content = search.detail(&content_hit.path).unwrap();
    assert_eq!(
        content.snippet.as_deref(),
        Some("the quarterly Report is due")
    );
    assert_eq!(content.snippet_highlights, vec![14..20]);
    assert!(content.name_highlights.is_empty());
    let name = search.detail(&name_hit.path).unwrap();
    assert_eq!(name.name_highlights, vec![0..6]);
    assert!(name.snippet.is_none());
}

#[test]
fn renamed_results_follow_their_new_path_and_name() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "report")).unwrap();
    let mut result = hit(1);
    result.path = PathBuf::from("/workspace/old/report.txt");
    result.display_name = "report.txt".to_owned();
    tab.accept_search_batch(first, page(1, vec![result], None));
    tab.selected_paths
        .insert(PathBuf::from("/workspace/old/report.txt"));

    tab.migrate_search_results(&[CompletedPathMigration::new(
        PathBuf::from("/workspace/old/report.txt"),
        PathBuf::from("/workspace/new/my-report.txt"),
    )]);

    let moved = PathBuf::from("/workspace/new/my-report.txt");
    assert_eq!(entry_paths(&tab), vec![moved.clone()]);
    assert_eq!(tab.entries[0].name, "my-report.txt");
    assert!(tab.selected_paths.contains(&moved));
    assert_eq!(
        search(&tab).detail(&moved).unwrap().name_highlights,
        vec![3..9]
    );
}

#[test]
fn removing_a_folder_removes_the_results_inside_it() {
    let mut tab = search_tab();
    let (first, _) = tab.begin_search_query(query(1, "report")).unwrap();
    let inside = PathBuf::from("/workspace/gone/report.txt");
    let outside = PathBuf::from("/workspace/kept/report.txt");
    let mut inside_hit = hit(1);
    inside_hit.path = inside.clone();
    let mut outside_hit = hit(2);
    outside_hit.path = outside.clone();
    tab.accept_search_batch(first, page(1, vec![inside_hit, outside_hit], None));
    tab.selected_paths.insert(inside.clone());

    assert!(tab.remove_search_results(&[PathBuf::from("/workspace/gone")]));
    assert_eq!(entry_paths(&tab), vec![outside]);
    assert!(tab.selected_paths.is_empty());
    assert!(search(&tab).detail(&inside).is_none());
    assert!(!tab.remove_search_results(&[PathBuf::from("/workspace/gone")]));
}
