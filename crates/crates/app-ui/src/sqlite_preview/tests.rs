use std::path::Path;

use super::{
    load_sqlite_preview_blocking, load_sqlite_table_data_blocking, run_sqlite_sql_blocking,
    SQLITE_ROW_LIMIT,
};
use crate::model::SqliteCellValue;
use rusqlite::Connection;

fn create_sample_database(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, avatar BLOB, score REAL);
             INSERT INTO users (name, avatar, score) VALUES ('alice', x'010203', 1.5);
             CREATE TABLE logs (id INTEGER PRIMARY KEY);
             CREATE TABLE \"weird\"\"name\" (id INTEGER);",
        )
        .unwrap();
}

#[test]
fn loads_table_summaries_with_row_counts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    create_sample_database(&path);

    let preview = load_sqlite_preview_blocking(&path).unwrap();
    let names: Vec<&str> = preview.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["logs", "users", "weird\"name"]);
    let users = preview.tables.iter().find(|t| t.name == "users").unwrap();
    assert_eq!(users.row_count, 1);
}

#[test]
fn reports_non_database_files_as_errors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fake.db");
    std::fs::write(&path, b"definitely not a sqlite database").unwrap();

    assert!(load_sqlite_preview_blocking(&path).is_err());
}

#[test]
fn truncates_table_data_at_row_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("many.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY);")
        .unwrap();
    for id in 0..(SQLITE_ROW_LIMIT as i64 + 50) {
        connection
            .execute("INSERT INTO items (id) VALUES (?1)", [id])
            .unwrap();
    }

    let data = load_sqlite_table_data_blocking(&path, "items").unwrap();
    assert_eq!(data.rows.len(), SQLITE_ROW_LIMIT);
    assert!(data.truncated);
    assert_eq!(data.columns, ["id"]);
}

#[test]
fn renders_null_text_blob_and_numeric_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("values.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE mixed (a INTEGER, b TEXT, c BLOB, d REAL);
             INSERT INTO mixed VALUES (7, 'text', x'0102', 3.5);
             INSERT INTO mixed VALUES (NULL, NULL, NULL, NULL);",
        )
        .unwrap();

    let data = load_sqlite_table_data_blocking(&path, "mixed").unwrap();
    assert_eq!(data.columns, ["a", "b", "c", "d"]);
    assert_eq!(data.rows.len(), 2);
    assert!(!data.truncated);
    assert_eq!(data.rows[0][0], SqliteCellValue::Integer(7));
    assert_eq!(data.rows[0][1], SqliteCellValue::Text("text".to_owned()));
    assert_eq!(data.rows[0][2], SqliteCellValue::Blob(2));
    assert_eq!(data.rows[0][3], SqliteCellValue::Real(3.5));
    assert_eq!(data.rows[1][0], SqliteCellValue::Null);
    assert_eq!(data.rows[1][1], SqliteCellValue::Null);
}

#[test]
fn sql_runner_rejects_writes_on_read_only_connection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readonly.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE items (id INTEGER); INSERT INTO items VALUES (1);")
        .unwrap();
    drop(connection);

    let outcome = run_sqlite_sql_blocking(&path, "INSERT INTO items VALUES (2)");
    assert!(outcome.is_err());

    let row_count: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row_count, 1);
}

#[test]
fn sql_runner_truncates_and_reports_errors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("query.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY);")
        .unwrap();
    for id in 0..(SQLITE_ROW_LIMIT as i64 + 10) {
        connection
            .execute("INSERT INTO items (id) VALUES (?1)", [id])
            .unwrap();
    }
    drop(connection);

    let outcome = run_sqlite_sql_blocking(&path, "SELECT id FROM items").unwrap();
    assert_eq!(outcome.rows.len(), SQLITE_ROW_LIMIT);
    assert!(outcome.truncated);

    let outcome = run_sqlite_sql_blocking(&path, "SELECT * FROM missing_table");
    assert!(outcome.is_err());
}

#[test]
fn sql_runner_reports_statements_without_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pragma.db");
    Connection::open(&path).unwrap();

    let outcome = run_sqlite_sql_blocking(&path, "PRAGMA user_version").unwrap();
    assert_eq!(outcome.columns, ["user_version"]);
    assert_eq!(outcome.rows.len(), 1);
    assert_eq!(outcome.rows[0][0], SqliteCellValue::Integer(0));
}
