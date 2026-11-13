use super::*;

#[test]
fn an_expired_ranked_snapshot_restarts_the_search_from_the_top() {
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
    scroll_search_list_to(&mut browser, f32::MAX);
    let next_page = pending_indexed_request(&browser);
    assert!(next_page.cursor.is_some());

    drop(browser.accept_search_results(next_page, IndexedSearchOutcome::SnapshotExpired));

    let restarted = pending_indexed_request(&browser);
    assert_ne!(restarted.run.generation, generation);
    assert!(restarted.cursor.is_none());
    assert!(browser.tab().entries.is_empty());
    assert!(browser.active_search().unwrap().window.failure.is_none());
}
