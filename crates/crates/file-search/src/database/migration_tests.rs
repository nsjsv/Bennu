use std::path::Path;

use rusqlite::{params, Connection};
use tempfile::tempdir;

use crate::extractor::ExtractionStatus;
use crate::model::{SearchFileKind, SearchQuery};

use super::{
    path_to_storage, EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase,
    SCHEMA_VERSION,
};

#[test]
fn path_migration_failure_rolls_back_every_rebuilt_table() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
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
                device INTEGER,
                inode INTEGER,
                mtime_ns INTEGER,
                ctime_ns INTEGER,
                observation_state TEXT NOT NULL DEFAULT 'observable'
                    CHECK (observation_state IN ('observable', 'inaccessible')),
                scan_generation INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE file_stage_state (
                path TEXT PRIMARY KEY,
                metadata_stage_state TEXT NOT NULL,
                content_stage_state TEXT NOT NULL
             );
             CREATE TABLE directory_snapshots (
                path TEXT PRIMARY KEY,
                parent_path TEXT NOT NULL,
                root_path TEXT NOT NULL,
                device INTEGER NOT NULL,
                inode INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL,
                ctime_ns INTEGER NOT NULL,
                observation_state TEXT NOT NULL DEFAULT 'observable'
                    CHECK (observation_state IN ('observable', 'inaccessible'))
             );
             CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, content);
             CREATE TABLE directory_snapshots_path_migration (sentinel INTEGER);
             PRAGMA user_version = 7;",
        )
        .unwrap();
    let content_status = serde_json::to_string(&ExtractionStatus::Indexed).unwrap();
    connection
        .execute(
            "INSERT INTO files (
                path, parent_path, display_name, kind, size, content_status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "/tmp/note.txt",
                "/tmp",
                "note.txt",
                "file",
                4_i64,
                content_status,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_stage_state
                (path, metadata_stage_state, content_stage_state)
             VALUES (?1, 'complete', 'complete')",
            ["/tmp/note.txt"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO directory_snapshots (
                path, parent_path, root_path, device, inode, mtime_ns, ctime_ns
             ) VALUES (?1, ?2, ?3, 1, 2, 3, 4)",
            ["/tmp", "/", "/tmp"],
        )
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());

    let connection = Connection::open(&database_path).unwrap();
    let schema_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema_version, 7);
    for (table, expected_path) in [
        ("files", "/tmp/note.txt"),
        ("file_stage_state", "/tmp/note.txt"),
        ("directory_snapshots", "/tmp"),
    ] {
        let (path_type, path): (String, String) = connection
            .query_row(
                &format!("SELECT typeof(path), path FROM {table} LIMIT 1"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(path_type, "text", "table {table}");
        assert_eq!(path, expected_path, "table {table}");
    }
    let partial_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table'
               AND name IN ('files_path_migration', 'file_stage_state_path_migration')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(partial_tables, 0);
}

fn create_schema_eight_database(database_path: &Path) -> i64 {
    let database = SearchDatabase::open(database_path).unwrap();
    let content_status = serde_json::to_string(&ExtractionStatus::Indexed).unwrap();
    database
        .connection
        .execute(
            "INSERT INTO files (
                path, parent_path, display_name, kind, size, content_status
             ) VALUES (?1, ?2, 'legacy.txt', 'file', 11, ?3)",
            params![
                path_to_storage(Path::new("/tmp/legacy.txt")),
                path_to_storage(Path::new("/tmp")),
                content_status,
            ],
        )
        .unwrap();
    let file_rowid = database.connection.last_insert_rowid();
    database
        .connection
        .execute_batch(
            "DROP TABLE file_search_fts;
             DROP TABLE file_search_snippets;
             CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, content);",
        )
        .unwrap();
    database
        .connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, content)
             VALUES(?1, '/tmp/legacy.txt', 'legacy.txt', 'preserved needle body')",
            [file_rowid],
        )
        .unwrap();
    database
        .connection
        .pragma_update(None, "user_version", 8)
        .unwrap();
    file_rowid
}

fn assert_schema_eight_fts_preserved(database_path: &Path, expected_rows: i64) {
    let connection = Connection::open(database_path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM file_search_fts
                 WHERE file_search_fts MATCH 'needle'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        expected_rows
    );
}

fn assert_no_migration_fts_table(database_path: &Path) {
    let connection = Connection::open(database_path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name = 'file_search_fts_v9'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn schema_eight_fulltext_migration_preserves_hits_and_builds_bounded_preview() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let expected_rowid = create_schema_eight_database(&database_path);

    let database = SearchDatabase::open(&database_path).unwrap();
    assert_eq!(
        database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        database
            .connection
            .query_row("SELECT rowid FROM file_search_fts", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        expected_rowid
    );
    assert_eq!(
        database
            .connection
            .query_row(
                "SELECT preview FROM file_search_snippets WHERE file_rowid = ?1",
                [expected_rowid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "preserved needle body"
    );
    let hits = database
        .search(&SearchQuery::global(1, "needle"))
        .unwrap()
        .hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, Path::new("/tmp/legacy.txt"));
}

#[test]
fn schema_ten_migration_adds_path_configuration_without_losing_search_rows() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    database
        .upsert_file(&IndexedFile {
            path: Path::new("/tmp/preserved.txt").to_path_buf(),
            parent_path: Path::new("/tmp").to_path_buf(),
            display_name: "preserved.txt".to_owned(),
            kind: SearchFileKind::File,
            size: 9,
            modified_ms: None,
            accessed_ms: None,
            created_ms: None,
            mime_type: Some("text/plain".to_owned()),
            stage_state: IndexedEntryStageState {
                metadata: EntryStageProgress::Complete,
                content: EntryStageProgress::Complete,
            },
            content: Some("schema ten needle".to_owned()),
            extraction_status: ExtractionStatus::Indexed,
            device: Some(1),
            inode: Some(2),
            mtime_ns: Some(3),
            ctime_ns: Some(4),
        })
        .unwrap();
    database
        .connection
        .execute_batch(
            "DROP TABLE search_root_mounts;
             DROP TABLE search_path_configuration;
             PRAGMA user_version = 9;",
        )
        .unwrap();
    drop(database);

    let database = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(
        database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        database
            .search(&SearchQuery::global(1, "needle"))
            .unwrap()
            .hits
            .len(),
        1
    );
    assert!(database.read_search_path_configuration().unwrap().is_none());
    assert!(database.read_search_root_mounts().unwrap().is_empty());
}

#[test]
fn schema_nine_create_failure_rolls_back_to_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    create_schema_eight_database(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch("CREATE TABLE file_search_fts_v9(sentinel INTEGER);")
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());
    assert_schema_eight_fts_preserved(&database_path, 1);
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = 'file_search_fts_v9'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "CREATE TABLE file_search_fts_v9(sentinel INTEGER)"
    );
}

#[test]
fn schema_nine_validation_failure_rolls_back_to_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    create_schema_eight_database(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, content)
             VALUES(999, '/tmp/orphan.txt', 'orphan.txt', 'needle orphan')",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());
    assert_schema_eight_fts_preserved(&database_path, 2);
    assert_no_migration_fts_table(&database_path);
}

#[test]
fn schema_nine_copy_failure_rolls_back_to_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let file_rowid = create_schema_eight_database(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "DROP TABLE file_search_fts;
             CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, body);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, body)
             VALUES(?1, '/tmp/legacy.txt', 'legacy.txt', 'preserved needle body')",
            [file_rowid],
        )
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        connection
            .query_row("SELECT body FROM file_search_fts", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "preserved needle body"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'file_search_fts_v9'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn schema_nine_replace_failure_rolls_back_to_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    create_schema_eight_database(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE file_search_snippets(
                file_rowid INTEGER PRIMARY KEY,
                preview TEXT NOT NULL
             );
             INSERT INTO file_search_snippets VALUES(77, 'sentinel');",
        )
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());
    assert_schema_eight_fts_preserved(&database_path, 1);
    assert_no_migration_fts_table(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT preview FROM file_search_snippets WHERE file_rowid = 77",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "sentinel"
    );
}

#[test]
fn schema_eleven_migration_resets_skipped_office_content_for_reextraction() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    let skipped_office_file = IndexedFile {
        path: Path::new("/tmp/report.docx").to_path_buf(),
        parent_path: Path::new("/tmp").to_path_buf(),
        display_name: "report.docx".to_owned(),
        kind: SearchFileKind::File,
        size: 9,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_owned(),
        ),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Skipped,
        },
        content: None,
        extraction_status: ExtractionStatus::ResourceBudgetExceeded {
            tool: "pandoc".to_owned(),
        },
        device: Some(1),
        inode: Some(2),
        mtime_ns: Some(3),
        ctime_ns: Some(4),
    };
    let skipped_plain_text_file = IndexedFile {
        path: Path::new("/tmp/legacy.txt").to_path_buf(),
        parent_path: Path::new("/tmp").to_path_buf(),
        display_name: "legacy.txt".to_owned(),
        kind: SearchFileKind::File,
        size: 4,
        modified_ms: None,
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Skipped,
        },
        content: None,
        extraction_status: ExtractionStatus::NonUtf8,
        device: Some(5),
        inode: Some(6),
        mtime_ns: Some(7),
        ctime_ns: Some(8),
    };
    database.upsert_file(&skipped_office_file).unwrap();
    database.upsert_file(&skipped_plain_text_file).unwrap();
    database
        .connection
        .execute_batch("PRAGMA user_version = 10;")
        .unwrap();
    drop(database);

    let database = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(
        database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        database
            .entry_stage_state(Path::new("/tmp/report.docx"))
            .unwrap()
            .expect("migrated office row must stay present")
            .content,
        EntryStageProgress::Pending,
        "office documents skipped by the old extractor must be re-extracted"
    );
    assert_eq!(
        database
            .entry_stage_state(Path::new("/tmp/legacy.txt"))
            .unwrap()
            .expect("control row must stay present")
            .content,
        EntryStageProgress::Skipped,
        "non-office skip reasons must not be disturbed by the migration"
    );
}
