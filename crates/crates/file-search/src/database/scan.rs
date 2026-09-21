use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};

use crate::error::{SearchError, SearchResult};
use crate::model::SearchFileKind;

use super::{
    path_from_storage_bytes, path_to_storage, recursive_storage_range, DirectorySignature,
    DirectorySnapshot, EntryObservationState, EntryStageProgress, FileSignature,
    IndexedEntryStageState, KnownDirectChild, KnownEntryState, KnownFileEntry, SearchDatabase,
};

/// 单批上限同时约束 SQL 参数、writer 交接和返回状态，容量与索引规模无关。
pub(crate) const MAX_CLASSIFICATION_BATCH_ENTRIES: usize = 128;
pub(crate) const MAX_CLASSIFICATION_BATCH_BYTES: usize = 262_144;
pub(crate) const MAX_KNOWN_ENTRY_PAGE_ENTRIES: usize = 128;
const CLASSIFICATION_FIXED_BYTES_PER_ENTRY: usize = 96;

// 将精确 scope 与后代范围用 OR 合并，会让 SQLite 为每个 128 行页面排序完整 scope。
// 两个互斥的有序分支可保持主键游标流式推进；`likely` 避免低选择性的可见性索引替代该游标。
const KNOWN_FILES_PAGE_SQL: &str = "SELECT * FROM (
                SELECT
                    f.path,
                    f.device,
                    f.inode,
                    f.mtime_ns,
                    f.ctime_ns,
                    f.size,
                    f.mime_type,
                    s.metadata_stage_state,
                    s.content_stage_state,
                    f.observation_state
                FROM files AS f
                LEFT JOIN file_stage_state AS s ON s.path = f.path
                WHERE likely(f.tombstoned = 0)
                  AND f.path > ?4
                  AND f.path = ?1
                  AND NOT EXISTS (
                       SELECT 1
                       FROM directory_snapshots AS parent_directory
                       WHERE parent_directory.path = f.parent_path
                         AND parent_directory.observation_state = 'inaccessible'
                  )
                UNION ALL
                SELECT
                    f.path,
                    f.device,
                    f.inode,
                    f.mtime_ns,
                    f.ctime_ns,
                    f.size,
                    f.mime_type,
                    s.metadata_stage_state,
                    s.content_stage_state,
                    f.observation_state
                FROM files AS f
                LEFT JOIN file_stage_state AS s ON s.path = f.path
                WHERE likely(f.tombstoned = 0)
                  AND f.path > ?4
                  AND f.path > ?1
                  AND f.path >= ?2
                  AND f.path < ?3
                  AND NOT EXISTS (
                       SELECT 1
                       FROM directory_snapshots AS parent_directory
                       WHERE parent_directory.path = f.parent_path
                         AND parent_directory.observation_state = 'inaccessible'
                  )
             )
             ORDER BY path
             LIMIT ?5";

const DIRECTORY_SNAPSHOTS_PAGE_SQL: &str = "SELECT * FROM (
                SELECT
                    path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                    observation_state
                FROM directory_snapshots
                WHERE path > ?4
                  AND path = ?1
                UNION ALL
                SELECT
                    path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                    observation_state
                FROM directory_snapshots
                WHERE path > ?4
                  AND path > ?1
                  AND path >= ?2
                  AND path < ?3
             )
             ORDER BY path
             LIMIT ?5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedFile {
    pub path: PathBuf,
    pub signature: FileSignature,
}

impl ObservedFile {
    pub(crate) fn estimated_bytes(&self) -> usize {
        self.path
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .saturating_add(CLASSIFICATION_FIXED_BYTES_PER_ENTRY)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileClassification {
    pub known_entry: Option<KnownEntryState>,
}

impl SearchDatabase {
    /// 只读取持久签名；未变化分类不能顺带写入“本轮已见”状态。
    pub(crate) fn classify_observed_files(
        &self,
        observed_files: &[ObservedFile],
    ) -> SearchResult<Vec<FileClassification>> {
        validate_classification_batch(observed_files)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT
                f.device,
                f.inode,
                f.mtime_ns,
                f.ctime_ns,
                f.size,
                f.mime_type,
                s.metadata_stage_state,
                s.content_stage_state,
                f.observation_state
             FROM files AS f
             LEFT JOIN file_stage_state AS s ON s.path = f.path
             WHERE f.path = ?1 AND f.tombstoned = 0",
        )?;

        observed_files
            .iter()
            .map(|observed_file| {
                let storage_path = path_to_storage(&observed_file.path);
                Ok(FileClassification {
                    known_entry: read_known_entry(&mut statement, &storage_path)?,
                })
            })
            .collect()
    }

    pub(crate) fn known_files_page(
        &self,
        scope: &Path,
        after_path: Option<&Path>,
        limit: usize,
    ) -> SearchResult<Vec<KnownFileEntry>> {
        validate_page_limit(limit)?;
        let scope_range = recursive_storage_range(scope);
        let after_path = after_path.map(path_to_storage).unwrap_or_default();
        let mut statement = self.connection.prepare_cached(KNOWN_FILES_PAGE_SQL)?;
        let rows = statement.query_map(
            params![
                scope_range.exact_path,
                scope_range.descendant_lower,
                scope_range.descendant_upper,
                after_path,
                limit as i64,
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    StoredKnownEntryFields::read(row, 1)?,
                ))
            },
        )?;

        rows.map(|row| {
            let (path, stored_fields) = row?;
            Ok(KnownFileEntry {
                path: path_from_storage_bytes(path),
                state: stored_fields.into_known_entry_state()?,
            })
        })
        .collect()
    }

    pub(crate) fn directory_snapshots_page(
        &self,
        scope: &Path,
        after_path: Option<&Path>,
        limit: usize,
    ) -> SearchResult<Vec<DirectorySnapshot>> {
        validate_page_limit(limit)?;
        let scope_range = recursive_storage_range(scope);
        let after_path = after_path.map(path_to_storage).unwrap_or_default();
        let mut statement = self
            .connection
            .prepare_cached(DIRECTORY_SNAPSHOTS_PAGE_SQL)?;
        let rows = statement.query_map(
            params![
                scope_range.exact_path,
                scope_range.descendant_lower,
                scope_range.descendant_upper,
                after_path,
                limit as i64,
            ],
            read_directory_snapshot_row,
        )?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub(crate) fn directory_snapshot_paths_page(
        &self,
        after_path: Option<&Path>,
        limit: usize,
    ) -> SearchResult<Vec<PathBuf>> {
        validate_page_limit(limit)?;
        let after_path = after_path.map(path_to_storage).unwrap_or_default();
        let mut statement = self.connection.prepare_cached(
            "SELECT path
             FROM directory_snapshots
             WHERE path > ?1 AND observation_state = 'observable'
             ORDER BY path
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after_path, limit as i64], |row| {
            row.get::<_, Vec<u8>>(0).map(path_from_storage_bytes)
        })?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub(crate) fn observable_directory_paths_for_root_page(
        &self,
        root: &Path,
        after_path: Option<&Path>,
        limit: usize,
    ) -> SearchResult<Vec<PathBuf>> {
        validate_page_limit(limit)?;
        let after_path = after_path.map(path_to_storage).unwrap_or_default();
        let mut statement = self.connection.prepare_cached(
            "SELECT path
             FROM directory_snapshots
             WHERE root_path = ?1 AND path > ?2 AND observation_state = 'observable'
             ORDER BY path
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![path_to_storage(root), after_path, limit as i64],
            |row| row.get::<_, Vec<u8>>(0).map(path_from_storage_bytes),
        )?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub(crate) fn directory_snapshot(
        &self,
        path: &Path,
    ) -> SearchResult<Option<DirectorySnapshot>> {
        self.connection
            .query_row(
                "SELECT path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                        observation_state
                 FROM directory_snapshots
                 WHERE path = ?1",
                params![path_to_storage(path)],
                read_directory_snapshot_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn upsert_directory_snapshot(
        &self,
        snapshot: &DirectorySnapshot,
    ) -> SearchResult<()> {
        self.connection.execute(
            "INSERT INTO directory_snapshots (
                path, parent_path, root_path, device, inode, mtime_ns, ctime_ns,
                observation_state
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET
                parent_path = excluded.parent_path,
                root_path = excluded.root_path,
                device = excluded.device,
                inode = excluded.inode,
                mtime_ns = excluded.mtime_ns,
                ctime_ns = excluded.ctime_ns,
                observation_state = excluded.observation_state",
            params![
                path_to_storage(&snapshot.path),
                path_to_storage(&snapshot.parent_path),
                path_to_storage(&snapshot.root_path),
                snapshot.signature.device as i64,
                snapshot.signature.inode as i64,
                snapshot.signature.mtime_ns,
                snapshot.signature.ctime_ns,
                snapshot.observation_state.as_storage_value(),
            ],
        )?;
        Ok(())
    }

    pub(crate) fn direct_children_page(
        &self,
        parent: &Path,
        after_path: Option<&Path>,
        limit: usize,
    ) -> SearchResult<Vec<KnownDirectChild>> {
        validate_page_limit(limit)?;
        let parent = path_to_storage(parent);
        let after_path = after_path.map(path_to_storage).unwrap_or_default();
        let mut statement = self.connection.prepare_cached(
            "SELECT path, entry_kind
             FROM (
                SELECT path, 'file' AS entry_kind
                FROM files
                WHERE parent_path = ?1 AND tombstoned = 0
                UNION ALL
                SELECT path, 'directory' AS entry_kind
                FROM directory_snapshots
                WHERE parent_path = ?1
             )
             WHERE path > ?2
             ORDER BY path
             LIMIT ?3",
        )?;
        let rows = statement.query_map(params![parent, after_path, limit as i64], |row| {
            let path = path_from_storage_bytes(row.get::<_, Vec<u8>>(0)?);
            let kind = SearchFileKind::from_storage_value(&row.get::<_, String>(1)?);
            Ok(KnownDirectChild { path, kind })
        })?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    pub(crate) fn mark_scope_inaccessible(&self, scope: &Path) -> SearchResult<bool> {
        let transaction = self.connection.unchecked_transaction()?;
        let scope_range = recursive_storage_range(scope);
        let files_changed = transaction.execute(
            "UPDATE files
             SET observation_state = 'inaccessible'
             WHERE tombstoned = 0
               AND observation_state <> 'inaccessible'
               AND (
                    path = ?1
                    OR (path >= ?2 AND path < ?3)
               )",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        let directories_changed = transaction.execute(
            "UPDATE directory_snapshots
             SET observation_state = 'inaccessible'
             WHERE observation_state <> 'inaccessible'
               AND (
                    path = ?1
                    OR (path >= ?2 AND path < ?3)
               )",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.commit()?;
        Ok(files_changed > 0 || directories_changed > 0)
    }

    pub(crate) fn delete_scope(&self, scope: &Path) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let scope_range = recursive_storage_range(scope);
        transaction.execute(
            "DELETE FROM file_search_snippets
             WHERE file_rowid IN (
                SELECT rowid FROM files
                WHERE path = ?1 OR (path >= ?2 AND path < ?3)
             )",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.execute(
            "DELETE FROM file_search_fts
             WHERE rowid IN (
                SELECT rowid FROM files
                WHERE path = ?1 OR (path >= ?2 AND path < ?3)
             )",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.execute(
            "DELETE FROM file_stage_state
             WHERE path IN (
                SELECT path FROM files
                WHERE path = ?1 OR (path >= ?2 AND path < ?3)
             )",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.execute(
            "DELETE FROM files
             WHERE path = ?1 OR (path >= ?2 AND path < ?3)",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.execute(
            "DELETE FROM directory_snapshots
             WHERE path = ?1 OR (path >= ?2 AND path < ?3)",
            params![
                &scope_range.exact_path,
                &scope_range.descendant_lower,
                &scope_range.descendant_upper,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn directory_snapshot_count(&self) -> SearchResult<u64> {
        self.connection
            .query_row("SELECT COUNT(*) FROM directory_snapshots", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.max(0) as u64)
            .map_err(Into::into)
    }
}

pub(crate) fn validate_classification_batch(observed_files: &[ObservedFile]) -> SearchResult<()> {
    let estimated_bytes = observed_files.iter().fold(0_usize, |total, observed_file| {
        total.saturating_add(observed_file.estimated_bytes())
    });
    if observed_files.len() > MAX_CLASSIFICATION_BATCH_ENTRIES
        || estimated_bytes > MAX_CLASSIFICATION_BATCH_BYTES
    {
        return Err(SearchError::InvalidQuery(format!(
            "scan classification batch exceeds capacity: {} entries, {} bytes",
            observed_files.len(),
            estimated_bytes
        )));
    }
    Ok(())
}

fn validate_page_limit(limit: usize) -> SearchResult<()> {
    if limit == 0 || limit > MAX_KNOWN_ENTRY_PAGE_ENTRIES {
        return Err(SearchError::InvalidQuery(format!(
            "known entry page size {limit} is outside 1..={MAX_KNOWN_ENTRY_PAGE_ENTRIES}"
        )));
    }
    Ok(())
}

fn read_known_entry(
    statement: &mut rusqlite::Statement<'_>,
    storage_path: &[u8],
) -> SearchResult<Option<KnownEntryState>> {
    let stored_fields = statement
        .query_row(params![storage_path], |row| {
            StoredKnownEntryFields::read(row, 0)
        })
        .optional()?;

    stored_fields
        .map(StoredKnownEntryFields::into_known_entry_state)
        .transpose()
}

struct StoredKnownEntryFields {
    device: Option<i64>,
    inode: Option<i64>,
    mtime_ns: Option<i64>,
    ctime_ns: Option<i64>,
    size: i64,
    mime_type: Option<String>,
    metadata_stage_state: Option<String>,
    content_stage_state: Option<String>,
    observation_state: String,
}

impl StoredKnownEntryFields {
    fn read(row: &rusqlite::Row<'_>, first_column: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            device: row.get(first_column)?,
            inode: row.get(first_column + 1)?,
            mtime_ns: row.get(first_column + 2)?,
            ctime_ns: row.get(first_column + 3)?,
            size: row.get(first_column + 4)?,
            mime_type: row.get(first_column + 5)?,
            metadata_stage_state: row.get(first_column + 6)?,
            content_stage_state: row.get(first_column + 7)?,
            observation_state: row.get(first_column + 8)?,
        })
    }

    fn into_known_entry_state(self) -> SearchResult<KnownEntryState> {
        let stage_state = match (self.metadata_stage_state, self.content_stage_state) {
            (Some(metadata), Some(content)) => IndexedEntryStageState {
                metadata: EntryStageProgress::from_storage_value(&metadata)?,
                content: EntryStageProgress::from_storage_value(&content)?,
            },
            _ => IndexedEntryStageState::pending(),
        };
        Ok(KnownEntryState {
            signature: FileSignature {
                device: self.device.map(|value| value as u64),
                inode: self.inode.map(|value| value as u64),
                mtime_ns: self.mtime_ns,
                ctime_ns: self.ctime_ns,
                size: self.size.max(0) as u64,
            },
            stage_state,
            mime_type: self.mime_type,
            observation_state: EntryObservationState::from_storage_value(&self.observation_state)?,
        })
    }
}

fn read_directory_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DirectorySnapshot> {
    let observation_state = row.get::<_, String>(7)?;
    let observation_state = match observation_state.as_str() {
        "observable" => EntryObservationState::Observable,
        "inaccessible" => EntryObservationState::Inaccessible,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported directory observation state: {observation_state}"),
                )),
            ))
        }
    };
    Ok(DirectorySnapshot {
        path: path_from_storage_bytes(row.get::<_, Vec<u8>>(0)?),
        parent_path: path_from_storage_bytes(row.get::<_, Vec<u8>>(1)?),
        root_path: path_from_storage_bytes(row.get::<_, Vec<u8>>(2)?),
        signature: DirectorySignature {
            device: row.get::<_, i64>(3)? as u64,
            inode: row.get::<_, i64>(4)? as u64,
            mtime_ns: row.get(5)?,
            ctime_ns: row.get(6)?,
        },
        observation_state,
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recursive_page_queries_stream_in_path_order_without_temporary_sorting() {
        let directory = tempdir().unwrap();
        let database = SearchDatabase::open(&directory.path().join("search.sqlite")).unwrap();
        let scope_range = recursive_storage_range(Path::new("/tmp/root"));

        for (label, sql, path_cursor) in [
            (
                "known files",
                KNOWN_FILES_PAGE_SQL,
                "SEARCH f USING INDEX sqlite_autoindex_files_1",
            ),
            (
                "directory snapshots",
                DIRECTORY_SNAPSHOTS_PAGE_SQL,
                "SEARCH directory_snapshots USING INDEX sqlite_autoindex_directory_snapshots_1",
            ),
        ] {
            let mut statement = database
                .connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let plan = statement
                .query_map(
                    params![
                        &scope_range.exact_path,
                        &scope_range.descendant_lower,
                        &scope_range.descendant_upper,
                        Vec::<u8>::new(),
                        MAX_KNOWN_ENTRY_PAGE_ENTRIES as i64,
                    ],
                    |row| row.get::<_, String>(3),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            let path_cursor_steps = plan
                .iter()
                .filter(|step| step.contains(path_cursor))
                .count();
            assert_eq!(
                path_cursor_steps, 2,
                "{label} page query does not use the path primary-key cursor in both branches: {plan:#?}"
            );
            assert!(
                !plan.iter().any(|step| step.contains("USE TEMP B-TREE")),
                "{label} page query sorts the recursive scope: {plan:#?}"
            );
        }
    }
}
