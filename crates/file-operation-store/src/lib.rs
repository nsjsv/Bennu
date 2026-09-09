use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

mod browser_session;
pub use browser_session::{
    StoredBrowserPane, StoredBrowserPaneLayout, StoredBrowserSession, StoredBrowserTab,
    StoredBrowserViewMode, StoredColumnBrowserViewport, StoredColumnViewport, StoredSplitAxis,
};
mod recoverable_transfer;
pub use recoverable_transfer::{
    ClaimedRecoverableTask, RecoverableRestoreCoordinatorLease, RecoverableTaskRunnerLease,
    StoredFileIdentity, StoredFileObjectKind, StoredFileOperationVerification,
    StoredManifestCheckpointBatchUpdate, StoredManifestEntry, StoredMergeChildCompletion,
    StoredTransferCheckpoint, StoredTransferCheckpointKind, StoredTransferCheckpointSwap,
    StoredTransferConflictStrategy, StoredTransferJournalEntry, StoredTransferOperation,
    StoredTransferRecoverySnapshot, StoredTransferWorkKey, TransferManifestCheckpointUpdate,
    TRANSFER_JOURNAL_VERSION,
};
mod user_preferences;
pub use user_preferences::{
    StoredContextMenuItemEntry, StoredContextMenuLayout, StoredContextMenuLayouts,
    StoredCustomColorScheme, StoredCustomColorSet, StoredListViewColumn, StoredNetworkConnection,
    StoredPreviewExtensionRules, StoredShortcutBinding, StoredSidebarFavorite,
    StoredUserPreferences, StoredWindowControlPlacement, LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING,
    LAUNCH_WINDOW_POLICY_OPEN_NEW_WINDOW,
};

#[cfg(test)]
mod tests;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS task_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    progress_fraction REAL,
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ui_column_view_preferences (
    preference_key TEXT PRIMARY KEY,
    value_real REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS browser_session (
    session_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS user_preferences (
    preference_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

const COLUMN_WIDTH_PREFERENCE_PREFIX: &str = "column_width.";
const BROWSER_SESSION_KEY: &str = "main";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    InvalidStatus(String),
    InvalidTaskId(i64),
    RecoverableTaskAlreadyRunning {
        task_id: u64,
    },
    InvalidRecoverableOperation(&'static str),
    InvalidTransferValue {
        field: &'static str,
        value: String,
    },
    StaleTransferRevision {
        task_id: u64,
        transfer_index: u64,
        expected_revision: u64,
    },
}

impl StoreError {
    pub fn is_invalid_recovery_data(&self) -> bool {
        matches!(
            self,
            Self::Json(_)
                | Self::InvalidStatus(_)
                | Self::InvalidTaskId(_)
                | Self::InvalidRecoverableOperation(_)
                | Self::InvalidTransferValue { .. }
        )
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::Json(error) => write!(formatter, "JSON error: {error}"),
            Self::InvalidStatus(status) => write!(formatter, "invalid task status: {status}"),
            Self::InvalidTaskId(id) => write!(formatter, "invalid SQLite task id: {id}"),
            Self::RecoverableTaskAlreadyRunning { task_id } => {
                write!(formatter, "recoverable task {task_id} is already running")
            }
            Self::InvalidRecoverableOperation(message) => {
                write!(formatter, "invalid recoverable operation: {message}")
            }
            Self::InvalidTransferValue { field, value } => {
                write!(formatter, "invalid transfer {field}: {value}")
            }
            Self::StaleTransferRevision {
                task_id,
                transfer_index,
                expected_revision,
            } => write!(
                formatter,
                "stale transfer revision for task {task_id}, transfer {transfer_index}: expected {expected_revision}"
            ),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidStatus(_)
            | Self::InvalidTaskId(_)
            | Self::RecoverableTaskAlreadyRunning { .. }
            | Self::InvalidRecoverableOperation(_)
            | Self::InvalidTransferValue { .. }
            | Self::StaleTransferRevision { .. } => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum StoredPath {
    #[cfg(unix)]
    UnixBytes {
        bytes: Vec<u8>,
    },
    #[cfg(windows)]
    WindowsWide {
        units: Vec<u16>,
    },
    Utf8 {
        value: String,
    },
}

impl StoredPath {
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            Self::UnixBytes {
                bytes: path.as_os_str().as_bytes().to_vec(),
            }
        }

        #[cfg(windows)]
        {
            return Self::WindowsWide {
                units: path.as_os_str().encode_wide().collect(),
            };
        }

        #[cfg(not(any(unix, windows)))]
        {
            Self::Utf8 {
                value: path.to_string_lossy().into_owned(),
            }
        }
    }

    pub fn to_path_buf(&self) -> PathBuf {
        match self {
            #[cfg(unix)]
            Self::UnixBytes { bytes } => PathBuf::from(OsString::from_vec(bytes.clone())),
            #[cfg(windows)]
            Self::WindowsWide { units } => PathBuf::from(OsString::from_wide(units)),
            Self::Utf8 { value } => PathBuf::from(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTransfer {
    pub source: StoredPath,
    pub target: StoredPath,
    #[serde(default)]
    pub conflict_strategy: StoredTransferConflictStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBatchRenameItem {
    pub from: StoredPath,
    pub to: StoredPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSymbolicLinkCreation {
    pub link_path: StoredPath,
    pub target_path: StoredPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTrashEntry {
    pub trash_path: StoredPath,
    pub info_path: StoredPath,
    pub original_path: StoredPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredArchiveFormat {
    Zip,
    SevenZip,
    TarGz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredArchiveCompressionLevel {
    Store,
    Fast,
    Balanced,
    Maximum,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredOperation {
    Rename {
        path: StoredPath,
        new_name: String,
    },
    BatchRename {
        items: Vec<StoredBatchRenameItem>,
    },
    CreateDirectory {
        parent: StoredPath,
    },
    CreateEmptyFile {
        parent: StoredPath,
    },
    Trash {
        paths: Vec<StoredPath>,
    },
    Restore {
        entries: Vec<StoredTrashEntry>,
    },
    DeleteTrashEntries {
        entries: Vec<StoredTrashEntry>,
    },
    DeletePermanently {
        paths: Vec<StoredPath>,
    },
    EmptyTrash,
    Copy {
        transfers: Vec<StoredTransfer>,
        #[serde(default)]
        verification: StoredFileOperationVerification,
        #[serde(default)]
        recovery_version: Option<u32>,
    },
    GatherSelectionIntoNewFolder {
        directory: StoredPath,
        sources: Vec<StoredPath>,
    },
    UngatherNewFolder {
        directory: StoredPath,
        restore_targets: Vec<StoredPath>,
    },
    CreateSymbolicLinks {
        links: Vec<StoredSymbolicLinkCreation>,
    },
    Move {
        transfers: Vec<StoredTransfer>,
        #[serde(default)]
        verification: StoredFileOperationVerification,
        #[serde(default)]
        recovery_version: Option<u32>,
    },
    CreateArchive {
        sources: Vec<StoredPath>,
        target: StoredPath,
        format: StoredArchiveFormat,
        compression_level: StoredArchiveCompressionLevel,
        password_required: bool,
    },
    ExtractArchive {
        archive: StoredPath,
        destination: StoredPath,
        password_required: bool,
    },
    /// 格式转换任务;转换不可恢复,存储只保留历史展示所需的源与目标扩展名。
    Convert {
        sources: Vec<StoredPath>,
        output_extensions: Vec<String>,
    },
}

impl StoredOperation {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "rename",
            Self::BatchRename { .. } => "batch_rename",
            Self::CreateDirectory { .. } => "create_directory",
            Self::CreateEmptyFile { .. } => "create_empty_file",
            Self::Trash { .. } => "trash",
            Self::Restore { .. } => "restore",
            Self::DeleteTrashEntries { .. } => "delete_trash_entries",
            Self::DeletePermanently { .. } => "delete_permanently",
            Self::EmptyTrash => "empty_trash",
            Self::Copy { .. } => "copy",
            Self::Move { .. } => "move",
            Self::GatherSelectionIntoNewFolder { .. } => "gather_selection_into_new_folder",
            Self::UngatherNewFolder { .. } => "ungather_new_folder",
            Self::CreateSymbolicLinks { .. } => "create_symbolic_links",
            Self::CreateArchive { .. } => "create_archive",
            Self::ExtractArchive { .. } => "extract_archive",
            Self::Convert { .. } => "convert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredTaskStatus {
    Pending,
    Running,
    Paused,
    Canceling,
    RecoveryPending,
    Failed,
    Completed,
    Canceled,
}

impl StoredTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Canceling => "canceling",
            Self::RecoveryPending => "recovery_pending",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
        }
    }

    fn parse(status: String) -> StoreResult<Self> {
        match status.as_str() {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "canceling" => Ok(Self::Canceling),
            "recovery_pending" => Ok(Self::RecoveryPending),
            "failed" => Ok(Self::Failed),
            "completed" => Ok(Self::Completed),
            "canceled" => Ok(Self::Canceled),
            _ => Err(StoreError::InvalidStatus(status)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StoredProgress {
    pub fraction: Option<f64>,
}

impl StoredProgress {
    pub fn pending() -> Self {
        Self { fraction: None }
    }

    pub fn with_fraction(fraction: f64) -> Self {
        Self {
            fraction: Some(fraction.clamp(0.0, 1.0)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredTask {
    pub id: u64,
    pub operation: StoredOperation,
    pub status: StoredTaskStatus,
    pub progress: StoredProgress,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredInterruptedRecoverableTask {
    pub task_id: u64,
    pub status: StoredTaskStatus,
    pub progress: StoredProgress,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StoredBrowserSessionShutdown {
    Skip,
    Persist(StoredBrowserSession),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredApplicationShutdown {
    pub browser_session: StoredBrowserSessionShutdown,
    pub user_preferences: Option<StoredUserPreferences>,
    pub interrupted_recoverable_tasks: Vec<StoredInterruptedRecoverableTask>,
    pub transient_task_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct TaskQueueStore {
    db_path: PathBuf,
}

impl TaskQueueStore {
    pub fn new(db_path: impl Into<PathBuf>) -> StoreResult<Self> {
        let store = Self {
            db_path: db_path.into(),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn initialize(&self) -> StoreResult<()> {
        if let Some(parent) = self
            .db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let connection = self.connection()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(SCHEMA_SQL)?;
        connection.execute_batch(recoverable_transfer::SCHEMA_SQL)?;
        Ok(())
    }

    pub fn insert_task(&self, operation: &StoredOperation) -> StoreResult<u64> {
        let connection = self.connection()?;
        let now = current_time_ms();
        let payload_json = serde_json::to_string(operation)?;
        connection.execute(
            "INSERT INTO task_queue (
                operation_kind, payload_json, status, progress_fraction, error,
                created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4)",
            params![
                operation.kind(),
                payload_json,
                StoredTaskStatus::Pending.as_str(),
                now
            ],
        )?;
        sqlite_id_to_u64(connection.last_insert_rowid())
    }

    pub fn read_task(&self, id: u64) -> StoreResult<Option<StoredTask>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, payload_json, status, progress_fraction, error, created_at_ms, updated_at_ms
             FROM task_queue
             WHERE id = ?1",
        )?;
        let row = statement
            .query_row(params![id], task_row_from_sql)
            .optional()?;
        row.map(StoredTask::try_from).transpose()
    }

    pub fn read_tasks(&self) -> StoreResult<Vec<StoredTask>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, payload_json, status, progress_fraction, error, created_at_ms, updated_at_ms
             FROM task_queue
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], task_row_from_sql)?;
        let mut tasks = Vec::new();
        let mut invalid_task_ids = Vec::new();
        for row in rows {
            let task_row = row?;
            let id = task_row.id;
            match StoredTask::try_from(task_row) {
                Ok(task) => tasks.push(task),
                Err(StoreError::Json(_)) => invalid_task_ids.push(id),
                Err(error) => return Err(error),
            }
        }
        drop(statement);
        for id in invalid_task_ids {
            connection.execute("DELETE FROM task_queue WHERE id = ?1", params![id])?;
        }
        Ok(tasks)
    }

    pub fn update_status(&self, id: u64, status: StoredTaskStatus) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE task_queue SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![status.as_str(), current_time_ms(), id],
        )?;
        Ok(())
    }

    pub fn update_progress(&self, id: u64, progress: StoredProgress) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE task_queue SET progress_fraction = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![progress.fraction, current_time_ms(), id],
        )?;
        Ok(())
    }

    pub fn update_error(&self, id: u64, error: Option<&str>) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE task_queue SET error = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![error, current_time_ms(), id],
        )?;
        Ok(())
    }

    pub fn update_task_state(
        &self,
        id: u64,
        status: StoredTaskStatus,
        progress: StoredProgress,
        error: Option<&str>,
    ) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE task_queue
             SET status = ?1, progress_fraction = ?2, error = ?3, updated_at_ms = ?4
             WHERE id = ?5",
            params![
                status.as_str(),
                progress.fraction,
                error,
                current_time_ms(),
                id
            ],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: u64) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute("DELETE FROM task_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_tasks(&self, ids: &[u64]) -> StoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare("DELETE FROM task_queue WHERE id = ?1")?;
            for id in ids {
                statement.execute(params![id])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn clear_tasks(&self) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute("DELETE FROM task_queue", [])?;
        Ok(())
    }

    pub fn read_column_widths(&self) -> StoreResult<HashMap<usize, f64>> {
        read_indexed_column_widths(&self.connection()?)
    }

    pub fn replace_column_widths(&self, widths: HashMap<usize, f64>) -> StoreResult<()> {
        let connection = self.connection()?;
        let indexed_pattern = format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}%");
        connection.execute(
            "DELETE FROM ui_column_view_preferences
             WHERE preference_key LIKE ?1",
            params![indexed_pattern],
        )?;

        let mut widths = widths
            .into_iter()
            .filter(|(_, width)| width.is_finite())
            .collect::<Vec<_>>();
        widths.sort_by_key(|(column_index, _)| *column_index);
        for (column_index, width) in widths {
            connection.execute(
                "INSERT INTO ui_column_view_preferences (preference_key, value_real)
                 VALUES (?1, ?2)",
                params![column_width_preference_key(column_index), width],
            )?;
        }
        Ok(())
    }

    pub fn mark_unfinished_tasks_failed(&self, error: &str) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE task_queue
             SET status = ?1, error = ?2, updated_at_ms = ?3
             WHERE status IN (?4, ?5, ?6, ?7)
               AND NOT EXISTS (
                   SELECT 1 FROM transfer_journal
                   WHERE transfer_journal.task_id = task_queue.id
               )",
            params![
                StoredTaskStatus::Failed.as_str(),
                error,
                current_time_ms(),
                StoredTaskStatus::Pending.as_str(),
                StoredTaskStatus::Running.as_str(),
                StoredTaskStatus::Paused.as_str(),
                StoredTaskStatus::Canceling.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn commit_application_shutdown(
        &self,
        shutdown: StoredApplicationShutdown,
    ) -> StoreResult<()> {
        let StoredApplicationShutdown {
            browser_session,
            user_preferences,
            interrupted_recoverable_tasks,
            transient_task_ids,
        } = shutdown;
        let browser_session_json = match &browser_session {
            StoredBrowserSessionShutdown::Skip => None,
            StoredBrowserSessionShutdown::Persist(session) => Some(serde_json::to_string(session)?),
        };
        let user_preferences_json = user_preferences
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        for task in interrupted_recoverable_tasks {
            if !matches!(
                task.status,
                StoredTaskStatus::RecoveryPending | StoredTaskStatus::Canceling
            ) || !task_has_recovery_rows(&transaction, task.task_id)?
            {
                return Err(StoreError::InvalidRecoverableOperation(
                    "shutdown recoverable task has no preserved recovery state",
                ));
            }
            let updated = transaction.execute(
                "UPDATE task_queue
                 SET status = ?1, progress_fraction = ?2, error = ?3, updated_at_ms = ?4
                 WHERE id = ?5",
                params![
                    task.status.as_str(),
                    task.progress.fraction,
                    task.error,
                    current_time_ms(),
                    task.task_id
                ],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidRecoverableOperation(
                    "shutdown recoverable task row is missing",
                ));
            }
        }

        for task_id in transient_task_ids {
            if task_has_recovery_rows(&transaction, task_id)? {
                return Err(StoreError::InvalidRecoverableOperation(
                    "transient shutdown task owns recovery state",
                ));
            }
            transaction.execute("DELETE FROM task_queue WHERE id = ?1", params![task_id])?;
        }

        if let Some(payload_json) = user_preferences_json {
            transaction.execute(
                "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(preference_key) DO UPDATE SET
                     payload_json = excluded.payload_json,
                     updated_at_ms = excluded.updated_at_ms",
                params![
                    user_preferences::USER_PREFERENCES_KEY,
                    payload_json,
                    current_time_ms()
                ],
            )?;
        }
        if let Some(payload_json) = browser_session_json {
            transaction.execute(
                "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_key) DO UPDATE SET
                     payload_json = excluded.payload_json,
                     updated_at_ms = excluded.updated_at_ms",
                params![BROWSER_SESSION_KEY, payload_json, current_time_ms()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn read_browser_session(&self) -> StoreResult<Option<StoredBrowserSession>> {
        let connection = self.connection()?;
        let payload_json = connection
            .query_row(
                "SELECT payload_json FROM browser_session WHERE session_key = ?1",
                params![BROWSER_SESSION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match payload_json {
            Some(payload_json) => match serde_json::from_str(&payload_json) {
                Ok(session) => Ok(Some(session)),
                Err(error) => {
                    connection.execute(
                        "DELETE FROM browser_session WHERE session_key = ?1",
                        params![BROWSER_SESSION_KEY],
                    )?;
                    Err(StoreError::Json(error))
                }
            },
            None => Ok(None),
        }
    }

    pub fn replace_browser_session(&self, session: &StoredBrowserSession) -> StoreResult<()> {
        let connection = self.connection()?;
        let payload_json = serde_json::to_string(session)?;
        connection.execute(
            "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 updated_at_ms = excluded.updated_at_ms",
            params![BROWSER_SESSION_KEY, payload_json, current_time_ms()],
        )?;
        Ok(())
    }

    pub fn clear_browser_session(&self) -> StoreResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "DELETE FROM browser_session WHERE session_key = ?1",
            params![BROWSER_SESSION_KEY],
        )?;
        Ok(())
    }

    fn connection(&self) -> StoreResult<Connection> {
        let connection = Connection::open(&self.db_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(connection)
    }
}

struct TaskRow {
    id: i64,
    payload_json: String,
    status: String,
    progress_fraction: Option<f64>,
    error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl TryFrom<TaskRow> for StoredTask {
    type Error = StoreError;

    fn try_from(row: TaskRow) -> StoreResult<Self> {
        Ok(Self {
            id: sqlite_id_to_u64(row.id)?,
            operation: serde_json::from_str(&row.payload_json)?,
            status: StoredTaskStatus::parse(row.status)?,
            progress: StoredProgress {
                fraction: row.progress_fraction,
            },
            error: row.error,
            created_at_ms: row.created_at_ms,
            updated_at_ms: row.updated_at_ms,
        })
    }
}

fn task_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        payload_json: row.get(1)?,
        status: row.get(2)?,
        progress_fraction: row.get(3)?,
        error: row.get(4)?,
        created_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
    })
}

fn sqlite_id_to_u64(id: i64) -> StoreResult<u64> {
    u64::try_from(id).map_err(|_| StoreError::InvalidTaskId(id))
}

fn read_indexed_column_widths(connection: &Connection) -> StoreResult<HashMap<usize, f64>> {
    let indexed_pattern = format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}%");
    let mut statement = connection.prepare(
        "SELECT preference_key, value_real
         FROM ui_column_view_preferences
         WHERE preference_key LIKE ?1
         ORDER BY preference_key ASC",
    )?;
    let rows = statement.query_map(params![indexed_pattern], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;

    let mut widths = HashMap::new();
    for row in rows {
        let (key, width) = row?;
        let Some(index) = key
            .strip_prefix(COLUMN_WIDTH_PREFERENCE_PREFIX)
            .and_then(|index| index.parse::<usize>().ok())
        else {
            continue;
        };
        if width.is_finite() {
            widths.insert(index, width);
        }
    }
    Ok(widths)
}

fn column_width_preference_key(column_index: usize) -> String {
    format!("{COLUMN_WIDTH_PREFERENCE_PREFIX}{column_index}")
}

fn task_has_recovery_rows(
    transaction: &rusqlite::Transaction<'_>,
    task_id: u64,
) -> StoreResult<bool> {
    let has_rows = transaction.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM transfer_journal WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_manifest WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_replacement_manifest WHERE task_id = ?1)
            OR EXISTS(SELECT 1 FROM transfer_merge_completion WHERE task_id = ?1)",
        params![task_id],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(has_rows)
}

fn current_time_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
