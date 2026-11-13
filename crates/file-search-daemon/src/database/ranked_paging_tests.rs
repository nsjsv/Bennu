use std::path::{Path, PathBuf};

use crate::error::SearchError;
use crate::extractor::ExtractionStatus;
use crate::model::{SearchCursor, SearchFileKind, SearchProviderFailure, SearchQuery};
use crate::query_state::QueryState;

use super::{EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase};

fn needle_file(path: &str, content: &str) -> IndexedFile {
    let path = PathBuf::from(path);
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        kind: SearchFileKind::File,
        size: content.len() as u64,
        modified_ms: Some(1),
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: Some(content.to_owned()),
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(1),
        mtime_ns: Some(1),
        ctime_ns: Some(1),
    }
}

fn first_page(
    database: &SearchDatabase,
    query_state: &QueryState,
    limit: usize,
) -> (SearchQuery, crate::model::SearchResultBatch) {
    let mut query = SearchQuery::global(1, "needle");
    query.limit = limit;
    let batch = database.search(&query, query_state).unwrap();
    (query, batch)
}

#[test]
fn ranked_pages_stay_on_the_first_page_snapshot_while_the_index_changes() {
    let database = SearchDatabase::in_memory().unwrap();
    for index in 0..6 {
        database
            .upsert_file(&needle_file(
                &format!("/tmp/note-{index}.txt"),
                &"needle ".repeat(index + 1),
            ))
            .unwrap();
    }
    let query_state = QueryState::default();
    let (mut query, page) = first_page(&database, &query_state, 2);
    let mut seen: Vec<PathBuf> = page.hits.into_iter().map(|hit| hit.path).collect();
    let mut cursor = page.next_cursor;

    // 定格之后：新文件排在最前也不出现；还没翻到的文件被删除就跳过。
    database
        .upsert_file(&needle_file("/tmp/new.txt", &"needle ".repeat(50)))
        .unwrap();
    let unseen = (0..6)
        .map(|index| PathBuf::from(format!("/tmp/note-{index}.txt")))
        .find(|path| !seen.contains(path))
        .unwrap();
    database.delete_scope(&unseen).unwrap();

    while let Some(next_cursor) = cursor {
        query.query_id += 1;
        query.cursor = Some(next_cursor);
        let page = database.search(&query, &query_state).unwrap();
        seen.extend(page.hits.into_iter().map(|hit| hit.path));
        cursor = page.next_cursor;
    }

    let mut expected: Vec<PathBuf> = (0..6)
        .map(|index| PathBuf::from(format!("/tmp/note-{index}.txt")))
        .filter(|path| path != &unseen)
        .collect();
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a page repeated a hit: {seen:?}");
    expected.sort();
    assert_eq!(unique, expected);
}

#[test]
fn a_released_snapshot_is_reported_as_expired() {
    let database = SearchDatabase::in_memory().unwrap();
    for index in 0..3 {
        database
            .upsert_file(&needle_file(&format!("/tmp/note-{index}.txt"), "needle"))
            .unwrap();
    }
    let (mut query, page) = first_page(&database, &QueryState::default(), 1);
    query.cursor = page.next_cursor;

    // 另一份存储里没有这个编号，等同于定格已被释放或服务重启过。
    let error = database.search(&query, &QueryState::default()).unwrap_err();

    assert!(matches!(
        error,
        SearchError::SearchFailed {
            query_id: 1,
            failure: SearchProviderFailure::RankedSnapshotExpired,
        }
    ));
}

#[test]
fn a_cursor_must_match_the_kind_of_paging_its_query_uses() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&needle_file("/tmp/note.txt", "needle"))
        .unwrap();
    let query_state = QueryState::default();

    let mut ranked = SearchQuery::global(1, "needle");
    ranked.cursor = Some(SearchCursor::Offset { offset: 1 });
    assert!(matches!(
        database.search(&ranked, &query_state),
        Err(SearchError::InvalidQuery(_))
    ));

    let mut by_metadata = SearchQuery::global(2, "");
    by_metadata.cursor = Some(SearchCursor::RankedSnapshot {
        snapshot_id: 1,
        position: 0,
    });
    assert!(matches!(
        database.search(&by_metadata, &query_state),
        Err(SearchError::InvalidQuery(_))
    ));
}

#[test]
fn a_snapshot_position_past_its_end_is_rejected() {
    let database = SearchDatabase::in_memory().unwrap();
    for index in 0..2 {
        database
            .upsert_file(&needle_file(&format!("/tmp/note-{index}.txt"), "needle"))
            .unwrap();
    }
    let query_state = QueryState::default();
    let (mut query, page) = first_page(&database, &query_state, 1);
    let Some(SearchCursor::RankedSnapshot { snapshot_id, .. }) = page.next_cursor else {
        panic!("a ranked first page with more hits returns a snapshot cursor");
    };
    query.cursor = Some(SearchCursor::RankedSnapshot {
        snapshot_id,
        position: 3,
    });

    assert!(matches!(
        database.search(&query, &query_state),
        Err(SearchError::InvalidQuery(_))
    ));
}

#[test]
fn deleted_hits_at_the_end_leave_an_empty_final_page() {
    let database = SearchDatabase::in_memory().unwrap();
    for index in 0..3 {
        database
            .upsert_file(&needle_file(&format!("/tmp/note-{index}.txt"), "needle"))
            .unwrap();
    }
    let query_state = QueryState::default();
    let (mut query, page) = first_page(&database, &query_state, 1);
    let shown = page.hits[0].path.clone();
    for index in 0..3 {
        let path = PathBuf::from(format!("/tmp/note-{index}.txt"));
        if path != shown {
            database.delete_scope(Path::new(&path)).unwrap();
        }
    }

    query.cursor = page.next_cursor;
    let rest = database.search(&query, &query_state).unwrap();

    assert!(rest.hits.is_empty());
    assert!(rest.finished);
    assert!(rest.next_cursor.is_none());
}
