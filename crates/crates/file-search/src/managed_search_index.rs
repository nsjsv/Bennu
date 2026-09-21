use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::ffi::ErrorCode;

use crate::database::{inspect_existing_schema, SearchDatabase};
use crate::error::{SearchError, SearchResult};

static QUARANTINE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexCorruption {
    DatabaseCorrupt,
    NotADatabase,
}

impl IndexCorruption {
    pub(crate) fn sqlite_code(self) -> &'static str {
        match self {
            Self::DatabaseCorrupt => "SQLITE_CORRUPT",
            Self::NotADatabase => "SQLITE_NOTADB",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexRecoveryNotice {
    pub(crate) quarantine_directory: PathBuf,
    pub(crate) corruption: IndexCorruption,
}

impl IndexRecoveryNotice {
    pub(crate) fn message(&self) -> String {
        format!(
            "{} index was quarantined at {} and is rebuilding",
            self.corruption.sqlite_code(),
            self.quarantine_directory.display()
        )
    }
}

pub(crate) struct ManagedIndexOpen {
    pub(crate) database: SearchDatabase,
    pub(crate) recovery_notice: Option<IndexRecoveryNotice>,
}

pub(crate) struct ManagedSearchIndex {
    database_path: PathBuf,
}

impl ManagedSearchIndex {
    pub(crate) fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    pub(crate) fn open(&self) -> SearchResult<ManagedIndexOpen> {
        let existing_paths = self.validate_managed_files()?;
        if existing_paths
            .iter()
            .any(|path| path == &self.database_path)
        {
            if let Err(error) = inspect_existing_schema(&self.database_path) {
                return match index_corruption(&error) {
                    Some(corruption) => self.rebuild_after_corruption(corruption),
                    None => Err(error),
                };
            }
        }
        match SearchDatabase::open(&self.database_path) {
            Ok(database) => Ok(ManagedIndexOpen {
                database,
                recovery_notice: None,
            }),
            Err(error) => match index_corruption(&error) {
                Some(corruption) => self.rebuild_after_corruption(corruption),
                None => Err(error),
            },
        }
    }

    fn rebuild_after_corruption(
        &self,
        corruption: IndexCorruption,
    ) -> SearchResult<ManagedIndexOpen> {
        let quarantine_directory = self.quarantine_managed_files()?;
        match SearchDatabase::open(&self.database_path) {
            Ok(database) => Ok(ManagedIndexOpen {
                database,
                recovery_notice: Some(IndexRecoveryNotice {
                    quarantine_directory,
                    corruption,
                }),
            }),
            Err(source) => Err(SearchError::ManagedIndexRebuildFailed {
                database_path: self.database_path.clone(),
                quarantine_directory,
                source: Box::new(source),
            }),
        }
    }

    fn validate_managed_files(&self) -> SearchResult<Vec<PathBuf>> {
        let mut existing_paths = Vec::new();
        for path in self.managed_paths() {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_file() => existing_paths.push(path),
                Ok(_) => return Err(SearchError::InvalidManagedIndexMember { path }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(SearchError::Io { path, source }),
            }
        }
        Ok(existing_paths)
    }

    fn quarantine_managed_files(&self) -> SearchResult<PathBuf> {
        let managed_paths = self.validate_managed_files()?;
        let quarantine_directory = self.create_quarantine_directory()?;
        self.move_managed_paths(
            managed_paths,
            &quarantine_directory,
            |source, destination| fs::rename(source, destination),
        )?;
        Ok(quarantine_directory)
    }

    fn move_managed_paths(
        &self,
        managed_paths: Vec<PathBuf>,
        quarantine_directory: &Path,
        mut move_path: impl FnMut(&Path, &Path) -> io::Result<()>,
    ) -> SearchResult<()> {
        let mut moved_paths = Vec::with_capacity(managed_paths.len());
        for source in managed_paths {
            let destination = quarantine_directory.join(
                source
                    .file_name()
                    .expect("managed search index path must have a file name"),
            );
            if let Err(error) = move_path(&source, &destination) {
                let rollback_error = rollback_moved_paths(&moved_paths);
                if rollback_error.is_none() {
                    let _ = fs::remove_dir(quarantine_directory);
                }
                return Err(SearchError::ManagedIndexQuarantineFailed {
                    database_path: self.database_path.clone(),
                    quarantine_directory: rollback_error
                        .as_ref()
                        .map(|_| quarantine_directory.to_path_buf()),
                    message: quarantine_failure_message(&source, error, rollback_error),
                });
            }
            moved_paths.push((source, destination));
        }
        Ok(())
    }

    fn create_quarantine_directory(&self) -> SearchResult<PathBuf> {
        let parent = self.database_path.parent().ok_or_else(|| {
            SearchError::ManagedIndexQuarantineFailed {
                database_path: self.database_path.clone(),
                quarantine_directory: None,
                message: "managed search index path has no parent directory".to_owned(),
            }
        })?;
        let timestamp_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        for _ in 0..1_024 {
            let sequence = QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let quarantine_directory = parent.join(format!(
                ".search-index-quarantine-{timestamp_millis}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&quarantine_directory) {
                Ok(()) => return Ok(quarantine_directory),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(SearchError::ManagedIndexQuarantineFailed {
                        database_path: self.database_path.clone(),
                        quarantine_directory: None,
                        message: format!("could not create quarantine directory: {error}"),
                    });
                }
            }
        }
        Err(SearchError::ManagedIndexQuarantineFailed {
            database_path: self.database_path.clone(),
            quarantine_directory: None,
            message: "could not allocate a unique quarantine directory".to_owned(),
        })
    }

    fn managed_paths(&self) -> [PathBuf; 3] {
        [
            self.database_path.clone(),
            sidecar_path(&self.database_path, "-wal"),
            sidecar_path(&self.database_path, "-shm"),
        ]
    }
}

fn index_corruption(error: &SearchError) -> Option<IndexCorruption> {
    let SearchError::Database(database_error) = error else {
        return None;
    };
    match database_error.sqlite_error_code() {
        Some(ErrorCode::DatabaseCorrupt) => Some(IndexCorruption::DatabaseCorrupt),
        Some(ErrorCode::NotADatabase) => Some(IndexCorruption::NotADatabase),
        _ => None,
    }
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = database_path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn rollback_moved_paths(moved_paths: &[(PathBuf, PathBuf)]) -> Option<String> {
    let mut failures = Vec::new();
    for (source, destination) in moved_paths.iter().rev() {
        if let Err(error) = fs::rename(destination, source) {
            failures.push(format!(
                "could not restore {} to {}: {error}",
                destination.display(),
                source.display()
            ));
        }
    }
    (!failures.is_empty()).then(|| failures.join("; "))
}

fn quarantine_failure_message(
    source: &Path,
    error: io::Error,
    rollback_error: Option<String>,
) -> String {
    let mut message = format!("could not move {}: {error}", source.display());
    if let Some(rollback_error) = rollback_error {
        message.push_str("; rollback failed: ");
        message.push_str(&rollback_error);
    }
    message
}

#[cfg(test)]
#[path = "managed_search_index/tests.rs"]
mod tests;
