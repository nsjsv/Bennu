use std::error::Error;
use std::fmt;

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
