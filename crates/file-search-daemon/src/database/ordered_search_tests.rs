use std::path::{Path, PathBuf};

use crate::extractor::ExtractionStatus;
use crate::model::{
    SearchCursor, SearchFileKind, SearchMatchMode, SearchOrder, SearchQuery, SearchScope,
    SearchSortDirection, SearchSortField,
};
use crate::query_state::QueryState;

use super::super::super::{
    EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase,
};

fn indexed(path: &str, kind: SearchFileKind, size: u64, modified_ms: i64) -> IndexedFile {
    let path = PathBuf::from(path);
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        kind,
        size,
        modified_ms: Some(modified_ms),
        accessed_ms: None,
        created_ms: None,
        mime_type: None,
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: None,
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(1),
        mtime_ns: Some(1),
        ctime_ns: Some(1),
    }
}

/// /tmp/b 下的 report-b.txt（大、旧）、/tmp/a 下的 report-c.txt（小、新）、
/// /tmp/a 下的文件夹 report-a。
fn database_with_reports() -> SearchDatabase {
    let database = SearchDatabase::in_memory().unwrap();
    for file in [
        indexed("/tmp/b/report-b.txt", SearchFileKind::File, 900, 10),
        indexed("/tmp/a/report-c.txt", SearchFileKind::File, 100, 30),
        indexed("/tmp/a/report-a", SearchFileKind::Directory, 0, 20),
    ] {
        database.upsert_file(&file).unwrap();
    }
    database
}

fn ordered(
    terms: &str,
    field: SearchSortField,
    direction: SearchSortDirection,
    directories_first: bool,
) -> SearchQuery {
    let mut query = SearchQuery::global(1, terms);
    query.order = SearchOrder::Field {
        field,
        direction,
        directories_first,
    };
    query
}

/// 一页两条翻到底，返回全部命中的名字。
fn all_names(database: &SearchDatabase, mut query: SearchQuery) -> Vec<String> {
    let query_state = QueryState::default();
    query.limit = 2;
    let mut names = Vec::new();
    loop {
        let page = database.search(&query, &query_state).unwrap();
        names.extend(page.hits.into_iter().map(|hit| hit.display_name));
        let Some(cursor) = page.next_cursor else {
            return names;
        };
        assert!(matches!(cursor, SearchCursor::Snapshot { .. }));
        query.query_id += 1;
        query.cursor = Some(cursor);
    }
}

#[test]
fn every_field_orders_all_results_and_pages_through_them() {
    let database = database_with_reports();
    let cases = [
        (
            SearchSortField::Name,
            ["report-a", "report-b.txt", "report-c.txt"],
        ),
        (
            SearchSortField::Size,
            ["report-a", "report-c.txt", "report-b.txt"],
        ),
        (
            SearchSortField::Modified,
            ["report-b.txt", "report-a", "report-c.txt"],
        ),
        (
            SearchSortField::Kind,
            ["report-a", "report-b.txt", "report-c.txt"],
        ),
        (
            SearchSortField::Location,
            ["report-a", "report-c.txt", "report-b.txt"],
        ),
    ];
    for (field, expected) in cases {
        let ascending = all_names(
            &database,
            ordered("report", field, SearchSortDirection::Ascending, false),
        );
        assert_eq!(ascending, expected, "{field:?} ascending");
        let mut descending = all_names(
            &database,
            ordered("report", field, SearchSortDirection::Descending, false),
        );
        descending.reverse();
        assert_eq!(descending, expected, "{field:?} descending");
    }
}

#[test]
fn folders_stay_first_in_both_directions() {
    let database = database_with_reports();
    for direction in [
        SearchSortDirection::Ascending,
        SearchSortDirection::Descending,
    ] {
        let names = all_names(
            &database,
            ordered("report", SearchSortField::Size, direction, true),
        );
        assert_eq!(names[0], "report-a", "{direction:?}");
    }
}

#[test]
fn short_terms_and_no_terms_are_ordered_too() {
    let database = database_with_reports();
    assert_eq!(
        all_names(
            &database,
            ordered(
                "re",
                SearchSortField::Name,
                SearchSortDirection::Descending,
                false
            ),
        ),
        ["report-c.txt", "report-b.txt", "report-a"]
    );
    let mut in_folder = ordered(
        "",
        SearchSortField::Name,
        SearchSortDirection::Ascending,
        false,
    );
    in_folder.scope = SearchScope::Directory(Path::new("/tmp/a").to_path_buf());
    assert_eq!(
        all_names(&database, in_folder),
        ["report-a", "report-c.txt"]
    );
}

#[test]
fn regex_results_are_ordered_by_the_chosen_field() {
    let database = database_with_reports();
    let mut query = ordered(
        r"report-[bc]",
        SearchSortField::Modified,
        SearchSortDirection::Descending,
        false,
    );
    query.match_mode = SearchMatchMode::Regex;
    assert_eq!(
        all_names(&database, query),
        ["report-c.txt", "report-b.txt"]
    );
}

#[test]
fn a_different_order_is_a_different_snapshot() {
    let database = database_with_reports();
    let query_state = QueryState::default();
    let mut by_name = ordered(
        "report",
        SearchSortField::Name,
        SearchSortDirection::Ascending,
        false,
    );
    by_name.limit = 1;
    let cursor = database
        .search(&by_name, &query_state)
        .unwrap()
        .next_cursor
        .unwrap();

    let mut by_size = ordered(
        "report",
        SearchSortField::Size,
        SearchSortDirection::Ascending,
        false,
    );
    by_size.limit = 1;
    by_size.cursor = Some(cursor);
    assert!(database.search(&by_size, &query_state).is_err());
}
