#[cfg(unix)]
use std::collections::BTreeSet;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::Path;

use rusqlite::Connection;
#[cfg(unix)]
use rusqlite::{params, OpenFlags};

use crate::error::{SearchError, SearchResult};

#[cfg(unix)]
use super::schema::{
    verify_search_storage_schema, BASE_SCHEMA, CONTENTLESS_SEARCH_SCHEMA,
    PATH_CONFIGURATION_SCHEMA, QUERY_INDEXES,
};
#[cfg(unix)]
use super::storage_workspace::{
    atomically_replace_database, remove_checkpointed_sidecars, storage_migration_error,
    sync_directory, sync_regular_file, SchemaMigrationWorkspace,
};
#[cfg(all(test, unix))]
use super::storage_workspace::{workspace_path, WORKSPACE_PENDING_MARKER_NAME};
#[cfg(unix)]
use super::{
    bounded_search_content_preview, SearchDatabase, SCHEMA_VERSION,
    SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT,
};

#[cfg(not(unix))]
pub(super) fn rebuild_schema_eight_database(
    _database_path: &Path,
    connection: Connection,
) -> SearchResult<Connection> {
    Ok(connection)
}

#[cfg(unix)]
pub(super) fn rebuild_schema_eight_database(
    database_path: &Path,
    connection: Connection,
) -> SearchResult<Connection> {
    rebuild_schema_eight_database_with_operations(
        database_path,
        connection,
        |from, to| fs::rename(from, to),
        sync_directory,
        open_committed_database,
    )
}

#[cfg(unix)]
fn rebuild_schema_eight_database_with_operations(
    database_path: &Path,
    mut connection: Connection,
    mut replace: impl FnMut(&Path, &Path) -> io::Result<()>,
    sync_after_replace: impl FnOnce(&Path) -> SearchResult<()>,
    open_after_replace: impl FnOnce(&Path) -> SearchResult<Connection>,
) -> SearchResult<Connection> {
    SchemaMigrationWorkspace::remove_interrupted(database_path)?;
    let stored_schema_version =
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    if stored_schema_version != 8 {
        return Ok(connection);
    }

    let locking_mode: String =
        connection.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |row| row.get(0))?;
    if !locking_mode.eq_ignore_ascii_case("exclusive") {
        return Err(SearchError::InvalidDatabaseSchema {
            message: "schema 8 重建无法取得 SQLite exclusive locking mode".to_owned(),
        });
    }
    let checkpoint_busy = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        row.get::<_, i64>(0)
    })?;
    if checkpoint_busy != 0 {
        return Err(SearchError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("schema migration checkpoint is blocked by an active connection".to_owned()),
        )));
    }
    let source_snapshot =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    validate_schema_eight_source(database_path, &source_snapshot)?;
    let workspace = SchemaMigrationWorkspace::create(database_path)?;
    if let Err(error) = build_replacement_database(database_path, &source_snapshot, &workspace) {
        return Err(workspace.cleanup_after(error));
    }
    if let Err(error) = source_snapshot.rollback() {
        return Err(workspace.cleanup_after(SearchError::Database(error)));
    }
    if let Err(error) = remove_checkpointed_sidecars(database_path) {
        return Err(workspace.cleanup_after(error));
    }
    if let Err(error) = workspace.preserve_previous_database(database_path) {
        return Err(workspace.cleanup_after(error));
    }

    drop(connection);
    if let Err(error) =
        atomically_replace_database(&workspace.replacement_path, database_path, |from, to| {
            replace(from, to)
        })
    {
        return Err(workspace.cleanup_after(error));
    }

    let committed_connection = sync_after_replace(workspace.parent_path())
        .and_then(|()| open_after_replace(database_path));
    let committed_connection = match committed_connection {
        Ok(connection) => connection,
        Err(error) => return Err(workspace.rollback_committed(error, database_path, &mut replace)),
    };
    if let Err(error) = workspace.finish_committed() {
        tracing::warn!(
            database_path = ?database_path,
            error = %error,
            "schema 10 已提交，但迁移 workspace 清理将在下次启动重试"
        );
    }
    Ok(committed_connection)
}

#[cfg(unix)]
fn open_committed_database(database_path: &Path) -> SearchResult<Connection> {
    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    super::configure_writer_connection(&connection)?;
    let database = SearchDatabase { connection };
    database.initialize()?;
    Ok(database.connection)
}

#[cfg(unix)]
fn validate_schema_eight_source(database_path: &Path, connection: &Connection) -> SearchResult<()> {
    let conflicting_storage = connection.query_row(
        "SELECT group_concat(name, ', ')
         FROM sqlite_schema
         WHERE name IN ('file_search_fts_v9', 'file_search_snippets')",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    if let Some(conflicting_storage) = conflicting_storage {
        return Err(storage_migration_error(
            database_path,
            format!("schema 8 contains conflicting migration storage: {conflicting_storage}"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn build_replacement_database(
    database_path: &Path,
    source: &Connection,
    workspace: &SchemaMigrationWorkspace,
) -> SearchResult<()> {
    let target = create_replacement_database(&workspace.replacement_path)?;
    target.pragma_update(None, "journal_mode", "DELETE")?;
    target.pragma_update(None, "synchronous", "FULL")?;
    target.pragma_update(None, "temp_store", "FILE")?;

    let target_transaction = target.unchecked_transaction()?;
    target_transaction.execute_batch(BASE_SCHEMA)?;
    target_transaction.execute_batch(PATH_CONFIGURATION_SCHEMA)?;
    target_transaction.execute_batch(CONTENTLESS_SEARCH_SCHEMA)?;

    copy_files(source, &target_transaction)?;
    copy_stage_state(source, &target_transaction)?;
    copy_directory_snapshots(source, &target_transaction)?;
    copy_data_migration_markers(source, &target_transaction)?;
    let fulltext_evidence = copy_fulltext_storage(source, &target_transaction)?;

    target_transaction.execute_batch(QUERY_INDEXES)?;
    target_transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    validate_copied_storage(
        database_path,
        source,
        &target_transaction,
        &fulltext_evidence,
    )?;
    target_transaction.commit()?;
    validate_probe_queries(
        database_path,
        source,
        &target,
        &fulltext_evidence.probe_terms,
    )?;

    let target_database = SearchDatabase { connection: target };
    target_database.recover_legacy_tombstones_once()?;
    verify_search_storage_schema(&target_database.connection)?;
    let quick_check = target_database
        .connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    if quick_check != "ok" {
        return Err(storage_migration_error(
            database_path,
            format!("replacement quick_check failed: {quick_check}"),
        ));
    }
    target_database
        .connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(target_database);

    sync_regular_file(&workspace.replacement_path)?;
    sync_directory(&workspace.directory_path)?;
    Ok(())
}

#[cfg(unix)]
fn create_replacement_database(replacement_path: &Path) -> SearchResult<Connection> {
    Ok(Connection::open_with_flags(
        replacement_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}

#[cfg(unix)]
fn copy_files(source: &Connection, target: &Connection) -> SearchResult<()> {
    let mut source_statement = source.prepare(
        "SELECT rowid, path, parent_path, display_name, kind, size,
                modified_ms, accessed_ms, created_ms, mime_type, content_status,
                tombstoned, device, inode, mtime_ns, ctime_ns, observation_state,
                scan_generation
         FROM files ORDER BY rowid",
    )?;
    let mut rows = source_statement.query([])?;
    let mut insert_statement = target.prepare_cached(
        "INSERT INTO files (
            rowid, path, parent_path, display_name, kind, size,
            modified_ms, accessed_ms, created_ms, mime_type, content_status,
            tombstoned, device, inode, mtime_ns, ctime_ns, observation_state,
            scan_generation
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
            ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
         )",
    )?;
    while let Some(row) = rows.next()? {
        insert_statement.execute(params![
            row.get::<_, i64>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, Option<i64>>(12)?,
            row.get::<_, Option<i64>>(13)?,
            row.get::<_, Option<i64>>(14)?,
            row.get::<_, Option<i64>>(15)?,
            row.get::<_, String>(16)?,
            row.get::<_, i64>(17)?,
        ])?;
    }
    Ok(())
}

#[cfg(unix)]
fn copy_stage_state(source: &Connection, target: &Connection) -> SearchResult<()> {
    let mut source_statement = source.prepare(
        "SELECT path, metadata_stage_state, content_stage_state
         FROM file_stage_state ORDER BY path",
    )?;
    let mut rows = source_statement.query([])?;
    let mut insert_statement = target.prepare_cached(
        "INSERT INTO file_stage_state (
            path, metadata_stage_state, content_stage_state
         ) VALUES (?1, ?2, ?3)",
    )?;
    while let Some(row) = rows.next()? {
        insert_statement.execute(params![
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ])?;
    }
    Ok(())
}

#[cfg(unix)]
fn copy_directory_snapshots(source: &Connection, target: &Connection) -> SearchResult<()> {
    let mut source_statement = source.prepare(
        "SELECT path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                observation_state
         FROM directory_snapshots ORDER BY path",
    )?;
    let mut rows = source_statement.query([])?;
    let mut insert_statement = target.prepare_cached(
        "INSERT INTO directory_snapshots (
            path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
            observation_state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    while let Some(row) = rows.next()? {
        insert_statement.execute(params![
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
        ])?;
    }
    Ok(())
}

#[cfg(unix)]
fn copy_data_migration_markers(source: &Connection, target: &Connection) -> SearchResult<()> {
    let mut source_statement =
        source.prepare("SELECT name FROM search_data_migrations ORDER BY name")?;
    let mut rows = source_statement.query([])?;
    let mut insert_statement =
        target.prepare_cached("INSERT INTO search_data_migrations (name) VALUES (?1)")?;
    while let Some(row) = rows.next()? {
        insert_statement.execute([row.get::<_, String>(0)?])?;
    }
    Ok(())
}

#[cfg(unix)]
struct FulltextCopyEvidence {
    row_count: i64,
    probe_terms: Vec<String>,
}

#[cfg(unix)]
fn copy_fulltext_storage(
    source: &Connection,
    target: &Connection,
) -> SearchResult<FulltextCopyEvidence> {
    let orphan_count = source.query_row(
        "SELECT COUNT(*)
         FROM file_search_fts old_fts
         LEFT JOIN files ON files.rowid = old_fts.rowid
         WHERE files.rowid IS NULL",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if orphan_count != 0 {
        return Err(SearchError::InvalidDatabaseSchema {
            message: format!("schema 8 FTS has {orphan_count} rowids without file metadata"),
        });
    }

    let mut source_statement =
        source.prepare("SELECT rowid, name, content FROM file_search_fts ORDER BY rowid")?;
    let mut rows = source_statement.query([])?;
    let mut fts_insert = target
        .prepare_cached("INSERT INTO file_search_fts(rowid, name, content) VALUES (?1, ?2, ?3)")?;
    let mut preview_insert = target
        .prepare_cached("INSERT INTO file_search_snippets(file_rowid, preview) VALUES (?1, ?2)")?;
    let mut row_count = 0_i64;
    let mut probe_terms = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let rowid = row.get::<_, i64>(0)?;
        let name = row.get::<_, Option<String>>(1)?;
        let content = row.get::<_, Option<String>>(2)?;
        fts_insert.execute(params![rowid, name, content])?;
        if let Some(preview) = content.as_deref().and_then(bounded_search_content_preview) {
            preview_insert.execute(params![rowid, preview])?;
        }
        if probe_terms.len() < 8 {
            collect_probe_terms(name.as_deref(), &mut probe_terms);
            collect_probe_terms(content.as_deref(), &mut probe_terms);
        }
        row_count += 1;
    }
    Ok(FulltextCopyEvidence {
        row_count,
        probe_terms: probe_terms.into_iter().collect(),
    })
}

#[cfg(unix)]
fn collect_probe_terms(text: Option<&str>, probe_terms: &mut BTreeSet<String>) {
    let Some(text) = text else {
        return;
    };
    for token in text.split_whitespace() {
        if probe_terms.len() >= 8 {
            return;
        }
        if (3..=32).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            probe_terms.insert(token.to_ascii_lowercase());
        }
    }
}

#[cfg(unix)]
fn validate_copied_storage(
    database_path: &Path,
    source: &Connection,
    target: &Connection,
    fulltext_evidence: &FulltextCopyEvidence,
) -> SearchResult<()> {
    for table_name in [
        "files",
        "file_stage_state",
        "directory_snapshots",
        "search_data_migrations",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table_name}");
        let source_count = source.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
        let target_count = target.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
        if source_count != target_count {
            return Err(storage_migration_error(
                database_path,
                format!("{table_name} row count changed from {source_count} to {target_count}"),
            ));
        }
    }

    let target_fts_count = target.query_row("SELECT COUNT(*) FROM file_search_fts", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let target_fts_orphans = target.query_row(
        "SELECT COUNT(*)
         FROM file_search_fts
         LEFT JOIN files ON files.rowid = file_search_fts.rowid
         WHERE files.rowid IS NULL",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let preview_orphans = target.query_row(
        "SELECT COUNT(*)
         FROM file_search_snippets
         LEFT JOIN files ON files.rowid = file_search_snippets.file_rowid
         LEFT JOIN file_search_fts ON file_search_fts.rowid = file_search_snippets.file_rowid
         WHERE files.rowid IS NULL OR file_search_fts.rowid IS NULL",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let longest_preview = target.query_row(
        "SELECT COALESCE(MAX(length(preview)), 0) FROM file_search_snippets",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if target_fts_count != fulltext_evidence.row_count
        || target_fts_orphans != 0
        || preview_orphans != 0
        || longest_preview > SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT as i64
    {
        return Err(storage_migration_error(
            database_path,
            format!(
                "replacement validation failed: FTS rows {target_fts_count}/{}, FTS orphans {target_fts_orphans}, preview orphans {preview_orphans}, longest preview {longest_preview}",
                fulltext_evidence.row_count
            ),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_probe_queries(
    database_path: &Path,
    source: &Connection,
    target: &Connection,
    probe_terms: &[String],
) -> SearchResult<()> {
    for term in probe_terms {
        let expression = format!("\"{}\"", term.replace('"', "\"\""));
        let source_rows = fulltext_rank_rows(source, &expression)?;
        let target_rows = fulltext_rank_rows(target, &expression)?;
        if source_rows != target_rows {
            return Err(storage_migration_error(
                database_path,
                format!("FTS hit/rank probe changed for token {term:?}"),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn fulltext_rank_rows(connection: &Connection, expression: &str) -> SearchResult<Vec<(i64, u64)>> {
    let mut statement = connection.prepare(
        "SELECT rowid, rank
         FROM file_search_fts
         WHERE file_search_fts MATCH ?1
         ORDER BY rank, rowid
         LIMIT 201",
    )?;
    let rows = statement
        .query_map([expression], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?.to_bits()))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(all(test, unix))]
#[path = "storage_rebuild_tests.rs"]
mod tests;
