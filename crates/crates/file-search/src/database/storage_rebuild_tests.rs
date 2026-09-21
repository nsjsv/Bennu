use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use tempfile::tempdir;

use crate::error::SearchError;
use crate::extractor::ExtractionStatus;

use super::{
    atomically_replace_database, create_replacement_database, open_committed_database,
    rebuild_schema_eight_database_with_operations, workspace_path, SchemaMigrationWorkspace,
    WORKSPACE_PENDING_MARKER_NAME,
};
use crate::database::{schema::BASE_SCHEMA, SearchDatabase, SCHEMA_VERSION};

#[derive(Clone)]
struct RankedQueryCase {
    match_expression: String,
    parent_path: Option<Vec<u8>>,
    mime_type: Option<String>,
    modified_at_least: Option<i64>,
}

fn ranked_query_pages(
    connection: &Connection,
    query: &RankedQueryCase,
) -> Vec<Vec<(Vec<u8>, u64)>> {
    let mut pages = Vec::new();
    let mut offset = 0_i64;
    loop {
        let mut statement = connection
            .prepare(
                "SELECT files.path, file_search_fts.rank
                 FROM file_search_fts
                 JOIN files ON files.rowid = file_search_fts.rowid
                 WHERE file_search_fts MATCH ?1
                   AND (?2 IS NULL OR files.parent_path = ?2)
                   AND (?3 IS NULL OR files.mime_type = ?3)
                   AND (?4 IS NULL OR files.modified_ms >= ?4)
                 ORDER BY file_search_fts.rank, files.rowid
                 LIMIT 7 OFFSET ?5",
            )
            .unwrap();
        let page = statement
            .query_map(
                params![
                    query.match_expression,
                    query.parent_path,
                    query.mime_type,
                    query.modified_at_least,
                    offset,
                ],
                |row| {
                    let rank: f64 = row.get(1)?;
                    Ok((row.get(0)?, rank.to_bits()))
                },
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        if page.is_empty() {
            break;
        }
        offset += i64::try_from(page.len()).unwrap();
        pages.push(page);
    }
    pages
}

#[test]
fn schema_eight_rebuild_preserves_the_fixed_ranked_query_corpus() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    for rowid in 0_i64..100 {
        let parent_path = format!("/root/scope-{}", rowid % 4).into_bytes();
        let mut path = parent_path.clone();
        path.extend_from_slice(format!("/item-{rowid:03}.txt").as_bytes());
        let display_name = format!("namegroup{}-item-{rowid:03}.txt", rowid % 20);
        let mime_type = if rowid % 2 == 0 {
            "text/plain"
        } else {
            "image/png"
        };
        let content = format!(
            "contentgroup{} shared multi{} body-{rowid}",
            rowid % 20,
            rowid % 20
        );
        connection
            .execute(
                "INSERT INTO files (
                    rowid, path, parent_path, display_name, kind, size, modified_ms,
                    accessed_ms, created_ms, mime_type, content_status, tombstoned,
                    device, inode, mtime_ns, ctime_ns, observation_state, scan_generation
                 ) VALUES (
                    ?1, ?2, ?3, ?4, 'file', ?5, ?6, ?6, ?6, ?7, ?8,
                    0, 1, ?1, ?6, ?6, 'observable', 0
                 )",
                params![
                    rowid + 1,
                    path,
                    parent_path,
                    display_name,
                    content.len() as i64,
                    rowid,
                    mime_type,
                    serde_json::to_string(&ExtractionStatus::Indexed).unwrap(),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO file_stage_state
                    (path, metadata_stage_state, content_stage_state)
                 SELECT path, 'complete', 'complete' FROM files WHERE rowid = ?1",
                [rowid + 1],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO file_search_fts(rowid, path, name, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![rowid + 1, format!("display-{rowid}"), display_name, content],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO search_data_migrations(name) VALUES (?1)",
            ["legacy_tombstone_recovery_v1"],
        )
        .unwrap();

    let mut queries = Vec::with_capacity(80);
    for group in 0..20 {
        queries.push(RankedQueryCase {
            match_expression: format!("name : (\"namegroup{group}\")"),
            parent_path: None,
            mime_type: None,
            modified_at_least: None,
        });
        queries.push(RankedQueryCase {
            match_expression: format!("\"contentgroup{group}\""),
            parent_path: None,
            mime_type: None,
            modified_at_least: None,
        });
        queries.push(RankedQueryCase {
            match_expression: format!("\"shared\" AND \"multi{group}\""),
            parent_path: None,
            mime_type: None,
            modified_at_least: None,
        });
        queries.push(RankedQueryCase {
            match_expression: "\"shared\"".to_owned(),
            parent_path: Some(format!("/root/scope-{}", group % 4).into_bytes()),
            mime_type: Some(if group % 2 == 0 {
                "text/plain".to_owned()
            } else {
                "image/png".to_owned()
            }),
            modified_at_least: Some(i64::from(group * 3)),
        });
    }
    let legacy_pages = queries
        .iter()
        .map(|query| ranked_query_pages(&connection, query))
        .collect::<Vec<_>>();
    drop(connection);

    let migrated = SearchDatabase::open(&database_path).unwrap();
    for (query_index, (query, expected_pages)) in
        queries.iter().zip(legacy_pages.iter()).enumerate()
    {
        assert_eq!(
            ranked_query_pages(&migrated.connection, query),
            *expected_pages,
            "ranked query corpus changed at index {query_index}"
        );
    }
}

#[test]
fn schema_eight_rebuild_physically_removes_raw_content_storage() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    let content = (0..200_000)
        .map(|index| format!("migrationtoken{} ", index % 1_000))
        .collect::<String>();
    insert_schema_eight_file(&connection, 41, b"/tmp/large.txt", "large.txt", &content);
    drop(connection);
    let old_bytes = fs::metadata(&database_path).unwrap().len();

    drop(SearchDatabase::open(&database_path).unwrap());

    let new_bytes = fs::metadata(&database_path).unwrap().len();
    assert!(
        new_bytes.saturating_mul(100) <= old_bytes.saturating_mul(60),
        "schema 9 file should stay within the migration budget: old={old_bytes}, new={new_bytes}"
    );
    let migrated = Connection::open(&database_path).unwrap();
    assert_eq!(schema_version(&migrated), SCHEMA_VERSION);
    assert!(!table_exists(&migrated, "file_search_fts_content"));
    assert_eq!(
        migrated
            .query_row(
                "SELECT rowid FROM file_search_fts
                 WHERE file_search_fts MATCH 'migrationtoken999'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        41
    );
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn schema_eight_rebuild_preserves_native_paths_stage_snapshots_and_markers() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    let path = b"/tmp/non-utf8-\x80.txt";
    insert_schema_eight_file(&connection, 73, path, "non-utf8.txt", "needle body");
    connection
        .execute(
            "INSERT INTO file_stage_state (
                path, metadata_stage_state, content_stage_state
             ) VALUES (?1, 'complete', 'complete')",
            [path.as_slice()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO directory_snapshots (
                path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                observation_state
             ) VALUES (?1, ?2, ?3, 1, 2, 3, 4, 'inaccessible')",
            params![
                b"/tmp/non-utf8-\x80".as_slice(),
                b"/tmp".as_slice(),
                b"/".as_slice()
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO search_data_migrations (name) VALUES ('sentinel')",
            [],
        )
        .unwrap();
    drop(connection);

    drop(SearchDatabase::open(&database_path).unwrap());

    let migrated = Connection::open(&database_path).unwrap();
    assert_eq!(schema_version(&migrated), SCHEMA_VERSION);
    assert_eq!(
        migrated
            .query_row(
                "SELECT rowid FROM files WHERE path = ?1 AND typeof(path) = 'blob'",
                [path.as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        73
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT content_stage_state FROM file_stage_state WHERE path = ?1",
                [path.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "complete"
    );
    assert_eq!(
        migrated
            .query_row(
                "SELECT observation_state FROM directory_snapshots WHERE path = ?1",
                [b"/tmp/non-utf8-\x80".as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "inaccessible"
    );
    assert!(migrated
        .query_row(
            "SELECT 1 FROM search_data_migrations WHERE name = 'sentinel'",
            [],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_some());
    assert_eq!(
        migrated
            .query_row(
                "SELECT preview FROM file_search_snippets WHERE file_rowid = 73",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "needle body"
    );
}

#[test]
fn interrupted_workspace_is_cleaned_before_rebuild() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    insert_schema_eight_file(&connection, 11, b"/tmp/old.txt", "old.txt", "oldtoken");
    drop(connection);

    let workspace = SchemaMigrationWorkspace::create(&database_path).unwrap();
    fs::write(&workspace.replacement_path, b"partial replacement").unwrap();
    let workspace_directory = workspace.directory_path.clone();
    drop(workspace);

    drop(SearchDatabase::open(&database_path).unwrap());

    assert!(!workspace_directory.exists());
    assert_eq!(
        Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT rowid FROM file_search_fts WHERE file_search_fts MATCH 'oldtoken'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        11
    );
}

#[test]
fn active_schema_eight_reader_prevents_atomic_rebuild() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let active_reader = create_schema_eight_database(&database_path);
    insert_schema_eight_file(
        &active_reader,
        1,
        b"/tmp/reader.txt",
        "reader.txt",
        "readertoken",
    );
    active_reader
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    active_reader.execute_batch("BEGIN").unwrap();
    assert_eq!(
        active_reader
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );

    let error = match SearchDatabase::open(&database_path) {
        Ok(_) => panic!("schema 8 rebuild ignored an active reader"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        SearchError::Database(ref sqlite_error)
            if matches!(
                sqlite_error.sqlite_error_code(),
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            )
    ));
    assert_eq!(schema_version(&active_reader), 8);
    assert_eq!(
        active_reader
            .query_row(
                "SELECT rowid FROM file_search_fts
                 WHERE file_search_fts MATCH 'readertoken'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert!(!workspace_path(&database_path).unwrap().exists());

    active_reader.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn active_workspace_lock_fails_closed() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    insert_schema_eight_file(
        &connection,
        12,
        b"/tmp/locked.txt",
        "locked.txt",
        "lockedtoken",
    );
    drop(connection);
    let workspace = SchemaMigrationWorkspace::create(&database_path).unwrap();

    let error = match SearchDatabase::open(&database_path) {
        Ok(_) => panic!("concurrent migration workspace was ignored"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        SearchError::DatabaseStorageMigrationFailed { .. }
    ));
    assert_eq!(
        schema_version(&Connection::open(&database_path).unwrap()),
        8
    );
    drop(workspace);
}

#[test]
fn malformed_legacy_fts_copy_failure_preserves_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    connection
        .execute_batch("DROP TABLE file_search_fts;")
        .unwrap();
    connection
        .execute_batch("CREATE VIRTUAL TABLE file_search_fts USING fts5(path UNINDEXED, name);")
        .unwrap();
    drop(connection);

    assert!(SearchDatabase::open(&database_path).is_err());

    let preserved = Connection::open(&database_path).unwrap();
    assert_eq!(schema_version(&preserved), 8);
    assert!(table_exists(&preserved, "file_search_fts"));
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn orphaned_legacy_fts_validation_failure_preserves_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, content)
             VALUES (99, '/tmp/orphan', 'orphan', 'orphantoken')",
            [],
        )
        .unwrap();
    drop(connection);

    let error = match SearchDatabase::open(&database_path) {
        Ok(_) => panic!("orphaned schema migrated successfully"),
        Err(error) => error,
    };

    assert!(matches!(error, SearchError::InvalidDatabaseSchema { .. }));
    let preserved = Connection::open(&database_path).unwrap();
    assert_eq!(schema_version(&preserved), 8);
    assert_eq!(
        preserved
            .query_row(
                "SELECT rowid FROM file_search_fts WHERE file_search_fts MATCH 'orphantoken'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        99
    );
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn atomic_replace_failure_leaves_the_original_main_file_unchanged() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let replacement_path = directory.path().join("replacement.sqlite");
    fs::write(&database_path, b"schema eight bytes").unwrap();
    fs::write(&replacement_path, b"schema nine bytes").unwrap();

    let error = atomically_replace_database(&replacement_path, &database_path, |_, _| {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();

    assert!(matches!(error, SearchError::Io { .. }));
    assert_eq!(fs::read(&database_path).unwrap(), b"schema eight bytes");
    assert_eq!(fs::read(&replacement_path).unwrap(), b"schema nine bytes");
}

#[test]
fn replacement_symlink_is_rejected_without_touching_its_target() {
    let directory = tempdir().unwrap();
    let external_path = directory.path().join("external.sqlite");
    let replacement_path = directory.path().join("replacement.sqlite");
    let external = Connection::open(&external_path).unwrap();
    external
        .execute_batch("CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('kept');")
        .unwrap();
    drop(external);
    symlink(&external_path, &replacement_path).unwrap();

    assert!(create_replacement_database(&replacement_path).is_err());
    assert_eq!(
        Connection::open(&external_path)
            .unwrap()
            .query_row("SELECT value FROM sentinel", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "kept"
    );
}

#[test]
fn full_rebuild_rename_failure_preserves_schema_eight_and_removes_workspace() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    insert_schema_eight_file(&connection, 1, b"/tmp/old.txt", "old.txt", "oldtoken");
    drop(connection);

    let error = match rebuild_schema_eight_database_with_operations(
        &database_path,
        Connection::open(&database_path).unwrap(),
        |_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        |_| Ok(()),
        open_committed_database,
    ) {
        Ok(_) => panic!("replacement rename unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(matches!(error, SearchError::Io { .. }));
    assert_schema_eight_token(&database_path, "oldtoken");
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn post_commit_sync_failure_restores_schema_eight() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    insert_schema_eight_file(&connection, 1, b"/tmp/old.txt", "old.txt", "oldtoken");
    drop(connection);
    let mut rename_count = 0_u8;

    let error = match rebuild_schema_eight_database_with_operations(
        &database_path,
        Connection::open(&database_path).unwrap(),
        |from, to| {
            rename_count += 1;
            fs::rename(from, to)
        },
        |path| {
            Err(SearchError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::Other),
            })
        },
        open_committed_database,
    ) {
        Ok(_) => panic!("post-commit sync failure unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        SearchError::DatabaseStorageMigrationFailed { .. }
    ));
    assert_eq!(rename_count, 2, "commit and rollback must both rename");
    assert_schema_eight_token(&database_path, "oldtoken");
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn post_commit_open_failure_restores_schema_eight_without_corruption_classification() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = create_schema_eight_database(&database_path);
    insert_schema_eight_file(&connection, 1, b"/tmp/old.txt", "old.txt", "oldtoken");
    drop(connection);
    let mut rename_count = 0_u8;

    let error = match rebuild_schema_eight_database_with_operations(
        &database_path,
        Connection::open(&database_path).unwrap(),
        |from, to| {
            rename_count += 1;
            fs::rename(from, to)
        },
        |_| Ok(()),
        |_| {
            Err(SearchError::Database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some("injected committed database corruption".to_owned()),
            )))
        },
    ) {
        Ok(_) => panic!("post-commit open failure unexpectedly succeeded"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        SearchError::DatabaseStorageMigrationFailed { .. }
    ));
    assert_eq!(rename_count, 2, "commit and rollback must both rename");
    assert_schema_eight_token(&database_path, "oldtoken");
    assert!(!workspace_path(&database_path).unwrap().exists());
}

#[test]
fn empty_interrupted_workspace_is_removed_before_rebuild() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    drop(create_schema_eight_database(&database_path));
    let workspace_directory = workspace_path(&database_path).unwrap();
    fs::create_dir(&workspace_directory).unwrap();
    fs::set_permissions(&workspace_directory, fs::Permissions::from_mode(0o700)).unwrap();

    let migrated = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(schema_version(&migrated.connection), SCHEMA_VERSION);
    assert!(!workspace_directory.exists());
}

#[test]
fn pending_owner_marker_is_removed_before_rebuild() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    drop(create_schema_eight_database(&database_path));
    let workspace_directory = workspace_path(&database_path).unwrap();
    fs::create_dir(&workspace_directory).unwrap();
    fs::set_permissions(&workspace_directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        workspace_directory.join(WORKSPACE_PENDING_MARKER_NAME),
        b"partial marker",
    )
    .unwrap();

    let migrated = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(schema_version(&migrated.connection), SCHEMA_VERSION);
    assert!(!workspace_directory.exists());
}

#[test]
fn untrusted_workspace_member_is_not_removed() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    drop(create_schema_eight_database(&database_path));
    let workspace_directory = workspace_path(&database_path).unwrap();
    fs::create_dir(&workspace_directory).unwrap();
    fs::set_permissions(&workspace_directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(workspace_directory.join("owner"), b"not our marker").unwrap();

    assert!(SearchDatabase::open(&database_path).is_err());

    assert_eq!(
        schema_version(&Connection::open(&database_path).unwrap()),
        8
    );
    assert!(workspace_directory.exists());
}

fn assert_schema_eight_token(database_path: &std::path::Path, token: &str) {
    let preserved = Connection::open(database_path).unwrap();
    assert_eq!(schema_version(&preserved), 8);
    assert_eq!(
        preserved
            .query_row(
                "SELECT COUNT(*) FROM file_search_fts WHERE file_search_fts MATCH ?1",
                [token],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

fn create_schema_eight_database(database_path: &std::path::Path) -> Connection {
    let connection = Connection::open(database_path).unwrap();
    connection.execute_batch(BASE_SCHEMA).unwrap();
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE file_search_fts
                USING fts5(path UNINDEXED, name, content);
             PRAGMA user_version = 8;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO search_data_migrations (name)
             VALUES ('legacy_tombstone_recovery_v1')",
            [],
        )
        .unwrap();
    connection
}

fn insert_schema_eight_file(
    connection: &Connection,
    rowid: i64,
    path: &[u8],
    display_name: &str,
    content: &str,
) {
    let content_status = serde_json::to_string(&ExtractionStatus::Indexed).unwrap();
    connection
        .execute(
            "INSERT INTO files (
                rowid, path, parent_path, display_name, kind, size,
                modified_ms, accessed_ms, created_ms, mime_type, content_status,
                tombstoned, device, inode, mtime_ns, ctime_ns, observation_state,
                scan_generation
             ) VALUES (
                ?1, ?2, ?3, ?4, 'file', ?5,
                10, 11, 12, 'text/plain', ?6,
                0, 1, ?1, 13, 14, 'observable', 0
             )",
            params![
                rowid,
                path,
                b"/tmp".as_slice(),
                display_name,
                content.len() as i64,
                content_status,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_search_fts(rowid, path, name, content)
             VALUES (?1, ?2, ?3, ?4)",
            params![rowid, String::from_utf8_lossy(path), display_name, content],
        )
        .unwrap();
}

fn schema_version(connection: &Connection) -> i64 {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn table_exists(connection: &Connection, table_name: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
             )",
            [table_name],
            |row| row.get(0),
        )
        .unwrap()
}
