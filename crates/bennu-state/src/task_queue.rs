//! 任务队列：用户发起的文件操作的持久记录。可恢复传输的日志表与租约在
//! `recoverable_transfer`，同样挂在 `TaskQueueStore` 上。

use std::path::Path;

use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::database::{current_time_ms, StateDatabase};
use crate::{
    StoreError, StoreResult, StoredFileName, StoredFileOperationVerification, StoredPath,
    StoredTransferConflictStrategy,
};

pub(crate) const SCHEMA_SQL: &str = r#"
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
"#;

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
        new_name: StoredFileName,
    },
    BatchRename {
        items: Vec<StoredBatchRenameItem>,
    },
    CreateDirectory {
        parent: StoredPath,
    },
    /// 高级新建文件夹：一次创建多个目录，可选把选中条目移入第一个。
    CreateDirectories {
        parent: StoredPath,
        names: Vec<String>,
        gather_sources: Vec<StoredPath>,
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
        /// 加入前写下的行没有这个字段，解码为空。
        #[serde(default)]
        created_directories: Vec<StoredPath>,
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
    /// 包内成员提取是一次性动作,不可重启恢复;存储只保留事实记录,
    /// 密码本体绝不落盘,只记是否需要密码(比照 ExtractArchive)。
    ExtractArchiveMembers {
        sources: Vec<StoredPath>,
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
            Self::CreateDirectories { .. } => "create_directories",
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
            Self::ExtractArchiveMembers { .. } => "extract_archive_members",
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

#[derive(Debug, Clone)]
pub struct TaskQueueStore {
    pub(crate) database: StateDatabase,
}

impl From<StateDatabase> for TaskQueueStore {
    fn from(database: StateDatabase) -> Self {
        Self { database }
    }
}

impl TaskQueueStore {
    pub fn new(db_path: impl AsRef<Path>) -> StoreResult<Self> {
        StateDatabase::open(db_path).map(Self::from)
    }

    /// 进程里的设置、会话、关机提交与任务队列共用这一个数据库句柄。
    pub fn database(&self) -> &StateDatabase {
        &self.database
    }

    pub fn db_path(&self) -> &Path {
        self.database.path()
    }

    pub fn insert_task(&self, operation: &StoredOperation) -> StoreResult<u64> {
        let connection = self.database.write();
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
        let connection = self.database.read();
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
        let mut tasks = Vec::new();
        let mut invalid_task_ids = Vec::new();
        {
            let connection = self.database.read();
            let mut statement = connection.prepare(
                "SELECT id, payload_json, status, progress_fraction, error, created_at_ms, updated_at_ms
                 FROM task_queue
                 ORDER BY id ASC",
            )?;
            let rows = statement.query_map([], task_row_from_sql)?;
            for row in rows {
                let task_row = row?;
                let id = task_row.id;
                match StoredTask::try_from(task_row) {
                    Ok(task) => tasks.push(task),
                    Err(StoreError::Json(_)) => invalid_task_ids.push(id),
                    Err(error) => return Err(error),
                }
            }
        }
        if !invalid_task_ids.is_empty() {
            let connection = self.database.write();
            for id in invalid_task_ids {
                connection.execute("DELETE FROM task_queue WHERE id = ?1", params![id])?;
            }
        }
        Ok(tasks)
    }

    pub fn update_status(&self, id: u64, status: StoredTaskStatus) -> StoreResult<()> {
        let connection = self.database.write();
        connection.execute(
            "UPDATE task_queue SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![status.as_str(), current_time_ms(), id],
        )?;
        Ok(())
    }

    pub fn update_progress(&self, id: u64, progress: StoredProgress) -> StoreResult<()> {
        let connection = self.database.write();
        connection.execute(
            "UPDATE task_queue SET progress_fraction = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![progress.fraction, current_time_ms(), id],
        )?;
        Ok(())
    }

    pub fn update_error(&self, id: u64, error: Option<&str>) -> StoreResult<()> {
        let connection = self.database.write();
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
        let connection = self.database.write();
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
        let connection = self.database.write();
        connection.execute("DELETE FROM task_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_tasks(&self, ids: &[u64]) -> StoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.database.write();
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
        let connection = self.database.write();
        connection.execute("DELETE FROM task_queue", [])?;
        Ok(())
    }

    pub fn mark_unfinished_tasks_failed(&self, error: &str) -> StoreResult<()> {
        let connection = self.database.write();
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

pub(crate) fn sqlite_id_to_u64(id: i64) -> StoreResult<u64> {
    u64::try_from(id).map_err(|_| StoreError::InvalidTaskId(id))
}
