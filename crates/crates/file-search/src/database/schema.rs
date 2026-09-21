use rusqlite::{Connection, OptionalExtension};

use crate::error::{SearchError, SearchResult};

use super::{SearchDatabase, SCHEMA_VERSION};

pub(super) const BASE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS files (
        path BLOB PRIMARY KEY,
        parent_path BLOB NOT NULL,
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
    CREATE TABLE IF NOT EXISTS file_stage_state (
        path BLOB PRIMARY KEY,
        metadata_stage_state TEXT NOT NULL,
        content_stage_state TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS directory_snapshots (
        path BLOB PRIMARY KEY,
        parent_path BLOB NOT NULL,
        root_path BLOB NOT NULL,
        device INTEGER NOT NULL,
        inode INTEGER NOT NULL,
        mtime_ns INTEGER NOT NULL,
        ctime_ns INTEGER NOT NULL,
        observation_state TEXT NOT NULL DEFAULT 'observable'
            CHECK (observation_state IN ('observable', 'inaccessible'))
    );
    CREATE TABLE IF NOT EXISTS search_data_migrations (
        name TEXT PRIMARY KEY
    ) WITHOUT ROWID;";

pub(super) const PATH_CONFIGURATION_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS search_path_configuration (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        revision INTEGER NOT NULL CHECK (revision >= 0),
        preferences BLOB NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS search_root_mounts (
        root_path BLOB PRIMARY KEY,
        mount_point BLOB NOT NULL,
        device INTEGER NOT NULL CHECK(device >= 0)
    ) WITHOUT ROWID;";

pub(super) const CONTENTLESS_SEARCH_SCHEMA: &str = "CREATE VIRTUAL TABLE file_search_fts
        USING fts5(
            name,
            content,
            content = '',
            contentless_delete = 1,
            detail = full
        );
     CREATE TABLE file_search_snippets (
        file_rowid INTEGER PRIMARY KEY,
        preview TEXT NOT NULL CHECK (length(preview) <= 1024)
     );";

pub(super) const QUERY_INDEXES: &str = "CREATE INDEX IF NOT EXISTS files_visible_modified_name
        ON files(tombstoned, observation_state, modified_ms DESC, display_name);
     CREATE INDEX IF NOT EXISTS files_parent_visible_modified_name
        ON files(parent_path, tombstoned, observation_state, modified_ms DESC, display_name);
     CREATE INDEX IF NOT EXISTS files_hidden_query_rows
        ON files(tombstoned, observation_state)
        WHERE tombstoned <> 0 OR observation_state <> 'observable';
     CREATE INDEX IF NOT EXISTS directory_snapshots_root_path
        ON directory_snapshots(root_path, path);
     CREATE INDEX IF NOT EXISTS directory_snapshots_parent_path
        ON directory_snapshots(parent_path, path);";

impl SearchDatabase {
    pub(super) fn initialize(&self) -> SearchResult<()> {
        let stored_schema_version =
            self.connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        let has_existing_search_schema = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table'
                  AND name IN ('files', 'file_stage_state', 'directory_snapshots', 'file_search_fts')
             )",
            [],
            |row| row.get::<_, bool>(0),
        )?;

        if stored_schema_version == 0 && !has_existing_search_schema {
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute_batch(BASE_SCHEMA)?;
            transaction.execute_batch(PATH_CONFIGURATION_SCHEMA)?;
            transaction.execute_batch(CONTENTLESS_SEARCH_SCHEMA)?;
            transaction.execute_batch(QUERY_INDEXES)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
        } else {
            self.connection.execute_batch(BASE_SCHEMA)?;
            if stored_schema_version < SCHEMA_VERSION {
                self.connection.execute_batch(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS file_search_fts
                        USING fts5(path UNINDEXED, name, content);",
                )?;
            }
            self.migrate()?;
            self.connection.execute_batch(QUERY_INDEXES)?;
        }
        self.verify_search_storage_schema()?;
        self.recover_legacy_tombstones_once()?;
        Ok(())
    }

    pub(super) fn verify_search_storage_schema(&self) -> SearchResult<()> {
        verify_search_storage_schema(&self.connection)
    }
}

pub(super) fn verify_search_storage_schema(connection: &Connection) -> SearchResult<()> {
    let fts_schema: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'file_search_fts'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let normalized_fts_schema = fts_schema
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let snippet_schema: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'file_search_snippets'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let normalized_snippet_schema = snippet_schema
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let fts_columns = connection.query_row(
        "SELECT COALESCE(group_concat(name, ','), '')
         FROM (SELECT name FROM pragma_table_info('file_search_fts') ORDER BY cid)",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let snippet_columns = connection.query_row(
        "SELECT COALESCE(group_concat(name, ','), '')
         FROM (SELECT name FROM pragma_table_info('file_search_snippets') ORDER BY cid)",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let required_table_count = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'table'
           AND name IN (
               'files', 'file_stage_state', 'directory_snapshots',
               'search_data_migrations', 'file_search_fts', 'file_search_snippets'
           )",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let path_configuration_columns = connection.query_row(
        "SELECT COALESCE(group_concat(name, ','), '')
         FROM (SELECT name FROM pragma_table_info('search_path_configuration') ORDER BY cid)",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let search_root_mount_columns = connection.query_row(
        "SELECT COALESCE(group_concat(name, ','), '')
         FROM (SELECT name FROM pragma_table_info('search_root_mounts') ORDER BY cid)",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let required_index_count = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'index'
           AND name IN (
               'files_visible_modified_name',
               'files_parent_visible_modified_name',
               'files_hidden_query_rows',
               'directory_snapshots_root_path',
               'directory_snapshots_parent_path'
           )",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let raw_fts_content_table_exists = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_schema
            WHERE type = 'table' AND name = 'file_search_fts_content'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !normalized_fts_schema.contains("content=''")
        || !normalized_fts_schema.contains("contentless_delete=1")
        || !normalized_fts_schema.contains("detail=full")
        || fts_columns != "name,content"
        || snippet_columns != "file_rowid,preview"
        || !normalized_snippet_schema.contains("check(length(preview)<=1024)")
        || required_table_count != 6
        || path_configuration_columns != "singleton,revision,preferences"
        || search_root_mount_columns != "root_path,mount_point,device"
        || required_index_count != 5
        || raw_fts_content_table_exists
    {
        return Err(SearchError::InvalidDatabaseSchema {
            message: "schema 10 requires an exact durable path snapshot, root mount identities, contentless full-detail FTS, and bounded snippets"
                .to_owned(),
        });
    }
    Ok(())
}
