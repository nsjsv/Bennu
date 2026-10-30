use std::fs;

use rusqlite::{params, Connection};
use tempfile::tempdir;

use super::{SearchDatabase, SearchError, SCHEMA_VERSION};

fn ranked_fts_rows(connection: &Connection, table: &str, terms: &str) -> Vec<(i64, u64)> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT rowid, rank FROM {table} WHERE {table} MATCH ?1 ORDER BY rank, rowid"
        ))
        .unwrap();
    statement
        .query_map([terms], |row| {
            let score: f64 = row.get(1)?;
            Ok((row.get(0)?, score.to_bits()))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn bundled_sqlite_supports_contentless_delete_and_full_detail_preserves_rank() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE stored_content
                USING fts5(name, content);
             CREATE VIRTUAL TABLE contentless_full
                USING fts5(name, content, content = '', contentless_delete = 1);
             CREATE VIRTUAL TABLE contentless_column
                USING fts5(
                    name,
                    content,
                    content = '',
                    contentless_delete = 1,
                    detail = column
                );",
        )
        .unwrap();

    for (rowid, name, content) in [
        (1_i64, "needle-name.txt", "brief body"),
        (2_i64, "dense.txt", "needle needle needle"),
        (
            3_i64,
            "long.txt",
            "needle with a substantially longer document body for ranking",
        ),
        (4_i64, "tie.txt", "same needle body"),
    ] {
        for table in ["stored_content", "contentless_full", "contentless_column"] {
            connection
                .execute(
                    &format!("INSERT INTO {table}(rowid, name, content) VALUES (?1, ?2, ?3)"),
                    params![rowid, name, content],
                )
                .unwrap();
        }
    }

    let stored_rank = ranked_fts_rows(&connection, "stored_content", "\"needle\"");
    assert_eq!(
        ranked_fts_rows(&connection, "contentless_full", "\"needle\""),
        stored_rank
    );
    assert_ne!(
        ranked_fts_rows(&connection, "contentless_column", "\"needle\""),
        stored_rank
    );

    connection
        .execute(
            "UPDATE contentless_full
             SET name = 'updated.txt', content = 'replacement token'
             WHERE rowid = 2",
            [],
        )
        .unwrap();
    assert!(
        ranked_fts_rows(&connection, "contentless_full", "\"needle\"")
            .iter()
            .all(|(rowid, _)| *rowid != 2)
    );
    assert_eq!(
        ranked_fts_rows(&connection, "contentless_full", "\"replacement\"")
            .iter()
            .map(|(rowid, _)| *rowid)
            .collect::<Vec<_>>(),
        vec![2]
    );

    connection
        .execute("DELETE FROM contentless_full WHERE rowid = 3", [])
        .unwrap();
    assert!(
        ranked_fts_rows(&connection, "contentless_full", "\"needle\"")
            .iter()
            .all(|(rowid, _)| *rowid != 3)
    );

    connection
        .execute(
            "INSERT OR REPLACE INTO contentless_full(rowid, name, content)
             VALUES (4, 'replaced.txt', 'new token')",
            [],
        )
        .unwrap();
    assert!(
        ranked_fts_rows(&connection, "contentless_full", "\"needle\"")
            .iter()
            .all(|(rowid, _)| *rowid != 4)
    );
    assert_eq!(
        ranked_fts_rows(&connection, "contentless_full", "\"new\"")
            .iter()
            .map(|(rowid, _)| *rowid)
            .collect::<Vec<_>>(),
        vec![4]
    );
}

#[test]
fn fresh_schema_keeps_only_contentless_tokens_and_bounded_previews() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();

    assert_eq!(
        database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let fts_schema: String = database
        .connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'file_search_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let normalized_schema = fts_schema
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    assert!(normalized_schema.contains("content=''"));
    assert!(normalized_schema.contains("contentless_delete=1"));
    assert!(normalized_schema.contains("detail=full"));
    assert_eq!(
        database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'file_search_fts_content'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    database
        .connection
        .execute(
            "INSERT INTO file_search_fts(rowid, name, content) VALUES(1, 'name.txt', 'secret body')",
            [],
        )
        .unwrap();
    let stored_columns: (Option<String>, Option<String>) = database
        .connection
        .query_row(
            "SELECT name, content FROM file_search_fts WHERE rowid = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored_columns, (None, None));

    let oversized_preview = "界".repeat(1_025);
    assert!(database
        .connection
        .execute(
            "INSERT INTO file_search_snippets(file_rowid, preview) VALUES(1, ?1)",
            [oversized_preview],
        )
        .is_err());
}

#[test]
fn read_only_open_requires_a_complete_current_schema() {
    let directory = tempdir().unwrap();
    let legacy_path = directory.path().join("legacy.sqlite");
    let legacy = Connection::open(&legacy_path).unwrap();
    legacy.pragma_update(None, "user_version", 8).unwrap();
    drop(legacy);
    assert!(matches!(
        SearchDatabase::open_read_only(&legacy_path),
        Err(SearchError::InvalidDatabaseSchema { .. })
    ));

    let malformed_path = directory.path().join("malformed.sqlite");
    let malformed = Connection::open(&malformed_path).unwrap();
    malformed
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    drop(malformed);
    assert!(matches!(
        SearchDatabase::open_read_only(&malformed_path),
        Err(SearchError::InvalidDatabaseSchema { .. })
    ));
}

#[test]
fn future_schema_is_rejected_before_writer_setup_or_migration() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sentinel (value TEXT NOT NULL);
             INSERT INTO sentinel VALUES ('preserved');",
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    drop(connection);

    let original_bytes = fs::read(&database_path).unwrap();
    let error = match SearchDatabase::open(&database_path) {
        Ok(_) => panic!("future schema opened successfully"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        SearchError::UnsupportedDatabaseSchema {
            found,
            supported,
        } if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
    ));
    assert_eq!(fs::read(&database_path).unwrap(), original_bytes);
    assert!(!database_path.with_file_name("search.sqlite-wal").exists());
    assert!(!database_path.with_file_name("search.sqlite-shm").exists());

    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION + 1
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM sentinel", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "preserved"
    );
}
