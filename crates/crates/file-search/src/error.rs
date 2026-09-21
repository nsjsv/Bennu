use std::path::PathBuf;

use thiserror::Error;

use crate::model::SearchProviderFailure;

pub type SearchResult<T> = Result<T, SearchError>;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("I/O failed for {path:?}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("search cannot access {path:?}: {source}")]
    Inaccessible {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("search watch error: {0}")]
    Watch(#[from] notify::Error),
    #[error("search database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("search database schema {found} is newer than supported schema {supported}")]
    UnsupportedDatabaseSchema { found: i64, supported: i64 },
    #[error("invalid search database schema: {message}")]
    InvalidDatabaseSchema { message: String },
    #[error("search database storage migration failed for {database_path:?}: {message}")]
    DatabaseStorageMigrationFailed {
        database_path: PathBuf,
        message: String,
    },
    #[error("managed search index member is not a regular file: {path:?}")]
    InvalidManagedIndexMember { path: PathBuf },
    #[error(
        "could not quarantine damaged search index {database_path:?}; quarantine directory: {quarantine_directory:?}; {message}"
    )]
    ManagedIndexQuarantineFailed {
        database_path: PathBuf,
        quarantine_directory: Option<PathBuf>,
        message: String,
    },
    #[error(
        "damaged search index {database_path:?} was quarantined at {quarantine_directory:?}, but replacement database failed: {source}"
    )]
    ManagedIndexRebuildFailed {
        database_path: PathBuf,
        quarantine_directory: PathBuf,
        source: Box<SearchError>,
    },
    #[error("search protocol JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("search protocol I/O error: {0}")]
    ProtocolIo(#[from] std::io::Error),
    #[error("search protocol frame is too large: {0} bytes")]
    ProtocolFrameTooLarge(u32),
    #[error("search socket is already owned: {path:?}")]
    SocketAlreadyOwned { path: PathBuf },
    #[error("invalid search query: {0}")]
    InvalidQuery(String),
    #[error("invalid search service configuration: {0}")]
    InvalidConfiguration(String),
    #[error("{boundary} payload is too large: {actual_bytes} bytes exceeds {max_bytes}")]
    PayloadTooLarge {
        boundary: &'static str,
        actual_bytes: usize,
        max_bytes: usize,
    },
    #[error("search query {query_id} failed: {failure}")]
    SearchFailed {
        query_id: u64,
        failure: SearchProviderFailure,
    },
    #[error("search operation was cancelled")]
    Cancelled,
    #[error("search worker failed: {0}")]
    WorkerFailed(String),
}
