use crate::error::{SearchError, SearchResult};
use crate::extractor::ExtractionStatus;

use super::{
    IndexedEntryStageState, SearchDatabase, SCHEMA_VERSION, SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT,
};

const LEGACY_TOMBSTONE_RECOVERY_MIGRATION: &str = "legacy_tombstone_recovery_v1";

impl SearchDatabase {
    /// 按 `PRAGMA user_version` 只向前迁移，确保旧索引可以逐步收敛到当前 schema。
    pub(super) fn migrate(&self) -> SearchResult<()> {
        let current: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current < 1 {
            // 旧库缺少增量扫描签名；NULL 会强制首次升级后重新索引一次。
            if !self.column_exists("files", "inode")? {
                self.connection
                    .execute_batch("ALTER TABLE files ADD COLUMN inode INTEGER;")?;
            }
            if !self.column_exists("files", "mtime_ns")? {
                self.connection
                    .execute_batch("ALTER TABLE files ADD COLUMN mtime_ns INTEGER;")?;
            }
        }
        if current < 2 {
            self.backfill_stage_state_rows()?;
        }
        if current < 3 && !self.column_exists("files", "observation_state")? {
            self.connection.execute_batch(
                "ALTER TABLE files ADD COLUMN observation_state TEXT NOT NULL DEFAULT 'observable'
                    CHECK (observation_state IN ('observable', 'inaccessible'));",
            )?;
        }
        if current < 4 && !self.column_exists("files", "scan_generation")? {
            self.connection.execute_batch(
                "ALTER TABLE files ADD COLUMN scan_generation INTEGER NOT NULL DEFAULT 0;
                 CREATE TABLE IF NOT EXISTS index_scans (
                    scan_generation INTEGER PRIMARY KEY AUTOINCREMENT,
                    state TEXT NOT NULL CHECK (state IN ('running', 'complete', 'aborted')),
                    started_ms INTEGER NOT NULL,
                    finished_ms INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS scan_scopes (
                    scan_generation INTEGER NOT NULL,
                    scope_path TEXT NOT NULL,
                    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('exact', 'tree')),
                    state TEXT NOT NULL CHECK (
                        state IN ('complete', 'inaccessible', 'missing', 'policy_excluded')
                    ),
                    PRIMARY KEY (scan_generation, scope_path, state)
                 ) WITHOUT ROWID;
                 CREATE INDEX IF NOT EXISTS files_scan_generation
                    ON files(scan_generation, tombstoned);",
            )?;
        }
        if current < 5 {
            self.migrate_fts_rowids()?;
        }
        if current < 6 && !self.column_exists("files", "ctime_ns")? {
            self.connection
                .execute_batch("ALTER TABLE files ADD COLUMN ctime_ns INTEGER;")?;
        }
        if current < 7 {
            if !self.column_exists("files", "device")? {
                self.connection
                    .execute_batch("ALTER TABLE files ADD COLUMN device INTEGER;")?;
            }
            self.connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS directory_snapshots (
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
                 CREATE INDEX IF NOT EXISTS directory_snapshots_root_path
                    ON directory_snapshots(root_path, path);
                 CREATE INDEX IF NOT EXISTS directory_snapshots_parent_path
                    ON directory_snapshots(parent_path, path);
                 DROP INDEX IF EXISTS files_scan_generation;
                 DROP TABLE IF EXISTS scan_scopes;
                 DROP TABLE IF EXISTS index_scans;",
            )?;
        }
        if current < 8 {
            self.migrate_paths_to_blob()?;
        }
        if current < 9 {
            self.migrate_fulltext_storage()?;
        }
        if current < 10 {
            self.migrate_path_configuration()?;
        }
        if current < 11 {
            self.reset_skipped_office_content_stage()?;
        }
        Ok(())
    }

    /// docx/xlsx/pptx/odt 从“跳过”升级为可进程内提取后，这些格式存量的
    /// skipped 内容阶段必须重置为 pending：签名比较只放行非 pending 行，
    /// 不重置的话存量 Office 文件在文件变更前永远不会被重新提取。
    fn reset_skipped_office_content_stage(&self) -> SearchResult<()> {
        const OFFICE_MIME_TYPES: [&str; 4] = [
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.oasis.opendocument.text",
        ];
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE file_stage_state
             SET content_stage_state = 'pending'
             WHERE content_stage_state = 'skipped'
               AND path IN (
                   SELECT path FROM files
                   WHERE mime_type IN (?1, ?2, ?3, ?4)
               )",
            rusqlite::params![
                OFFICE_MIME_TYPES[0],
                OFFICE_MIME_TYPES[1],
                OFFICE_MIME_TYPES[2],
                OFFICE_MIME_TYPES[3]
            ],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    fn migrate_path_configuration(&self) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(super::schema::PATH_CONFIGURATION_SCHEMA)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    fn migrate_fulltext_storage(&self) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let old_row_count =
            transaction.query_row("SELECT COUNT(*) FROM file_search_fts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let missing_file_count = transaction.query_row(
            "SELECT COUNT(*)
             FROM file_search_fts AS old
             LEFT JOIN files AS f ON f.rowid = old.rowid
             WHERE f.rowid IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if missing_file_count != 0 {
            return Err(SearchError::InvalidDatabaseSchema {
                message: format!(
                    "schema 8 FTS rowid coverage is incomplete: {missing_file_count} orphan FTS rows"
                ),
            });
        }

        transaction.execute_batch(
            "CREATE VIRTUAL TABLE file_search_fts_v9
                USING fts5(
                    name,
                    content,
                    content = '',
                    contentless_delete = 1,
                    detail = full
                );
             CREATE TABLE file_search_snippets_v9 (
                file_rowid INTEGER PRIMARY KEY,
                preview TEXT NOT NULL CHECK (length(preview) <= 1024)
             );
             INSERT INTO file_search_fts_v9(rowid, name, content)
                SELECT rowid, name, content FROM file_search_fts;",
        )?;
        transaction.execute(
            "INSERT INTO file_search_snippets_v9(file_rowid, preview)
             SELECT rowid, substr(content, 1, ?1)
             FROM file_search_fts
             WHERE content IS NOT NULL AND content <> ''",
            [SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT as i64],
        )?;

        let new_row_count =
            transaction.query_row("SELECT COUNT(*) FROM file_search_fts_v9", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let invalid_preview_count = transaction.query_row(
            "SELECT COUNT(*) FROM file_search_snippets_v9
             WHERE length(preview) > ?1 OR preview = ''",
            [SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT as i64],
            |row| row.get::<_, i64>(0),
        )?;
        let new_orphan_count = transaction.query_row(
            "SELECT COUNT(*)
             FROM file_search_fts_v9 AS migrated
             LEFT JOIN files AS f ON f.rowid = migrated.rowid
             WHERE f.rowid IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if new_row_count != old_row_count || invalid_preview_count != 0 || new_orphan_count != 0 {
            return Err(SearchError::InvalidDatabaseSchema {
                message: format!(
                    "schema 9 FTS validation failed: expected {old_row_count} rows, copied {new_row_count}, invalid previews {invalid_preview_count}, orphan rows {new_orphan_count}"
                ),
            });
        }

        transaction.execute_batch(
            "DROP TABLE file_search_fts;
             ALTER TABLE file_search_fts_v9 RENAME TO file_search_fts;
             ALTER TABLE file_search_snippets_v9 RENAME TO file_search_snippets;",
        )?;
        transaction.pragma_update(None, "user_version", 9)?;
        transaction.commit()?;
        Ok(())
    }

    fn migrate_paths_to_blob(&self) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        // 非 Unix 旧 TEXT 无法证明原生编码，派生索引必须清空并由启动扫描重建。
        #[cfg(not(unix))]
        transaction.execute_batch(
            "DELETE FROM file_search_fts;
             DELETE FROM file_stage_state;
             DELETE FROM directory_snapshots;
             DELETE FROM files;",
        )?;
        transaction.execute_batch(
            "CREATE TABLE files_path_migration (
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
             INSERT INTO files_path_migration (
                rowid, path, parent_path, display_name, kind, size, modified_ms, accessed_ms,
                created_ms, mime_type, content_status, tombstoned, device, inode, mtime_ns,
                ctime_ns, observation_state, scan_generation
             )
                SELECT rowid, CAST(path AS BLOB), CAST(parent_path AS BLOB), display_name, kind, size,
                       modified_ms, accessed_ms, created_ms, mime_type, content_status, tombstoned,
                       device, inode, mtime_ns, ctime_ns, observation_state, scan_generation
                FROM files;
             DROP TABLE files;
             ALTER TABLE files_path_migration RENAME TO files;
             CREATE TABLE file_stage_state_path_migration (
                path BLOB PRIMARY KEY,
                metadata_stage_state TEXT NOT NULL,
                content_stage_state TEXT NOT NULL
             );
             INSERT INTO file_stage_state_path_migration
                SELECT CAST(path AS BLOB), metadata_stage_state, content_stage_state
                FROM file_stage_state;
             DROP TABLE file_stage_state;
             ALTER TABLE file_stage_state_path_migration RENAME TO file_stage_state;
             CREATE TABLE directory_snapshots_path_migration (
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
             INSERT INTO directory_snapshots_path_migration
                SELECT CAST(path AS BLOB), CAST(parent_path AS BLOB), CAST(root_path AS BLOB),
                       device, inode, mtime_ns, ctime_ns, observation_state
                FROM directory_snapshots;
             DROP TABLE directory_snapshots;
             ALTER TABLE directory_snapshots_path_migration RENAME TO directory_snapshots;",
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn migrate_fts_rowids(&self) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(
            "DROP TABLE IF EXISTS file_search_fts_rowid_migration;
             CREATE VIRTUAL TABLE file_search_fts_rowid_migration
                USING fts5(path UNINDEXED, name, content);
             INSERT INTO file_search_fts_rowid_migration(rowid, path, name, content)
                SELECT f.rowid, old.path, old.name, old.content
                FROM file_search_fts AS old
                JOIN files AS f ON f.path = old.path;
             DROP TABLE file_search_fts;
             ALTER TABLE file_search_fts_rowid_migration RENAME TO file_search_fts;",
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn recover_legacy_tombstones_once(&self) -> SearchResult<()> {
        let migration_completed = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM search_data_migrations WHERE name = ?1)",
            [LEGACY_TOMBSTONE_RECOVERY_MIGRATION],
            |row| row.get::<_, bool>(0),
        )?;
        if migration_completed {
            return Ok(());
        }

        let transaction = self.connection.unchecked_transaction()?;
        let tombstones_exist = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE tombstoned <> 0)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if tombstones_exist {
            transaction.execute(
                "DELETE FROM file_search_snippets
                 WHERE file_rowid IN (SELECT rowid FROM files WHERE tombstoned <> 0)",
                [],
            )?;
            transaction.execute(
                "DELETE FROM file_search_fts
                 WHERE rowid IN (SELECT rowid FROM files WHERE tombstoned <> 0)",
                [],
            )?;
            transaction.execute(
                "DELETE FROM file_stage_state
                 WHERE path IN (SELECT path FROM files WHERE tombstoned <> 0)",
                [],
            )?;
            transaction.execute("DELETE FROM files WHERE tombstoned <> 0", [])?;
        }
        transaction.execute(
            "INSERT INTO search_data_migrations(name) VALUES(?1)",
            [LEGACY_TOMBSTONE_RECOVERY_MIGRATION],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn backfill_stage_state_rows(&self) -> SearchResult<()> {
        let mut statement = self
            .connection
            .prepare("SELECT path, content_status FROM files")?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(0)?;
            let content_status: String = row.get(1)?;
            Ok((path, content_status))
        })?;

        for row in rows {
            let (path, content_status) = row?;
            let extraction_status: ExtractionStatus = serde_json::from_str(&content_status)?;
            let stage_state =
                IndexedEntryStageState::from_legacy_content_status(&extraction_status);
            self.upsert_stage_state_by_storage_path(&path, &stage_state)?;
        }

        Ok(())
    }

    pub(super) fn column_exists(&self, table: &str, column: &str) -> SearchResult<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let exists = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == column);
        Ok(exists)
    }
}
