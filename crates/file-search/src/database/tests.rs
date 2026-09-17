use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rusqlite::{params, Connection, ErrorCode};
use tempfile::tempdir;

use crate::extractor::ExtractionStatus;
use crate::model::{
    MatchSource, MimePattern, SearchEntryTypeRule, SearchFileKind, SearchMatchMode, SearchQuery,
    SearchScope, SearchTextScope, SizeRange, TimeRange,
};

use super::{
    path_from_storage_bytes, path_to_storage, DirectorySignature, DirectorySnapshot,
    EntryObservationState, EntryStageProgress, FileSignature, IndexedEntryStageState, IndexedFile,
    ObservedFile, SearchDatabase, READER_PAGE_CACHE_KIB, WAL_AUTOCHECKPOINT_PAGES,
    WRITER_PAGE_CACHE_KIB,
};

#[test]
fn searches_file_name_and_content() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file("/tmp/notes.txt", "notes.txt", "alpha body"))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/report.txt",
            "report.txt",
            "quarterly alpha",
        ))
        .unwrap();

    let batch = database.search(&SearchQuery::global(1, "alpha")).unwrap();

    assert_eq!(batch.hits.len(), 2);
    assert!(batch.finished);
}

#[test]
fn name_only_search_excludes_content_matches_and_marks_name_hits() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/needle.txt",
            "needle.txt",
            "unrelated body",
        ))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/report.txt",
            "report.txt",
            "needle in content",
        ))
        .unwrap();
    let mut query = SearchQuery::global(1, "needle");
    query.text_scope = SearchTextScope::NameOnly;

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/needle.txt"));
    assert_eq!(batch.hits[0].match_source, MatchSource::Name);
    assert!(batch.hits[0].snippet.is_none());
}

#[test]
fn native_rank_matches_bm25_across_pages_without_temporary_sorting() {
    fn assert_native_rank_plan(plan: &[String]) {
        assert!(
            plan.iter()
                .any(|step| step.contains("file_search_fts VIRTUAL TABLE INDEX 32:")),
            "FTS5 did not consume native rank ordering: {plan:#?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.contains("USE TEMP B-TREE FOR ORDER BY")),
            "native rank query created a temporary ORDER BY: {plan:#?}"
        );
        assert!(
            !plan.iter().any(|step| step.contains("MATERIALIZE ranked")),
            "native rank query still materialized an intermediate window: {plan:#?}"
        );
    }

    fn explicit_bm25_hits(database: &SearchDatabase) -> Vec<(PathBuf, u64)> {
        let mut statement = database
            .connection
            .prepare(
                "SELECT f.path, bm25(file_search_fts) AS score
                 FROM file_search_fts
                 JOIN files f ON f.rowid = file_search_fts.rowid
                 WHERE file_search_fts MATCH ?1
                   AND f.tombstoned = 0
                   AND f.observation_state = 'observable'
                 ORDER BY score",
            )
            .unwrap();
        statement
            .query_map(["\"needle\""], |row| {
                let path: Vec<u8> = row.get(0)?;
                let score: f64 = row.get(1)?;
                Ok((path_from_storage_bytes(path), (-score).to_bits()))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn searched_hits(database: &SearchDatabase) -> Vec<(PathBuf, u64)> {
        let mut query = SearchQuery::global(1, "needle");
        query.limit = 2;
        let mut hits = Vec::new();
        loop {
            let batch = database.search(&query).unwrap();
            hits.extend(
                batch
                    .hits
                    .into_iter()
                    .map(|hit| (hit.path, hit.rank.to_bits())),
            );
            let Some(cursor) = batch.next_cursor else {
                break;
            };
            query.cursor = Some(cursor);
        }
        hits
    }

    let database = SearchDatabase::in_memory().unwrap();
    for (path, name, content) in [
        ("/tmp/name.txt", "needle-name.txt", "brief body"),
        ("/tmp/dense.txt", "dense.txt", "needle needle needle"),
        (
            "/tmp/long.txt",
            "long.txt",
            "needle with a substantially longer document body for ranking",
        ),
        ("/tmp/tie-a.txt", "tie-a.txt", "same needle body"),
        ("/tmp/tie-b.txt", "tie-b.txt", "same needle body"),
    ] {
        database
            .upsert_file(&indexed_file(path, name, content))
            .unwrap();
    }
    let query = SearchQuery::global(1, "needle");
    let unfiltered_rank_plan = database.search_plan(&query).unwrap();
    assert_native_rank_plan(&unfiltered_rank_plan);
    assert_eq!(searched_hits(&database), explicit_bm25_hits(&database));

    let mut inaccessible = indexed_file(
        "/tmp/inaccessible.txt",
        "needle-inaccessible.txt",
        "needle needle needle needle",
    );
    database.upsert_file(&inaccessible).unwrap();
    inaccessible.content = None;
    inaccessible.extraction_status = ExtractionStatus::ReadFailed {
        message: "permission denied".to_owned(),
    };
    database.upsert_inaccessible_file(&inaccessible).unwrap();
    let metadata_filter_plan = database.search_plan(&query).unwrap();
    assert_native_rank_plan(&metadata_filter_plan);
    assert_eq!(searched_hits(&database), explicit_bm25_hits(&database));
}

#[test]
fn sqlite_connections_apply_the_fixed_memory_budget() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let writer = SearchDatabase::open(&database_path).unwrap();
    assert_eq!(
        writer
            .connection
            .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        -WRITER_PAGE_CACHE_KIB
    );
    assert_eq!(
        writer
            .connection
            .query_row("PRAGMA mmap_size", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        writer
            .connection
            .query_row("PRAGMA temp_store", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        writer
            .connection
            .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        WAL_AUTOCHECKPOINT_PAGES
    );

    let reader = SearchDatabase::open_read_only(&database_path).unwrap();
    assert_eq!(
        reader
            .connection
            .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        -READER_PAGE_CACHE_KIB
    );
    assert_eq!(
        reader
            .connection
            .query_row("PRAGMA mmap_size", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        reader
            .connection
            .query_row("PRAGMA temp_store", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn hidden_query_rows_use_the_partial_visibility_index() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut statement = database
        .connection
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT EXISTS(
                SELECT 1 FROM files
                WHERE tombstoned <> 0 OR observation_state <> 'observable'
             )",
        )
        .unwrap();
    let plan = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(plan
        .iter()
        .any(|step| step.contains("files_hidden_query_rows")));
}

#[test]
fn search_hits_expose_only_content_snippets() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/alpha-beta-name.txt",
            "alpha-beta-name.txt",
            "other content",
        ))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/content-match.txt",
            "content-match.txt",
            "prefix alpha\nneedle <tag> suffix",
        ))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/alpha-mixed.txt",
            "alpha-mixed.txt",
            "beta body",
        ))
        .unwrap();

    let name_hit = database
        .search(&SearchQuery::global(1, "alpha beta"))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/alpha-beta-name.txt"))
        .unwrap();
    assert_eq!(name_hit.match_source, MatchSource::Name);
    assert!(name_hit.snippet.is_none());

    let content_hit = database
        .search(&SearchQuery::global(2, "needle"))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/content-match.txt"))
        .unwrap();
    assert_eq!(content_hit.match_source, MatchSource::Content);
    let snippet = content_hit.snippet.unwrap();
    assert!(snippet.contains("needle"));
    assert!(!snippet.chars().any(char::is_control));
    assert!(snippet.chars().count() <= 240);

    let mixed_hit = database
        .search(&SearchQuery::global(3, "alpha beta"))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/alpha-mixed.txt"))
        .unwrap();
    assert_eq!(mixed_hit.match_source, MatchSource::Content);
    assert!(mixed_hit
        .snippet
        .as_deref()
        .is_some_and(|snippet| snippet.contains("beta")));

    let metadata_hit = database
        .search(&SearchQuery::global(4, ""))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/content-match.txt"))
        .unwrap();
    assert_eq!(metadata_hit.match_source, MatchSource::Metadata);
    assert!(metadata_hit.snippet.is_none());
}

#[test]
fn content_hit_outside_bounded_preview_has_no_snippet() {
    let database = SearchDatabase::in_memory().unwrap();
    let content = format!("{}deepneedle", "prefix ".repeat(200));
    database
        .upsert_file(&indexed_file(
            "/tmp/deep-content.txt",
            "deep-content.txt",
            &content,
        ))
        .unwrap();

    let hit = database
        .search(&SearchQuery::global(1, "deepneedle"))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/deep-content.txt"))
        .unwrap();
    assert_eq!(hit.match_source, MatchSource::Content);
    assert!(hit.snippet.is_none());
    assert_eq!(
        database
            .connection
            .query_row(
                "SELECT length(preview) FROM file_search_snippets",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1_024
    );
}

#[test]
fn preview_substrings_do_not_masquerade_as_fulltext_tokens() {
    let database = SearchDatabase::in_memory().unwrap();
    let content = format!("cart {} art", "filler ".repeat(200));
    database
        .upsert_file(&indexed_file(
            "/tmp/token-boundary.txt",
            "token-boundary.txt",
            &content,
        ))
        .unwrap();

    let hit = database
        .search(&SearchQuery::global(1, "art"))
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.path == Path::new("/tmp/token-boundary.txt"))
        .unwrap();
    assert_eq!(hit.match_source, MatchSource::Content);
    assert!(hit.snippet.is_none());
}

#[test]
fn fulltext_replacement_and_scope_delete_keep_snippets_aligned() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/replaced.txt",
            "replaced.txt",
            "legacytoken body",
        ))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/replaced.txt",
            "replaced.txt",
            "replacementtoken body",
        ))
        .unwrap();

    assert_eq!(
        database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM file_search_fts WHERE file_search_fts MATCH 'legacytoken'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM file_search_fts WHERE file_search_fts MATCH 'replacementtoken'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        database
            .connection
            .query_row("SELECT preview FROM file_search_snippets", [], |row| row
                .get::<_, String>(
                0
            ),)
            .unwrap(),
        "replacementtoken body"
    );

    database
        .delete_scope(Path::new("/tmp/replaced.txt"))
        .unwrap();
    for table in ["files", "file_search_fts", "file_search_snippets"] {
        assert_eq!(
            database
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0,
            "table {table}"
        );
    }
}

#[test]
fn sql_snippet_gating_matches_public_match_sources() {
    let database = SearchDatabase::in_memory().unwrap();
    for (path, name, content) in [
        ("/tmp/name.txt", "ALPHA-beta-name.txt", "unrelated content"),
        ("/tmp/content.txt", "content.txt", "alpha BETA body"),
        ("/tmp/mixed.txt", "alpha-mixed.txt", "BETA body"),
    ] {
        database
            .upsert_file(&indexed_file(path, name, content))
            .unwrap();
    }

    let query = SearchQuery::global(1, "alpha beta");
    let projected_snippets = database.projected_content_snippets(&query).unwrap();
    let batch = database.search(&query).unwrap();
    for (path, expected_source) in [
        (Path::new("/tmp/name.txt"), MatchSource::Name),
        (Path::new("/tmp/content.txt"), MatchSource::Content),
        (Path::new("/tmp/mixed.txt"), MatchSource::Content),
    ] {
        let raw_snippet = projected_snippets
            .iter()
            .find(|(candidate, _)| candidate == path)
            .unwrap()
            .1
            .as_ref();
        let hit = batch.hits.iter().find(|hit| hit.path == path).unwrap();
        assert_eq!(hit.match_source, expected_source);
        assert_eq!(
            raw_snippet.is_some(),
            expected_source == MatchSource::Content
        );
        assert_eq!(
            hit.snippet.is_some(),
            expected_source == MatchSource::Content
        );
    }

    let mut name_only_query = SearchQuery::global(2, "alpha BETA");
    name_only_query.text_scope = SearchTextScope::NameOnly;
    let projected_snippets = database
        .projected_content_snippets(&name_only_query)
        .unwrap();
    let batch = database.search(&name_only_query).unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].match_source, MatchSource::Name);
    assert!(batch.hits[0].snippet.is_none());
    assert_eq!(
        projected_snippets,
        vec![(Path::new("/tmp/name.txt").to_path_buf(), None)]
    );
}

#[test]
fn sqlite_interrupt_stops_a_long_statement_and_leaves_the_reader_reusable() {
    let database = SearchDatabase::in_memory().unwrap();
    let interrupt = database.interrupt_handle();
    let (statement_started_sender, statement_started_receiver) = mpsc::channel();
    let query_thread = thread::spawn(move || {
        statement_started_sender.send(()).unwrap();
        let query_outcome = database.connection.query_row(
            "WITH RECURSIVE counter(value) AS (
                VALUES(0)
                UNION ALL
                SELECT value + 1 FROM counter WHERE value < 1000000000
             )
             SELECT sum(value) FROM counter",
            [],
            |row| row.get::<_, i64>(0),
        );
        (database, query_outcome)
    });

    statement_started_receiver.recv().unwrap();
    thread::sleep(Duration::from_millis(10));
    interrupt.interrupt();
    let (database, query_outcome) = query_thread.join().unwrap();

    assert!(matches!(
        query_outcome,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == ErrorCode::OperationInterrupted
    ));
    assert_eq!(
        database
            .connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn filters_by_directory_and_kind() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file("/tmp/a/notes.txt", "notes.txt", "alpha"))
        .unwrap();
    database
        .upsert_file(&indexed_file("/tmp/b/notes.txt", "notes.txt", "alpha"))
        .unwrap();
    let mut query = SearchQuery::global(1, "alpha");
    query.scope = SearchScope::Directory(Path::new("/tmp/a").to_path_buf());
    query.filters.entry_type_rules = vec![SearchEntryTypeRule::Kind(SearchFileKind::File)];

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/a/notes.txt"));
}

#[test]
fn recursive_scope_treats_wildcards_and_case_as_literal_path_components() {
    let database = SearchDatabase::in_memory().unwrap();
    for path in [
        "/tmp/a%/inside-percent.txt",
        "/tmp/ax/outside-percent.txt",
        "/tmp/a_/inside-underscore.txt",
        "/tmp/ab/outside-underscore.txt",
        "/tmp/Case/inside-case.txt",
        "/tmp/case/outside-case.txt",
    ] {
        database
            .upsert_file(&indexed_file(
                path,
                Path::new(path).file_name().unwrap().to_str().unwrap(),
                "alpha",
            ))
            .unwrap();
    }

    for (scope, expected_path) in [
        ("/tmp/a%", "/tmp/a%/inside-percent.txt"),
        ("/tmp/a_", "/tmp/a_/inside-underscore.txt"),
        ("/tmp/Case", "/tmp/Case/inside-case.txt"),
    ] {
        let mut query = SearchQuery::global(1, "alpha");
        query.scope = SearchScope::Directory(Path::new(scope).to_path_buf());
        let batch = database.search(&query).unwrap();
        assert_eq!(batch.hits.len(), 1, "scope {scope}");
        assert_eq!(batch.hits[0].path, Path::new(expected_path));
    }
}

#[test]
fn recursive_scope_includes_only_the_directory_and_its_descendants() {
    let database = SearchDatabase::in_memory().unwrap();
    for path in [
        "/tmp/范围",
        "/tmp/范围/inside.txt",
        "/tmp/范围/deeper/nested.txt",
        "/tmp/范围-other/sibling.txt",
        "/tmp/范围0/other-prefix.txt",
    ] {
        database
            .upsert_file(&indexed_file(
                path,
                Path::new(path).file_name().unwrap().to_str().unwrap(),
                "alpha",
            ))
            .unwrap();
    }

    let mut directory_query = SearchQuery::global(1, "");
    directory_query.scope = SearchScope::Directory(Path::new("/tmp/范围").to_path_buf());
    let directory_paths = database
        .search(&directory_query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        directory_paths,
        [
            Path::new("/tmp/范围").to_path_buf(),
            Path::new("/tmp/范围/inside.txt").to_path_buf(),
            Path::new("/tmp/范围/deeper/nested.txt").to_path_buf(),
        ]
        .into_iter()
        .collect()
    );

    let mut root_query = SearchQuery::global(2, "");
    root_query.scope = SearchScope::Directory(Path::new("/").to_path_buf());
    assert_eq!(database.search(&root_query).unwrap().hits.len(), 5);
}

#[test]
fn empty_recursive_scope_materializes_path_candidates_before_ordering() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut query = SearchQuery::global(1, "");
    query.scope = SearchScope::Directory(Path::new("/workspace").to_path_buf());

    let plan = database.search_plan(&query).unwrap();

    assert!(plan.iter().any(|step| step.contains("LIST SUBQUERY")));
    assert!(plan
        .iter()
        .any(|step| step.contains("SEARCH files") && step.contains("path>? AND path<?")));
    assert!(plan
        .iter()
        .any(|step| step.contains("files_visible_modified_name")));
    assert!(!plan.iter().any(|step| step.starts_with("SCAN f")));
}

#[test]
fn filters_by_time_range() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut old = indexed_file("/tmp/old.txt", "old.txt", "alpha");
    old.modified_ms = Some(10);
    let mut new = indexed_file("/tmp/new.txt", "new.txt", "alpha");
    new.modified_ms = Some(30);
    let mut missing = indexed_file("/tmp/missing.txt", "missing.txt", "alpha");
    missing.modified_ms = None;
    database.upsert_file(&old).unwrap();
    database.upsert_file(&new).unwrap();
    database.upsert_file(&missing).unwrap();
    let mut query = SearchQuery::global(1, "alpha");
    query.filters.modified = Some(TimeRange {
        start_ms: 20,
        end_ms: 40,
    });

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/new.txt"));
}

#[test]
fn entry_type_rules_are_or_with_each_other_and_and_with_other_filters() {
    let database = SearchDatabase::in_memory().unwrap();
    for (path, mime_type, modified_ms, kind) in [
        ("/tmp/image.png", "image/png", 30, SearchFileKind::File),
        ("/tmp/vector.svg", "image/svg+xml", 10, SearchFileKind::File),
        (
            "/tmp/report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            35,
            SearchFileKind::File,
        ),
        (
            "/tmp/image-directory",
            "image/png",
            30,
            SearchFileKind::Directory,
        ),
    ] {
        let mut file = indexed_file(
            path,
            Path::new(path).file_name().unwrap().to_str().unwrap(),
            "",
        );
        file.mime_type = Some(mime_type.to_owned());
        file.modified_ms = Some(modified_ms);
        file.kind = kind;
        database.upsert_file(&file).unwrap();
    }
    let mut query = SearchQuery::global(1, "");
    query.filters.entry_type_rules = vec![
        SearchEntryTypeRule::Kind(SearchFileKind::Directory),
        SearchEntryTypeRule::Mime(MimePattern::Prefix("image/".to_owned())),
        SearchEntryTypeRule::Mime(MimePattern::Prefix(
            "application/vnd.openxmlformats-officedocument.".to_owned(),
        )),
    ];
    query.filters.modified = Some(TimeRange {
        start_ms: 20,
        end_ms: 40,
    });

    let paths = database
        .search(&query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        paths,
        BTreeSet::from([
            Path::new("/tmp/image.png").to_path_buf(),
            Path::new("/tmp/image-directory").to_path_buf(),
            Path::new("/tmp/report.docx").to_path_buf(),
        ])
    );
}

#[test]
fn extension_filters_or_within_themselves_and_exclude_directories() {
    let database = SearchDatabase::in_memory().unwrap();
    for (path, display_name, kind) in [
        ("/tmp/report.PDF", "report.PDF", SearchFileKind::File),
        ("/tmp/notes.Md", "notes.Md", SearchFileKind::File),
        ("/tmp/photo.jpg", "photo.jpg", SearchFileKind::File),
        (
            "/tmp/archive.tar.gz",
            "archive.tar.gz",
            SearchFileKind::File,
        ),
        ("/tmp/folder.pdf", "folder.pdf", SearchFileKind::Directory),
    ] {
        let mut file = indexed_file(path, display_name, "");
        file.kind = kind;
        database.upsert_file(&file).unwrap();
    }
    let mut query = SearchQuery::global(1, "");
    query.filters.extensions = vec!["pdf".to_owned(), "md".to_owned()];

    let paths = database
        .search(&query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        paths,
        BTreeSet::from([
            Path::new("/tmp/report.PDF").to_path_buf(),
            Path::new("/tmp/notes.Md").to_path_buf(),
        ])
    );
}

#[test]
fn extension_filters_and_with_text_terms_and_time_ranges() {
    let database = SearchDatabase::in_memory().unwrap();
    for (path, content, modified_ms) in [
        ("/tmp/match.pdf", "alpha", 30),
        ("/tmp/wrong-ext.pdf", "beta", 30),
        ("/tmp/wrong-time.txt", "alpha", 10),
    ] {
        let mut file = indexed_file(
            path,
            Path::new(path).file_name().unwrap().to_str().unwrap(),
            content,
        );
        file.modified_ms = Some(modified_ms);
        database.upsert_file(&file).unwrap();
    }
    let mut query = SearchQuery::global(1, "alpha");
    query.filters.extensions = vec!["pdf".to_owned()];
    query.filters.modified = Some(TimeRange {
        start_ms: 20,
        end_ms: 40,
    });

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/match.pdf"));
}

#[test]
fn size_filters_apply_half_open_bounds_and_exempt_directories() {
    let database = SearchDatabase::in_memory().unwrap();
    let kb = 1024_u64;
    let mb = kb * kb;
    let gb = mb * kb;
    for (size, name) in [
        (0, "zero.bin"),
        (1, "one.bin"),
        (16 * kb - 1, "tiny-last.bin"),
        (16 * kb, "small-first.bin"),
        (mb - 1, "small-last.bin"),
        (mb, "medium-first.bin"),
        (128 * mb - 1, "medium-last.bin"),
        (128 * mb, "large-first.bin"),
        (gb - 1, "large-last.bin"),
        (gb, "huge-first.bin"),
        (4 * gb - 1, "huge-last.bin"),
        (4 * gb, "gigantic-first.bin"),
        (4 * gb + 1, "gigantic-last.bin"),
    ] {
        let mut file = indexed_file(&format!("/tmp/{name}"), name, "");
        file.size = size;
        database.upsert_file(&file).unwrap();
    }
    // 目录 size 为 0：任何档位（包括 >= 4 GB）都不得把它排除。
    let mut directory = indexed_file("/tmp/size-folder", "size-folder", "");
    directory.kind = SearchFileKind::Directory;
    database.upsert_file(&directory).unwrap();

    let preset_hits = [
        (
            SizeRange {
                min_bytes: 0,
                max_bytes: Some(1),
            },
            vec!["zero.bin"],
        ),
        (
            SizeRange {
                min_bytes: 0,
                max_bytes: Some(16 * kb),
            },
            vec!["tiny-last.bin", "one.bin", "zero.bin"],
        ),
        (
            SizeRange {
                min_bytes: 16 * kb,
                max_bytes: Some(mb),
            },
            vec!["small-first.bin", "small-last.bin"],
        ),
        (
            SizeRange {
                min_bytes: mb,
                max_bytes: Some(128 * mb),
            },
            vec!["medium-first.bin", "medium-last.bin"],
        ),
        (
            SizeRange {
                min_bytes: 128 * mb,
                max_bytes: Some(gb),
            },
            vec!["large-first.bin", "large-last.bin"],
        ),
        (
            SizeRange {
                min_bytes: gb,
                max_bytes: Some(4 * gb),
            },
            vec!["huge-first.bin", "huge-last.bin"],
        ),
        (
            SizeRange {
                min_bytes: 4 * gb,
                max_bytes: None,
            },
            vec!["gigantic-first.bin", "gigantic-last.bin"],
        ),
    ];
    for (range, expected_files) in preset_hits {
        let mut query = SearchQuery::global(1, "");
        query.filters.size = Some(range);

        let mut names = database
            .search(&query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.display_name)
            .collect::<BTreeSet<_>>();
        // 目录永不被大小条件排除。
        assert!(
            names.remove("size-folder"),
            "directory exempted for {range:?}"
        );
        assert_eq!(
            names,
            expected_files
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "unexpected hits for {range:?}"
        );
    }
}

#[test]
fn size_filter_combines_with_terms_extensions_and_directories_stay_exempt() {
    let database = SearchDatabase::in_memory().unwrap();
    for (name, content, kind) in [
        ("match.pdf", "alpha", SearchFileKind::File),
        ("wrong-size.pdf", "", SearchFileKind::File),
        ("wrong-ext.txt", "alpha", SearchFileKind::File),
    ] {
        let mut file = indexed_file(&format!("/tmp/{name}"), name, content);
        file.kind = kind;
        database.upsert_file(&file).unwrap();
    }
    let mut query = SearchQuery::global(1, "alpha");
    query.filters.extensions = vec!["pdf".to_owned()];
    query.filters.size = Some(SizeRange {
        min_bytes: 3,
        max_bytes: None,
    });

    let names = database
        .search(&query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.display_name)
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["match.pdf".to_owned()]);
}

#[test]
fn search_window_reports_more_rows_only_when_an_extra_row_exists() {
    fn search_count(file_count: usize) -> crate::model::SearchResultBatch {
        let database = SearchDatabase::in_memory().unwrap();
        for index in 0..file_count {
            database
                .upsert_file(&indexed_file(
                    &format!("/tmp/needle-{index:03}.txt"),
                    &format!("needle-{index:03}.txt"),
                    "needle",
                ))
                .unwrap();
        }
        let mut query = SearchQuery::global(1, "needle");
        query.limit = 100;
        database.search(&query).unwrap()
    }

    let below_window = search_count(99);
    assert_eq!(below_window.hits.len(), 99);
    assert!(below_window.finished);
    assert!(below_window.next_cursor.is_none());

    let exact_window = search_count(100);
    assert_eq!(exact_window.hits.len(), 100);
    assert!(exact_window.finished);
    assert!(exact_window.next_cursor.is_none());

    let above_window = search_count(101);
    assert_eq!(above_window.hits.len(), 100);
    assert!(!above_window.finished);
    assert_eq!(above_window.next_cursor.unwrap().offset, 100);
}

fn indexed_file(path: &str, display_name: &str, content: &str) -> IndexedFile {
    let path = Path::new(path).to_path_buf();
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        path,
        display_name: display_name.to_owned(),
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

#[test]
fn stage_state_round_trips_separately_from_content_status() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut file = indexed_file("/tmp/too-large.txt", "too-large.txt", "");
    file.stage_state = IndexedEntryStageState {
        metadata: EntryStageProgress::Complete,
        content: EntryStageProgress::Skipped,
    };
    file.content = None;
    file.extraction_status = ExtractionStatus::TooLarge;

    database.upsert_file(&file).unwrap();

    assert_eq!(
        database
            .content_status(Path::new("/tmp/too-large.txt"))
            .unwrap(),
        Some(ExtractionStatus::TooLarge)
    );
    assert_eq!(
        database
            .entry_stage_state(Path::new("/tmp/too-large.txt"))
            .unwrap(),
        Some(file.stage_state)
    );
}

#[test]
fn inaccessible_content_stays_retained_hidden_and_retryable_until_recovery() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut file = indexed_file("/tmp/private.txt", "private.txt", "old needle");
    file.inode = Some(42);
    file.mtime_ns = Some(99);
    database.upsert_file(&file).unwrap();

    let mut inaccessible_file = file.clone();
    inaccessible_file.stage_state = IndexedEntryStageState {
        metadata: EntryStageProgress::Complete,
        content: EntryStageProgress::Pending,
    };
    inaccessible_file.content = None;
    inaccessible_file.extraction_status = ExtractionStatus::ReadFailed {
        message: "permission denied".to_owned(),
    };
    database
        .upsert_inaccessible_file(&inaccessible_file)
        .unwrap();

    assert!(database
        .search(&SearchQuery::global(1, "needle"))
        .unwrap()
        .hits
        .is_empty());
    assert_eq!(database.indexed_file_count().unwrap(), 0);
    let retained_fts_rows: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*)
             FROM file_search_fts
             JOIN files AS f ON f.rowid = file_search_fts.rowid
             WHERE f.path = ?1 AND file_search_fts MATCH 'needle'",
            [path_to_storage(Path::new("/tmp/private.txt"))],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_fts_rows, 1);

    let inaccessible_entry = database
        .classify_observed_files(&[ObservedFile {
            path: file.path.clone(),
            signature: FileSignature {
                device: Some(1),
                inode: Some(42),
                mtime_ns: Some(99),
                ctime_ns: Some(99),
                size: file.size,
            },
        }])
        .unwrap()
        .pop()
        .unwrap()
        .known_entry
        .unwrap();
    assert_eq!(
        inaccessible_entry.observation_state,
        EntryObservationState::Inaccessible
    );
    assert!(!inaccessible_entry.allows_signature_skip(
        FileSignature {
            device: Some(1),
            inode: Some(42),
            mtime_ns: Some(99),
            ctime_ns: Some(99),
            size: file.size,
        },
        Some("text/plain"),
    ));
    database.upsert_file(&file).unwrap();
    assert_eq!(
        database
            .search(&SearchQuery::global(2, "needle"))
            .unwrap()
            .hits
            .len(),
        1
    );
    assert_eq!(database.indexed_file_count().unwrap(), 1);
    let recovered_entry = database
        .classify_observed_files(&[ObservedFile {
            path: file.path,
            signature: FileSignature {
                device: Some(1),
                inode: Some(42),
                mtime_ns: Some(99),
                ctime_ns: Some(99),
                size: file.size,
            },
        }])
        .unwrap()
        .pop()
        .unwrap()
        .known_entry
        .unwrap();
    assert_eq!(
        recovered_entry.observation_state,
        EntryObservationState::Observable
    );
}

#[test]
fn inaccessible_subtree_is_hidden_without_removing_known_rows() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/private/note.txt",
            "note.txt",
            "private needle",
        ))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/public/note.txt",
            "note.txt",
            "public needle",
        ))
        .unwrap();

    database
        .mark_scope_inaccessible(Path::new("/tmp/private"))
        .unwrap();

    let batch = database.search(&SearchQuery::global(1, "needle")).unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/public/note.txt"));
    let retained_rows: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM files WHERE tombstoned = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_rows, 2);
    let private_state: String = database
        .connection
        .query_row(
            "SELECT observation_state FROM files WHERE path = ?1",
            params![path_to_storage(Path::new("/tmp/private/note.txt"))],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(private_state, "inaccessible");
}

#[test]
fn repeated_inaccessible_observation_does_not_rewrite_the_same_scope() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/private/note.txt",
            "note.txt",
            "private needle",
        ))
        .unwrap();

    database
        .mark_scope_inaccessible(Path::new("/tmp/private"))
        .unwrap();
    let changes_after_first_observation = database.connection.total_changes();

    database
        .mark_scope_inaccessible(Path::new("/tmp/private"))
        .unwrap();

    assert_eq!(
        database.connection.total_changes(),
        changes_after_first_observation
    );
}

#[test]
fn legacy_tombstones_are_recovered_once_without_changing_schema_version() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let file_path = Path::new("/tmp/legacy-tombstone.txt");
    let database = SearchDatabase::open(&database_path).unwrap();
    database
        .upsert_file(&indexed_file(
            file_path.to_str().unwrap(),
            "legacy-tombstone.txt",
            "legacy needle",
        ))
        .unwrap();
    drop(database);

    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "DELETE FROM search_data_migrations WHERE name = 'legacy_tombstone_recovery_v1'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE files SET tombstoned = 1 WHERE path = ?1",
            [path_to_storage(file_path)],
        )
        .unwrap();
    drop(connection);

    let migrated = SearchDatabase::open(&database_path).unwrap();
    assert_eq!(
        migrated
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        super::SCHEMA_VERSION
    );
    assert_eq!(
        migrated
            .connection
            .query_row(
                "SELECT COUNT(*) FROM search_data_migrations
                 WHERE name = 'legacy_tombstone_recovery_v1'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        migrated
            .connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        migrated
            .connection
            .query_row("SELECT COUNT(*) FROM file_stage_state", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        migrated
            .connection
            .query_row("SELECT COUNT(*) FROM file_search_fts", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn migration_backfills_stage_state_for_legacy_rows() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("search.sqlite");
    let legacy_status = serde_json::to_string(&ExtractionStatus::TooLarge).unwrap();

    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE files (
                path TEXT PRIMARY KEY,
                parent_path TEXT NOT NULL,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL,
                modified_ms INTEGER,
                accessed_ms INTEGER,
                created_ms INTEGER,
                mime_type TEXT,
                content_status TEXT NOT NULL,
                tombstoned INTEGER NOT NULL DEFAULT 0,
                inode INTEGER,
                mtime_ns INTEGER
            );
            CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, content);
            PRAGMA user_version = 1;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO files (
                path, parent_path, display_name, kind, size, modified_ms, accessed_ms,
                created_ms, mime_type, content_status, tombstoned, inode, mtime_ns
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12)",
            params![
                "/tmp/legacy.txt",
                "/tmp",
                "legacy.txt",
                SearchFileKind::File.as_storage_value(),
                12_i64,
                Some(1_i64),
                Option::<i64>::None,
                Option::<i64>::None,
                Some("text/plain".to_owned()),
                legacy_status,
                Some(7_i64),
                Some(99_i64),
            ],
        )
        .unwrap();
    drop(connection);

    let database = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(
        database
            .entry_stage_state(Path::new("/tmp/legacy.txt"))
            .unwrap(),
        Some(IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Skipped,
        })
    );
    let schema_version: i64 = database
        .connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema_version, super::SCHEMA_VERSION);
    let migrated_path_type: String = database
        .connection
        .query_row("SELECT typeof(path) FROM files LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(migrated_path_type, "blob");
    assert!(database.column_exists("files", "scan_generation").unwrap());
    assert!(database.column_exists("files", "device").unwrap());
    let legacy_scan_tables: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN ('index_scans', 'scan_scopes')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_scan_tables, 0);
}

#[test]
fn fts_rowid_migration_preserves_content_and_uses_files_row_identity() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/rowid.txt",
            "rowid.txt",
            "preserved needle",
        ))
        .unwrap();
    drop(database);

    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "DROP TABLE file_search_fts;
             DROP TABLE file_search_snippets;
             CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, content);
             UPDATE files SET path = CAST(path AS TEXT), parent_path = CAST(parent_path AS TEXT);
             UPDATE file_stage_state SET path = CAST(path AS TEXT);
             UPDATE directory_snapshots SET
                path = CAST(path AS TEXT),
                parent_path = CAST(parent_path AS TEXT),
                root_path = CAST(root_path AS TEXT);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, content)
             VALUES (1001, ?1, ?2, ?3)",
            params!["/tmp/rowid.txt", "rowid.txt", "preserved needle"],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 4).unwrap();
    drop(connection);

    let database = SearchDatabase::open(&database_path).unwrap();
    let (file_rowid, fts_rowid): (i64, i64) = database
        .connection
        .query_row(
            "SELECT f.rowid, x.rowid
             FROM files AS f JOIN file_search_fts AS x ON x.rowid = f.rowid",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(file_rowid, fts_rowid);
    assert_eq!(
        database
            .search(&SearchQuery::global(1, "needle"))
            .unwrap()
            .hits
            .len(),
        1
    );
}

#[test]
fn signatures_round_trip_and_detect_changes() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut file = indexed_file("/tmp/a.txt", "a.txt", "body");
    file.device = Some(7);
    file.inode = Some(42);
    file.mtime_ns = Some(1_000);
    file.ctime_ns = Some(1_000);
    database.upsert_file(&file).unwrap();

    let changes_before = database.connection.total_changes();
    let stored = database
        .classify_observed_files(&[ObservedFile {
            path: file.path.clone(),
            signature: FileSignature {
                device: Some(7),
                inode: Some(42),
                mtime_ns: Some(1_000),
                ctime_ns: Some(1_000),
                size: file.size,
            },
        }])
        .unwrap()
        .pop()
        .unwrap()
        .known_entry
        .expect("signature stored");
    assert_eq!(database.connection.total_changes(), changes_before);
    assert_eq!(
        stored.stage_state,
        IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        }
    );
    let signature = stored.signature;

    assert!(stored.allows_signature_skip(
        FileSignature {
            device: Some(7),
            inode: Some(42),
            mtime_ns: Some(1_000),
            ctime_ns: Some(1_000),
            size: file.size,
        },
        Some("text/plain"),
    ));
    assert!(!stored.allows_signature_skip(
        FileSignature {
            device: Some(7),
            inode: Some(42),
            mtime_ns: Some(2_000),
            ctime_ns: Some(1_000),
            size: file.size,
        },
        Some("text/plain"),
    ));
    assert!(!stored.allows_signature_skip(
        FileSignature {
            device: Some(7),
            inode: Some(42),
            mtime_ns: Some(1_000),
            ctime_ns: Some(2_000),
            size: file.size,
        },
        Some("text/plain"),
    ));
    assert!(!stored.allows_signature_skip(
        FileSignature {
            device: Some(8),
            inode: Some(42),
            mtime_ns: Some(1_000),
            ctime_ns: Some(1_000),
            size: file.size,
        },
        Some("text/plain"),
    ));
    assert!(!stored.allows_signature_skip(
        FileSignature {
            device: Some(7),
            inode: Some(42),
            mtime_ns: Some(1_000),
            ctime_ns: Some(1_000),
            size: file.size,
        },
        Some("image/png"),
    ));
    assert_eq!(signature.device, Some(7));
    assert_eq!(signature.inode, Some(42));
}

#[test]
fn recursive_root_pages_return_each_exact_path_once() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut root_file = indexed_file("/root-placeholder", "/", "root");
    root_file.path = Path::new("/").to_path_buf();
    root_file.parent_path = Path::new("/").to_path_buf();
    database.upsert_file(&root_file).unwrap();
    database
        .upsert_file(&indexed_file("/child.txt", "child.txt", "child"))
        .unwrap();
    for (path, parent_path, inode) in [("/", "/", 10), ("/child", "/", 11)] {
        database
            .upsert_directory_snapshot(&DirectorySnapshot {
                path: Path::new(path).to_path_buf(),
                parent_path: Path::new(parent_path).to_path_buf(),
                root_path: Path::new("/").to_path_buf(),
                signature: DirectorySignature {
                    device: 1,
                    inode,
                    mtime_ns: 20,
                    ctime_ns: 30,
                },
                observation_state: EntryObservationState::Observable,
            })
            .unwrap();
    }

    let first_file_page = database.known_files_page(Path::new("/"), None, 1).unwrap();
    let second_file_page = database
        .known_files_page(Path::new("/"), Some(&first_file_page[0].path), 1)
        .unwrap();
    let final_file_page = database
        .known_files_page(Path::new("/"), Some(&second_file_page[0].path), 1)
        .unwrap();
    assert_eq!(first_file_page[0].path, Path::new("/"));
    assert_eq!(second_file_page[0].path, Path::new("/child.txt"));
    assert!(final_file_page.is_empty());

    let first_snapshot_page = database
        .directory_snapshots_page(Path::new("/"), None, 1)
        .unwrap();
    let second_snapshot_page = database
        .directory_snapshots_page(Path::new("/"), Some(&first_snapshot_page[0].path), 1)
        .unwrap();
    let final_snapshot_page = database
        .directory_snapshots_page(Path::new("/"), Some(&second_snapshot_page[0].path), 1)
        .unwrap();
    assert_eq!(first_snapshot_page[0].path, Path::new("/"));
    assert_eq!(second_snapshot_page[0].path, Path::new("/child"));
    assert!(final_snapshot_page.is_empty());
}

#[test]
fn directory_snapshots_page_stably_and_local_delete_cleans_all_rows() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/root/removed/note.txt",
            "note.txt",
            "needle",
        ))
        .unwrap();
    database
        .upsert_directory_snapshot(&DirectorySnapshot {
            path: Path::new("/tmp/root").to_path_buf(),
            parent_path: Path::new("/tmp").to_path_buf(),
            root_path: Path::new("/tmp/root").to_path_buf(),
            signature: DirectorySignature {
                device: 1,
                inode: 10,
                mtime_ns: 20,
                ctime_ns: 30,
            },
            observation_state: EntryObservationState::Observable,
        })
        .unwrap();
    database
        .upsert_directory_snapshot(&DirectorySnapshot {
            path: Path::new("/tmp/root/removed").to_path_buf(),
            parent_path: Path::new("/tmp/root").to_path_buf(),
            root_path: Path::new("/tmp/root").to_path_buf(),
            signature: DirectorySignature {
                device: 1,
                inode: 11,
                mtime_ns: 21,
                ctime_ns: 31,
            },
            observation_state: EntryObservationState::Observable,
        })
        .unwrap();

    let first_page = database
        .directory_snapshots_page(Path::new("/tmp/root"), None, 1)
        .unwrap();
    assert_eq!(first_page.len(), 1);
    let second_page = database
        .directory_snapshots_page(Path::new("/tmp/root"), Some(&first_page[0].path), 1)
        .unwrap();
    assert_eq!(second_page.len(), 1);

    database
        .delete_scope(Path::new("/tmp/root/removed"))
        .unwrap();

    assert!(database
        .search(&SearchQuery::global(1, "needle"))
        .unwrap()
        .hits
        .is_empty());
    assert_eq!(database.directory_snapshot_count().unwrap(), 1);
}

#[test]
fn known_file_pages_skip_inaccessible_directories_without_hiding_retryable_files() {
    let database = SearchDatabase::in_memory().unwrap();
    for path in ["/tmp/root/private/blocked.txt", "/tmp/root/retryable.txt"] {
        database
            .upsert_file(&indexed_file(
                path,
                Path::new(path).file_name().unwrap().to_str().unwrap(),
                "needle",
            ))
            .unwrap();
    }
    for (path, parent_path, inode) in [
        ("/tmp/root", "/tmp", 10),
        ("/tmp/root/private", "/tmp/root", 11),
    ] {
        database
            .upsert_directory_snapshot(&DirectorySnapshot {
                path: Path::new(path).to_path_buf(),
                parent_path: Path::new(parent_path).to_path_buf(),
                root_path: Path::new("/tmp/root").to_path_buf(),
                signature: DirectorySignature {
                    device: 1,
                    inode,
                    mtime_ns: 20,
                    ctime_ns: 30,
                },
                observation_state: EntryObservationState::Observable,
            })
            .unwrap();
    }
    database
        .mark_scope_inaccessible(Path::new("/tmp/root/private"))
        .unwrap();
    database
        .mark_scope_inaccessible(Path::new("/tmp/root/retryable.txt"))
        .unwrap();

    let known_files = database
        .known_files_page(Path::new("/tmp/root"), None, 128)
        .unwrap();

    assert_eq!(known_files.len(), 1);
    assert_eq!(known_files[0].path, Path::new("/tmp/root/retryable.txt"));
    assert_eq!(
        known_files[0].state.observation_state,
        EntryObservationState::Inaccessible
    );
}

#[test]
fn policy_exclusion_deletes_a_previously_inaccessible_scope() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/private/blocked.txt",
            "blocked.txt",
            "needle",
        ))
        .unwrap();

    database
        .mark_scope_inaccessible(Path::new("/tmp/private"))
        .unwrap();
    database.delete_scope(Path::new("/tmp/private")).unwrap();

    let retained_file_rows: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            params![path_to_storage(Path::new("/tmp/private/blocked.txt"))],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_file_rows, 0);
}

#[test]
fn classification_rejects_batches_above_the_fixed_capacity() {
    let database = SearchDatabase::in_memory().unwrap();
    let observations = (0..=super::MAX_CLASSIFICATION_BATCH_ENTRIES)
        .map(|index| ObservedFile {
            path: Path::new("/tmp").join(format!("file-{index}.txt")),
            signature: FileSignature {
                device: Some(1),
                inode: Some(index as u64),
                mtime_ns: Some(index as i64),
                ctime_ns: Some(index as i64),
                size: 0,
            },
        })
        .collect::<Vec<_>>();

    let error = database.classify_observed_files(&observations).unwrap_err();
    assert!(matches!(error, crate::error::SearchError::InvalidQuery(_)));
}

#[test]
fn regex_mode_matches_names_without_touching_content() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file("/tmp/notes.txt", "notes.txt", "alpha body"))
        .unwrap();
    database
        .upsert_file(&indexed_file(
            "/tmp/alpha-report.txt",
            "alpha-report.txt",
            "unrelated",
        ))
        .unwrap();
    let mut query = SearchQuery::global(1, "^alpha-\\w+\\.txt$");
    query.match_mode = SearchMatchMode::Regex;
    query.text_scope = SearchTextScope::NameAndContent;

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/alpha-report.txt"));
    assert_eq!(batch.hits[0].match_source, MatchSource::Name);
    assert!(batch.hits[0].snippet.is_none());
    assert_eq!(batch.hits[0].rank, 0.0);
}

#[test]
fn regex_mode_defaults_to_case_insensitive_and_honors_inline_overrides() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file("/tmp/Notes.TXT", "Notes.TXT", ""))
        .unwrap();
    database
        .upsert_file(&indexed_file("/tmp/exact.md", "exact.md", ""))
        .unwrap();

    let mut default_query = SearchQuery::global(1, "^notes\\.txt$");
    default_query.match_mode = SearchMatchMode::Regex;
    assert_eq!(database.search(&default_query).unwrap().hits.len(), 1);

    let mut sensitive_query = SearchQuery::global(2, "(?-i)^notes\\.txt$");
    sensitive_query.match_mode = SearchMatchMode::Regex;
    assert_eq!(database.search(&sensitive_query).unwrap().hits.len(), 0);

    let mut sensitive_hit_query = SearchQuery::global(3, "(?-i)^Notes\\.TXT$");
    sensitive_hit_query.match_mode = SearchMatchMode::Regex;
    assert_eq!(database.search(&sensitive_hit_query).unwrap().hits.len(), 1);
}

#[test]
fn regex_mode_rejects_invalid_patterns_as_invalid_query() {
    let database = SearchDatabase::in_memory().unwrap();
    let mut query = SearchQuery::global(1, "foo(");
    query.match_mode = SearchMatchMode::Regex;

    let error = database.search(&query).unwrap_err();
    assert!(matches!(error, crate::error::SearchError::InvalidQuery(_)));
}

#[test]
fn regex_mode_pages_in_scan_order_without_duplicates_or_gaps() {
    let database = SearchDatabase::in_memory().unwrap();
    for index in 0..5 {
        let name = format!("log-{index:02}.txt");
        database
            .upsert_file(&indexed_file(&format!("/tmp/{name}"), &name, ""))
            .unwrap();
    }

    let mut first_page_query = SearchQuery::global(1, r"^log-\d+\.txt$");
    first_page_query.match_mode = SearchMatchMode::Regex;
    first_page_query.limit = 3;
    let first_page = database.search(&first_page_query).unwrap();
    assert_eq!(first_page.hits.len(), 3);
    assert!(!first_page.finished);

    let mut second_page_query = first_page_query.clone();
    second_page_query.query_id = 2;
    second_page_query.cursor = first_page.next_cursor;
    let second_page = database.search(&second_page_query).unwrap();
    assert_eq!(second_page.hits.len(), 2);
    assert!(second_page.finished);

    let mut paths = first_page
        .hits
        .iter()
        .chain(&second_page.hits)
        .map(|hit| hit.path.clone())
        .collect::<Vec<_>>();
    let unique_count = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(unique_count, paths.len());
    assert_eq!(paths.len(), 5);
}

#[test]
fn regex_mode_still_applies_directory_scope_and_filters() {
    let database = SearchDatabase::in_memory().unwrap();
    database
        .upsert_file(&indexed_file("/tmp/docs/log-1.txt", "log-1.txt", ""))
        .unwrap();
    database
        .upsert_file(&indexed_file("/tmp/other/log-2.txt", "log-2.txt", ""))
        .unwrap();
    let mut query = SearchQuery::global(1, r"^log-\d+\.txt$");
    query.match_mode = SearchMatchMode::Regex;
    query.scope = SearchScope::Directory(PathBuf::from("/tmp/docs"));

    let batch = database.search(&query).unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, Path::new("/tmp/docs/log-1.txt"));
}
