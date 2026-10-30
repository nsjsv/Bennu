use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::database::{
    validate_classification_batch, DirectorySnapshot, FileClassification, IndexedFile,
    KnownDirectChild, KnownFileEntry, ObservedFile, ScanFileMutation, SearchDatabase,
    SearchRootMount,
};
use crate::error::{SearchError, SearchResult};
use crate::extractor::ExtractionStatus;
use crate::logging::bounded_search_log_detail;
use crate::{SearchPathPolicy, VersionedSearchPathPreferences};

pub(crate) const MAX_WRITER_FILE_PAYLOAD_BYTES: usize = 2_500_000;
const MAX_WRITER_SCOPE_PATH_BYTES: usize = 8_192;
const INDEXED_FILE_FIXED_BYTES: usize = 1_024;

/// A single operation for the dedicated writer thread to apply. Every command
/// replies with its own outcome, so a failed write is reported to the work
/// that issued it and never to whichever request happens to come next.
enum WriteCommand {
    UpsertObservable {
        file: Box<IndexedFile>,
        reply: Sender<SearchResult<()>>,
    },
    ClassifyObserved {
        files: Vec<ObservedFile>,
        reply: Sender<SearchResult<Vec<FileClassification>>>,
    },
    ApplyFileBatch {
        mutations: Vec<ScanFileMutation>,
        reply: Sender<SearchResult<()>>,
    },
    UpsertDirectorySnapshot {
        snapshot: Box<DirectorySnapshot>,
        reply: Sender<SearchResult<()>>,
    },
    DirectorySnapshot {
        path: PathBuf,
        reply: Sender<SearchResult<Option<DirectorySnapshot>>>,
    },
    KnownFilesPage {
        scope: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
        reply: Sender<SearchResult<Vec<KnownFileEntry>>>,
    },
    DirectorySnapshotsPage {
        scope: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
        reply: Sender<SearchResult<Vec<DirectorySnapshot>>>,
    },
    DirectChildrenPage {
        parent: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
        reply: Sender<SearchResult<Vec<KnownDirectChild>>>,
    },
    MarkScopeInaccessible {
        scope: PathBuf,
        reply: Sender<SearchResult<bool>>,
    },
    DeleteScope {
        scope: PathBuf,
        reply: Sender<SearchResult<()>>,
    },
    ApplyPathConfigurationTransition {
        effective: VersionedSearchPathPreferences,
        policy: SearchPathPolicy,
        mounts: Vec<SearchRootMount>,
        invalidated_roots: Vec<PathBuf>,
        affected_scopes: Vec<PathBuf>,
        reply: Sender<SearchResult<()>>,
    },
    /// 统计当前可查询条目，并继续复用 writer 持有的数据库连接。
    Count(Sender<SearchResult<u64>>),
    /// Stop the writer after every previously queued command has been applied.
    /// Used when a stale daemon is retiring itself during a version handoff so
    /// the next daemon never races it for the writable SQLite connection.
    Shutdown(Option<Sender<()>>),
}

/// Serializes every write to the search database through one owned connection
/// on a dedicated thread. This is the same discipline tracker-miner-fs uses:
/// a single writer funnels all store operations, so two threads can never race
/// for the write lock. Read paths use `SearchDatabase::open_read_only` instead
/// and, under WAL, never block this writer.
pub struct IndexWriter {
    sender: SyncSender<WriteCommand>,
    interrupt: rusqlite::InterruptHandle,
    maintenance_cancelled: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl IndexWriter {
    /// Takes ownership of the one writable database connection and moves it onto
    /// a background thread that applies commands in the order they arrive.
    pub fn spawn(database: SearchDatabase) -> Self {
        // 零容量通道让 crawler 与唯一 writer 直接交接，正文不能在内存队列中累积。
        let (sender, receiver) = mpsc::sync_channel(0);
        let interrupt = database.interrupt_handle();
        let maintenance_cancelled = Arc::new(AtomicBool::new(false));
        let writer_maintenance_cancelled = Arc::clone(&maintenance_cancelled);
        let join = thread::Builder::new()
            .name("file-search-writer".to_owned())
            .spawn(move || writer_loop(database, receiver, writer_maintenance_cancelled))
            .expect("spawn search writer thread");
        Self {
            sender,
            interrupt,
            maintenance_cancelled,
            join: Mutex::new(Some(join)),
        }
    }

    /// Blocks until the dedicated writer has applied the file and returns that write's outcome.
    pub fn upsert(&self, file: IndexedFile) -> SearchResult<()> {
        validate_indexed_file_payload(&file)?;
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::UpsertObservable {
                file: Box::new(file),
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn classify_observed(
        &self,
        files: Vec<ObservedFile>,
    ) -> SearchResult<Vec<FileClassification>> {
        validate_classification_batch(&files)?;
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::ClassifyObserved { files, reply })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn apply_file_batch(&self, mutations: Vec<ScanFileMutation>) -> SearchResult<()> {
        validate_scan_file_batch(&mutations)?;
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::ApplyFileBatch { mutations, reply })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn upsert_directory_snapshot(
        &self,
        snapshot: DirectorySnapshot,
    ) -> SearchResult<()> {
        let path_bytes = snapshot.path.as_os_str().as_encoded_bytes().len();
        if path_bytes > MAX_WRITER_SCOPE_PATH_BYTES {
            return Err(SearchError::PayloadTooLarge {
                boundary: "directory snapshot path",
                actual_bytes: path_bytes,
                max_bytes: MAX_WRITER_SCOPE_PATH_BYTES,
            });
        }
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::UpsertDirectorySnapshot {
                snapshot: Box::new(snapshot),
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn directory_snapshot(
        &self,
        path: PathBuf,
    ) -> SearchResult<Option<DirectorySnapshot>> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::DirectorySnapshot { path, reply })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn known_files_page(
        &self,
        scope: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
    ) -> SearchResult<Vec<KnownFileEntry>> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::KnownFilesPage {
                scope,
                after_path,
                limit,
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn directory_snapshots_page(
        &self,
        scope: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
    ) -> SearchResult<Vec<DirectorySnapshot>> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::DirectorySnapshotsPage {
                scope,
                after_path,
                limit,
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn direct_children_page(
        &self,
        parent: PathBuf,
        after_path: Option<PathBuf>,
        limit: usize,
    ) -> SearchResult<Vec<KnownDirectChild>> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::DirectChildrenPage {
                parent,
                after_path,
                limit,
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn mark_scope_inaccessible(&self, scope: PathBuf) -> SearchResult<bool> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::MarkScopeInaccessible { scope, reply })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn delete_scope(&self, scope: PathBuf) -> SearchResult<()> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::DeleteScope { scope, reply })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn apply_path_configuration_transition(
        &self,
        effective: VersionedSearchPathPreferences,
        policy: SearchPathPolicy,
        mounts: Vec<SearchRootMount>,
        invalidated_roots: Vec<PathBuf>,
        affected_scopes: Vec<PathBuf>,
    ) -> SearchResult<()> {
        let (reply, outcome) = mpsc::channel();
        self.sender
            .send(WriteCommand::ApplyPathConfigurationTransition {
                effective,
                policy,
                mounts,
                invalidated_roots,
                affected_scopes,
                reply,
            })
            .map_err(|_| writer_gone())?;
        outcome.recv().map_err(|_| writer_gone())?
    }

    pub(crate) fn cancel_index_maintenance(&self) {
        self.maintenance_cancelled.store(true, Ordering::Release);
        self.interrupt.interrupt();
    }

    /// 返回当前可查询的索引条目数，并阻塞等待 writer 回复。
    pub fn count(&self) -> SearchResult<u64> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(WriteCommand::Count(tx))
            .map_err(|_| writer_gone())?;
        rx.recv().map_err(|_| writer_gone())?
    }

    /// 排空此前接收的写入、停止并等待 writer 线程。首次调用持有完整关闭所有权，
    /// 并发或后续调用会等待首次调用结束后直接成功。
    pub fn shutdown(&self) -> SearchResult<()> {
        let mut join = self
            .join
            .lock()
            .expect("search index writer join mutex poisoned");
        let Some(writer_thread) = join.take() else {
            return Ok(());
        };

        let (tx, rx) = mpsc::channel();
        let writer_outcome = self
            .sender
            .send(WriteCommand::Shutdown(Some(tx)))
            .map_err(|_| writer_gone())
            .and_then(|()| rx.recv().map_err(|_| writer_gone()));
        let join_outcome = writer_thread.join().map_err(|_| {
            SearchError::WorkerFailed("search index writer thread panicked".to_owned())
        });

        writer_outcome.and(join_outcome)
    }
}

impl Drop for IndexWriter {
    fn drop(&mut self) {
        let _ = self.sender.send(WriteCommand::Shutdown(None));
        let join = self
            .join
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(join) = join.take() {
            let _ = join.join();
        }
    }
}

fn writer_loop(
    database: SearchDatabase,
    receiver: Receiver<WriteCommand>,
    maintenance_cancelled: Arc<AtomicBool>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            WriteCommand::UpsertObservable { file, reply } => {
                let outcome = logged_write(database.upsert_file(&file), &maintenance_cancelled);
                let _ = reply.send(outcome);
            }
            WriteCommand::ClassifyObserved { files, reply } => {
                let _ = reply.send(database.classify_observed_files(&files));
            }
            WriteCommand::ApplyFileBatch { mutations, reply } => {
                let outcome = logged_write(
                    database.apply_file_batch(&mutations),
                    &maintenance_cancelled,
                );
                let _ = reply.send(outcome);
            }
            WriteCommand::UpsertDirectorySnapshot { snapshot, reply } => {
                let outcome = logged_write(
                    database.upsert_directory_snapshot(&snapshot),
                    &maintenance_cancelled,
                );
                let _ = reply.send(outcome);
            }
            WriteCommand::DirectorySnapshot { path, reply } => {
                let _ = reply.send(database.directory_snapshot(&path));
            }
            WriteCommand::KnownFilesPage {
                scope,
                after_path,
                limit,
                reply,
            } => {
                let _ = reply.send(database.known_files_page(&scope, after_path.as_deref(), limit));
            }
            WriteCommand::DirectorySnapshotsPage {
                scope,
                after_path,
                limit,
                reply,
            } => {
                let _ = reply.send(database.directory_snapshots_page(
                    &scope,
                    after_path.as_deref(),
                    limit,
                ));
            }
            WriteCommand::DirectChildrenPage {
                parent,
                after_path,
                limit,
                reply,
            } => {
                let _ = reply.send(database.direct_children_page(
                    &parent,
                    after_path.as_deref(),
                    limit,
                ));
            }
            WriteCommand::MarkScopeInaccessible { scope, reply } => {
                let outcome = logged_write(
                    database.mark_scope_inaccessible(&scope),
                    &maintenance_cancelled,
                );
                let _ = reply.send(outcome);
            }
            WriteCommand::DeleteScope { scope, reply } => {
                let outcome = logged_write(database.delete_scope(&scope), &maintenance_cancelled);
                let _ = reply.send(outcome);
            }
            WriteCommand::ApplyPathConfigurationTransition {
                effective,
                policy,
                mounts,
                invalidated_roots,
                affected_scopes,
                reply,
            } => {
                let outcome = logged_write(
                    database.apply_search_path_transition(
                        &effective,
                        &policy,
                        &mounts,
                        &invalidated_roots,
                        &affected_scopes,
                    ),
                    &maintenance_cancelled,
                );
                let _ = reply.send(outcome);
            }
            WriteCommand::Count(reply) => {
                let _ = reply.send(database.indexed_file_count());
            }
            WriteCommand::Shutdown(reply) => {
                if let Some(reply) = reply {
                    let _ = reply.send(());
                }
                break;
            }
        }
    }
}

/// Logs every failed write once, at the writer. Failures after maintenance was
/// cancelled come from the SQLite interrupt that cancellation itself issued, so
/// they still reach the cancelled caller but are not reported as faults.
fn logged_write<T>(
    write_outcome: SearchResult<T>,
    maintenance_cancelled: &AtomicBool,
) -> SearchResult<T> {
    if let Err(error) = &write_outcome {
        if !maintenance_cancelled.load(Ordering::Acquire) {
            let log_error = bounded_search_log_detail(&error.to_string());
            tracing::error!(
                target: "file_search::writer",
                event = "index_write_failed",
                error = %log_error,
                "search index write failed"
            );
        }
    }
    write_outcome
}

fn writer_gone() -> SearchError {
    SearchError::WorkerFailed("search index writer thread is no longer running".to_owned())
}

fn validate_indexed_file_payload(file: &IndexedFile) -> SearchResult<()> {
    let estimated_bytes = indexed_file_payload_bytes(file);
    if estimated_bytes > MAX_WRITER_FILE_PAYLOAD_BYTES {
        return Err(SearchError::PayloadTooLarge {
            boundary: "indexed file command",
            actual_bytes: estimated_bytes,
            max_bytes: MAX_WRITER_FILE_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

pub(crate) fn scan_file_mutation_bytes(mutation: &ScanFileMutation) -> usize {
    match mutation {
        ScanFileMutation::Observable(file) => indexed_file_payload_bytes(file),
        ScanFileMutation::Inaccessible { scope, file } => indexed_file_payload_bytes(file)
            .saturating_add(scope.as_os_str().as_encoded_bytes().len()),
    }
}

fn validate_scan_file_batch(mutations: &[ScanFileMutation]) -> SearchResult<()> {
    let estimated_bytes = mutations.iter().fold(0_usize, |total, mutation| {
        total.saturating_add(scan_file_mutation_bytes(mutation))
    });
    if mutations.len() > crate::database::MAX_CLASSIFICATION_BATCH_ENTRIES
        || estimated_bytes > MAX_WRITER_FILE_PAYLOAD_BYTES
    {
        return Err(SearchError::PayloadTooLarge {
            boundary: "scan file batch",
            actual_bytes: estimated_bytes,
            max_bytes: MAX_WRITER_FILE_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

fn indexed_file_payload_bytes(file: &IndexedFile) -> usize {
    INDEXED_FILE_FIXED_BYTES
        .saturating_add(file.path.as_os_str().as_encoded_bytes().len())
        .saturating_add(file.parent_path.as_os_str().as_encoded_bytes().len())
        .saturating_add(file.display_name.len())
        .saturating_add(file.mime_type.as_ref().map_or(0, String::len))
        .saturating_add(file.content.as_ref().map_or(0, String::len))
        .saturating_add(extraction_status_bytes(&file.extraction_status))
}

fn extraction_status_bytes(status: &ExtractionStatus) -> usize {
    match status {
        ExtractionStatus::ReadFailed { message } => message.len(),
        ExtractionStatus::ToolUnavailable { tool }
        | ExtractionStatus::TimedOut { tool }
        | ExtractionStatus::ResourceBudgetExceeded { tool } => tool.len(),
        ExtractionStatus::ToolFailed { tool, message } => tool.len().saturating_add(message.len()),
        ExtractionStatus::Indexed
        | ExtractionStatus::Disabled
        | ExtractionStatus::Unsupported
        | ExtractionStatus::TooLarge
        | ExtractionStatus::NonUtf8 => 0,
    }
}

#[cfg(test)]
#[path = "writer/tests.rs"]
mod tests;
