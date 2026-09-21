use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::error::{SearchError, SearchResult};
use crate::extractor::{DurableContentStageState, ExtractionStatus};
use crate::model::SearchFileKind;
use crate::path_encoding::{path_from_storage, storage_bytes};

#[path = "database/known_entry.rs"]
mod known_entry;
#[path = "database/migration.rs"]
mod migration;
#[path = "database/path_configuration.rs"]
mod path_configuration;
#[path = "database/scan.rs"]
mod scan;
#[path = "database/schema.rs"]
mod schema;
#[path = "database/search.rs"]
mod search;
#[path = "database/storage_rebuild.rs"]
mod storage_rebuild;
#[cfg(unix)]
#[path = "database/storage_workspace.rs"]
mod storage_workspace;

pub(crate) use known_entry::{
    DirectorySignature, DirectorySnapshot, EntryObservationState, KnownDirectChild, KnownFileEntry,
};
pub use known_entry::{FileSignature, KnownEntryState};
pub(crate) use path_configuration::SearchRootMount;
pub(crate) use scan::{
    validate_classification_batch, FileClassification, ObservedFile,
    MAX_CLASSIFICATION_BATCH_BYTES, MAX_CLASSIFICATION_BATCH_ENTRIES, MAX_KNOWN_ENTRY_PAGE_ENTRIES,
};

/// Bumped whenever the on-disk schema changes in a way that requires a migration.
pub(crate) const SCHEMA_VERSION: i64 = 11;
pub(super) const SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT: usize = 1_024;

const WRITER_PAGE_CACHE_KIB: i64 = 2_048;
const READER_PAGE_CACHE_KIB: i64 = 512;
const NORMAL_BUSY_TIMEOUT_MILLIS: i64 = 5_000;
const WAL_AUTOCHECKPOINT_PAGES: i64 = 128;
const WAL_JOURNAL_LIMIT_BYTES: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStageProgress {
    Pending,
    Complete,
    Skipped,
}

impl EntryStageProgress {
    fn as_storage_value(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Complete => "complete",
            Self::Skipped => "skipped",
        }
    }

    fn from_storage_value(value: &str) -> SearchResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "complete" => Ok(Self::Complete),
            "skipped" => Ok(Self::Skipped),
            _ => Err(SearchError::InvalidQuery(format!(
                "unsupported entry stage progress: {value}"
            ))),
        }
    }

    fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedEntryStageState {
    pub metadata: EntryStageProgress,
    pub content: EntryStageProgress,
}

impl IndexedEntryStageState {
    fn allows_signature_skip(&self) -> bool {
        !self.metadata.is_pending() && !self.content.is_pending()
    }

    pub fn pending() -> Self {
        Self {
            metadata: EntryStageProgress::Pending,
            content: EntryStageProgress::Pending,
        }
    }

    fn from_legacy_content_status(extraction_status: &ExtractionStatus) -> Self {
        Self {
            metadata: EntryStageProgress::Complete,
            content: content_stage_progress_for_status(extraction_status),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub parent_path: PathBuf,
    pub display_name: String,
    pub kind: SearchFileKind,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub accessed_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub mime_type: Option<String>,
    pub stage_state: IndexedEntryStageState,
    pub content: Option<String>,
    pub extraction_status: ExtractionStatus,
    pub device: Option<u64>,
    /// Filesystem inode and nanosecond mtime used for incremental crawls.
    /// Mirrors how tracker-miner-fs keys change detection off inode + mtime.
    pub inode: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
}

#[derive(Debug)]
pub(crate) enum ScanFileMutation {
    Observable(IndexedFile),
    Inaccessible { scope: PathBuf, file: IndexedFile },
}

struct QueryVisibleMetadataRow<'file> {
    path: &'file Path,
    parent_path: &'file Path,
    display_name: &'file str,
    kind: &'file SearchFileKind,
    size: u64,
    modified_ms: Option<i64>,
    accessed_ms: Option<i64>,
    created_ms: Option<i64>,
    mime_type: Option<&'file str>,
    extraction_status: &'file ExtractionStatus,
    signature: FileSignature,
}

struct FulltextContentRow<'file> {
    path: &'file Path,
    display_name: &'file str,
    content: Option<&'file str>,
}

impl IndexedFile {
    fn query_visible_metadata_row(&self) -> QueryVisibleMetadataRow<'_> {
        QueryVisibleMetadataRow {
            path: &self.path,
            parent_path: &self.parent_path,
            display_name: &self.display_name,
            kind: &self.kind,
            size: self.size,
            modified_ms: self.modified_ms,
            accessed_ms: self.accessed_ms,
            created_ms: self.created_ms,
            mime_type: self.mime_type.as_deref(),
            extraction_status: &self.extraction_status,
            signature: FileSignature {
                device: self.device,
                inode: self.inode,
                mtime_ns: self.mtime_ns,
                ctime_ns: self.ctime_ns,
                size: self.size,
            },
        }
    }

    fn fulltext_content_row(&self) -> FulltextContentRow<'_> {
        FulltextContentRow {
            path: &self.path,
            display_name: &self.display_name,
            content: self.content.as_deref(),
        }
    }
}

pub struct SearchDatabase {
    connection: Connection,
}

impl SearchDatabase {
    /// Opens the single writable connection. Only the dedicated writer thread
    /// should hold one of these so that all writes are serialized through it,
    /// the way tracker-miner-fs funnels every store operation through one queue.
    pub fn open(path: &Path) -> SearchResult<Self> {
        let connection = Connection::open(path)?;
        verify_supported_schema(&connection)?;
        let connection = storage_rebuild::rebuild_schema_eight_database(path, connection)?;
        configure_writer_connection(&connection)?;
        let database = Self { connection };
        database.initialize()?;
        Ok(database)
    }

    /// 打开受固定页缓存约束的只读连接；连接数量由协议和 daemon reader owner 限制。
    pub fn open_read_only(path: &Path) -> SearchResult<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        verify_supported_schema(&connection)?;
        verify_current_schema(&connection)?;
        schema::verify_search_storage_schema(&connection)?;
        configure_read_only_connection(&connection)?;
        Ok(Self { connection })
    }

    pub fn in_memory() -> SearchResult<Self> {
        let connection = Connection::open_in_memory()?;
        verify_supported_schema(&connection)?;
        configure_writer_connection(&connection)?;
        let database = Self { connection };
        database.initialize()?;
        Ok(database)
    }

    pub(crate) fn interrupt_handle(&self) -> rusqlite::InterruptHandle {
        self.connection.get_interrupt_handle()
    }

    pub fn upsert_file(&self, file: &IndexedFile) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let metadata_row = file.query_visible_metadata_row();
        let fulltext_row = file.fulltext_content_row();
        upsert_query_visible_metadata(&transaction, &metadata_row)?;
        upsert_stage_state(&transaction, metadata_row.path, &file.stage_state)?;
        replace_fulltext_content(&transaction, &fulltext_row)?;
        transaction.commit()?;
        Ok(())
    }

    fn upsert_stage_state_by_storage_path(
        &self,
        storage_path: &str,
        stage_state: &IndexedEntryStageState,
    ) -> SearchResult<()> {
        self.connection.execute(
            "INSERT INTO file_stage_state (path, metadata_stage_state, content_stage_state)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET
                metadata_stage_state = excluded.metadata_stage_state,
                content_stage_state = excluded.content_stage_state",
            params![
                storage_path,
                stage_state.metadata.as_storage_value(),
                stage_state.content.as_storage_value(),
            ],
        )?;
        Ok(())
    }

    pub fn upsert_inaccessible_file(&self, file: &IndexedFile) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let metadata_row = file.query_visible_metadata_row();
        upsert_inaccessible_metadata(&transaction, &metadata_row)?;
        upsert_stage_state(&transaction, metadata_row.path, &file.stage_state)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn apply_file_batch(&self, mutations: &[ScanFileMutation]) -> SearchResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        for mutation in mutations {
            match mutation {
                ScanFileMutation::Observable(file) => {
                    let metadata_row = file.query_visible_metadata_row();
                    let fulltext_row = file.fulltext_content_row();
                    upsert_query_visible_metadata(&transaction, &metadata_row)?;
                    upsert_stage_state(&transaction, metadata_row.path, &file.stage_state)?;
                    replace_fulltext_content(&transaction, &fulltext_row)?;
                }
                ScanFileMutation::Inaccessible { file, .. } => {
                    let metadata_row = file.query_visible_metadata_row();
                    upsert_inaccessible_metadata(&transaction, &metadata_row)?;
                    upsert_stage_state(&transaction, metadata_row.path, &file.stage_state)?;
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn content_status(&self, path: &Path) -> SearchResult<Option<ExtractionStatus>> {
        let status_json: Option<String> = self
            .connection
            .query_row(
                "SELECT content_status FROM files WHERE path = ?1",
                params![path_to_storage(path)],
                |row| row.get(0),
            )
            .optional()?;
        status_json
            .map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub fn entry_stage_state(&self, path: &Path) -> SearchResult<Option<IndexedEntryStageState>> {
        let stage_row: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT metadata_stage_state, content_stage_state
                 FROM file_stage_state
                 WHERE path = ?1",
                params![path_to_storage(path)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        stage_row
            .map(|(metadata_stage_state, content_stage_state)| {
                Ok(IndexedEntryStageState {
                    metadata: EntryStageProgress::from_storage_value(&metadata_stage_state)?,
                    content: EntryStageProgress::from_storage_value(&content_stage_state)?,
                })
            })
            .transpose()
    }
}

pub(crate) fn inspect_existing_schema(path: &Path) -> SearchResult<()> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    verify_supported_schema(&connection)
}

fn verify_supported_schema(connection: &Connection) -> SearchResult<()> {
    let found = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    if found > SCHEMA_VERSION {
        return Err(SearchError::UnsupportedDatabaseSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn verify_current_schema(connection: &Connection) -> SearchResult<()> {
    let found = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    if found != SCHEMA_VERSION {
        return Err(SearchError::InvalidDatabaseSchema {
            message: format!(
                "read-only search requires schema {SCHEMA_VERSION}, found schema {found}"
            ),
        });
    }
    Ok(())
}

fn configure_writer_connection(connection: &Connection) -> SearchResult<()> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "busy_timeout", NORMAL_BUSY_TIMEOUT_MILLIS)?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "cache_size", -WRITER_PAGE_CACHE_KIB)?;
    connection.pragma_update(None, "mmap_size", 0_i64)?;
    connection.pragma_update(None, "temp_store", "FILE")?;
    connection.pragma_update(None, "wal_autocheckpoint", WAL_AUTOCHECKPOINT_PAGES)?;
    connection.pragma_update(None, "journal_size_limit", WAL_JOURNAL_LIMIT_BYTES)?;
    Ok(())
}

fn configure_read_only_connection(connection: &Connection) -> SearchResult<()> {
    connection.pragma_update(None, "busy_timeout", NORMAL_BUSY_TIMEOUT_MILLIS)?;
    connection.pragma_update(None, "cache_size", -READER_PAGE_CACHE_KIB)?;
    connection.pragma_update(None, "mmap_size", 0_i64)?;
    connection.pragma_update(None, "temp_store", "FILE")?;
    Ok(())
}

fn upsert_query_visible_metadata(
    connection: &Connection,
    metadata_row: &QueryVisibleMetadataRow<'_>,
) -> SearchResult<()> {
    let path = path_to_storage(metadata_row.path);
    let parent_path = path_to_storage(metadata_row.parent_path);
    let content_status = serde_json::to_string(metadata_row.extraction_status)?;
    connection
        .prepare_cached(
            "INSERT INTO files (
            path, parent_path, display_name, kind, size, modified_ms, accessed_ms,
            created_ms, mime_type, content_status, device, inode, mtime_ns, ctime_ns, tombstoned,
            observation_state
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, 'observable')
         ON CONFLICT(path) DO UPDATE SET
            parent_path = excluded.parent_path,
            display_name = excluded.display_name,
            kind = excluded.kind,
            size = excluded.size,
            modified_ms = excluded.modified_ms,
            accessed_ms = excluded.accessed_ms,
            created_ms = excluded.created_ms,
            mime_type = excluded.mime_type,
            content_status = excluded.content_status,
            device = excluded.device,
            inode = excluded.inode,
            mtime_ns = excluded.mtime_ns,
            ctime_ns = excluded.ctime_ns,
            tombstoned = 0,
            observation_state = 'observable'",
        )?
        .execute(params![
            path,
            parent_path,
            metadata_row.display_name,
            metadata_row.kind.as_storage_value(),
            metadata_row.size as i64,
            metadata_row.modified_ms,
            metadata_row.accessed_ms,
            metadata_row.created_ms,
            metadata_row.mime_type,
            content_status,
            metadata_row.signature.device.map(|device| device as i64),
            metadata_row.signature.inode.map(|inode| inode as i64),
            metadata_row.signature.mtime_ns,
            metadata_row.signature.ctime_ns,
        ])?;
    Ok(())
}

fn upsert_inaccessible_metadata(
    connection: &Connection,
    metadata_row: &QueryVisibleMetadataRow<'_>,
) -> SearchResult<()> {
    let path = path_to_storage(metadata_row.path);
    let parent_path = path_to_storage(metadata_row.parent_path);
    let content_status = serde_json::to_string(metadata_row.extraction_status)?;
    connection
        .prepare_cached(
            "INSERT INTO files (
            path, parent_path, display_name, kind, size, modified_ms, accessed_ms,
            created_ms, mime_type, content_status, device, inode, mtime_ns, ctime_ns, tombstoned,
            observation_state
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, 'inaccessible')
         ON CONFLICT(path) DO UPDATE SET
            parent_path = excluded.parent_path,
            display_name = excluded.display_name,
            kind = excluded.kind,
            size = excluded.size,
            modified_ms = excluded.modified_ms,
            accessed_ms = excluded.accessed_ms,
            created_ms = excluded.created_ms,
            mime_type = excluded.mime_type,
            device = excluded.device,
            inode = excluded.inode,
            mtime_ns = excluded.mtime_ns,
            ctime_ns = excluded.ctime_ns,
            tombstoned = 0,
            observation_state = 'inaccessible'",
        )?
        .execute(params![
            path,
            parent_path,
            metadata_row.display_name,
            metadata_row.kind.as_storage_value(),
            metadata_row.size as i64,
            metadata_row.modified_ms,
            metadata_row.accessed_ms,
            metadata_row.created_ms,
            metadata_row.mime_type,
            content_status,
            metadata_row.signature.device.map(|device| device as i64),
            metadata_row.signature.inode.map(|inode| inode as i64),
            metadata_row.signature.mtime_ns,
            metadata_row.signature.ctime_ns,
        ])?;
    Ok(())
}

fn upsert_stage_state(
    connection: &Connection,
    path: &Path,
    stage_state: &IndexedEntryStageState,
) -> SearchResult<()> {
    connection
        .prepare_cached(
            "INSERT INTO file_stage_state (path, metadata_stage_state, content_stage_state)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET
            metadata_stage_state = excluded.metadata_stage_state,
            content_stage_state = excluded.content_stage_state",
        )?
        .execute(params![
            path_to_storage(path),
            stage_state.metadata.as_storage_value(),
            stage_state.content.as_storage_value(),
        ])?;
    Ok(())
}

fn replace_fulltext_content(
    connection: &Connection,
    fulltext_row: &FulltextContentRow<'_>,
) -> SearchResult<()> {
    let storage_path = path_to_storage(fulltext_row.path);
    let file_rowid: i64 = connection
        .prepare_cached("SELECT rowid FROM files WHERE path = ?1")?
        .query_row(params![storage_path], |row| row.get(0))?;
    connection
        .prepare_cached("DELETE FROM file_search_fts WHERE rowid = ?1")?
        .execute(params![file_rowid])?;
    connection
        .prepare_cached("DELETE FROM file_search_snippets WHERE file_rowid = ?1")?
        .execute(params![file_rowid])?;
    connection
        .prepare_cached("INSERT INTO file_search_fts (rowid, name, content) VALUES (?1, ?2, ?3)")?
        .execute(params![
            file_rowid,
            fulltext_row.display_name,
            fulltext_row.content.unwrap_or("")
        ])?;
    if let Some(preview) = fulltext_row
        .content
        .and_then(bounded_search_content_preview)
    {
        // 命中身份只属于 FTS；预览受硬上限约束，不能成为正文副本。
        connection
            .prepare_cached(
                "INSERT INTO file_search_snippets (file_rowid, preview) VALUES (?1, ?2)",
            )?
            .execute(params![file_rowid, preview])?;
    }
    Ok(())
}

fn bounded_search_content_preview(content: &str) -> Option<String> {
    let preview = content
        .chars()
        .take(SEARCH_CONTENT_PREVIEW_CHARACTER_LIMIT)
        .collect::<String>();
    (!preview.is_empty()).then_some(preview)
}

fn path_to_storage(path: &Path) -> Vec<u8> {
    storage_bytes(path)
}

fn path_from_storage_bytes(bytes: Vec<u8>) -> PathBuf {
    path_from_storage(bytes)
}

struct RecursiveStorageRange {
    exact_path: Vec<u8>,
    descendant_lower: Vec<u8>,
    descendant_upper: Vec<u8>,
}

fn recursive_storage_range(path: &Path) -> RecursiveStorageRange {
    let exact_path = path_to_storage(path);
    let separator = storage_bytes(Path::new(std::path::MAIN_SEPARATOR_STR));
    let mut descendant_lower = exact_path.clone();
    if !descendant_lower.ends_with(&separator) {
        descendant_lower.extend_from_slice(&separator);
    }
    let mut descendant_upper = descendant_lower.clone();
    let upper_byte_position = descendant_upper
        .iter()
        .rposition(|byte| *byte < u8::MAX)
        .expect("platform path separator storage must have an upper bound");
    descendant_upper[upper_byte_position] += 1;
    descendant_upper.truncate(upper_byte_position + 1);
    RecursiveStorageRange {
        exact_path,
        descendant_lower,
        descendant_upper,
    }
}

fn content_stage_progress_for_status(extraction_status: &ExtractionStatus) -> EntryStageProgress {
    match extraction_status.durable_content_stage_state() {
        DurableContentStageState::Complete => EntryStageProgress::Complete,
        DurableContentStageState::Skipped => EntryStageProgress::Skipped,
    }
}

#[cfg(test)]
#[path = "database/migration_tests.rs"]
mod migration_tests;

#[cfg(all(test, unix))]
#[path = "database/path_identity_tests.rs"]
mod path_identity_tests;

#[cfg(test)]
#[path = "database/schema_tests.rs"]
mod schema_tests;

#[cfg(test)]
#[path = "database/tests.rs"]
mod tests;
