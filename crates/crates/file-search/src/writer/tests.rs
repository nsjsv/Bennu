use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tempfile::tempdir;
use tracing_subscriber::prelude::*;

use crate::database::{EntryStageProgress, IndexedEntryStageState, SearchDatabase};
use crate::extractor::ExtractionStatus;
use crate::model::{SearchFileKind, SearchQuery};

use super::*;

#[derive(Clone, Default)]
struct ErrorEventCounter(Arc<AtomicUsize>);

impl<S> tracing_subscriber::Layer<S> for ErrorEventCounter
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() == tracing::Level::ERROR {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[test]
fn repeated_write_failures_record_only_the_owned_first_error() {
    let error_events = ErrorEventCounter::default();
    let subscriber = tracing_subscriber::registry().with(error_events.clone());
    let mut pending_error = None;

    let maintenance_cancelled = AtomicBool::new(false);
    tracing::subscriber::with_default(subscriber, || {
        record_first_write_error(
            &mut pending_error,
            Err(SearchError::WorkerFailed("first".to_owned())),
            &maintenance_cancelled,
        );
        record_first_write_error(
            &mut pending_error,
            Err(SearchError::WorkerFailed("second".to_owned())),
            &maintenance_cancelled,
        );
    });

    assert_eq!(error_events.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        pending_error.unwrap().to_string(),
        "search worker failed: first"
    );
}

fn sample_file(path: &Path, inode: u64, mtime_ns: i64) -> IndexedFile {
    IndexedFile {
        path: path.to_path_buf(),
        parent_path: path.parent().unwrap().to_path_buf(),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        kind: SearchFileKind::File,
        size: 6,
        modified_ms: Some(1),
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: Some("needle".to_owned()),
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(inode),
        mtime_ns: Some(mtime_ns),
        ctime_ns: Some(mtime_ns),
    }
}

#[test]
fn writes_are_visible_to_a_concurrent_reader_without_locking() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let file_path = dir.path().join("note.txt");

    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    let reader = SearchDatabase::open_read_only(&db_path).unwrap();

    writer.upsert(sample_file(&file_path, 10, 20)).unwrap();
    writer.flush().unwrap();

    // The read-only connection sees the committed write while the writer
    // thread is still alive and holding the write connection.
    let batch = reader.search(&SearchQuery::global(1, "needle")).unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].display_name, "note.txt");
}

#[test]
fn signatures_round_trip_through_the_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let file_path = dir.path().join("note.txt");

    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    writer.upsert(sample_file(&file_path, 42, 99)).unwrap();
    writer.flush().unwrap();

    let stored = writer
        .classify_observed(vec![ObservedFile {
            path: file_path.clone(),
            signature: crate::database::FileSignature {
                device: Some(1),
                inode: Some(42),
                mtime_ns: Some(99),
                ctime_ns: Some(99),
                size: 6,
            },
        }])
        .unwrap()
        .pop()
        .unwrap()
        .known_entry
        .expect("signature recorded");
    assert_eq!(
        stored.stage_state,
        IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        }
    );
    let stored_signature = stored.signature;
    assert_eq!(stored_signature.inode, Some(42));
    assert_eq!(stored_signature.mtime_ns, Some(99));
    assert_eq!(writer.count().unwrap(), 1);
}

#[test]
fn skipped_stage_state_round_trips_through_the_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let file_path = dir.path().join("too-large.txt");

    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    let mut file = sample_file(&file_path, 42, 99);
    file.display_name = "too-large.txt".to_owned();
    file.content = None;
    file.extraction_status = ExtractionStatus::TooLarge;
    file.stage_state = IndexedEntryStageState {
        metadata: EntryStageProgress::Complete,
        content: EntryStageProgress::Skipped,
    };
    writer.upsert(file).unwrap();
    writer.flush().unwrap();

    let stored = writer
        .classify_observed(vec![ObservedFile {
            path: file_path,
            signature: crate::database::FileSignature {
                device: Some(1),
                inode: Some(42),
                mtime_ns: Some(99),
                ctime_ns: Some(99),
                size: 6,
            },
        }])
        .unwrap()
        .pop()
        .unwrap()
        .known_entry
        .expect("signature recorded");
    assert_eq!(
        stored.stage_state,
        IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Skipped,
        }
    );
}

#[test]
fn local_delete_only_removes_its_target() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let kept = dir.path().join("kept.txt");
    let gone = dir.path().join("gone.txt");

    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    writer.upsert(sample_file(&kept, 1, 1)).unwrap();
    writer.upsert(sample_file(&gone, 2, 2)).unwrap();
    writer.flush().unwrap();

    writer.delete_scope(gone).unwrap();
    writer.flush().unwrap();

    assert_eq!(writer.count().unwrap(), 1);
    let reader = SearchDatabase::open_read_only(&db_path).unwrap();
    let batch = reader.search(&SearchQuery::global(1, "needle")).unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].path, kept);
}

#[test]
fn shutdown_drains_queued_writes_before_stopping_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let file_path = dir.path().join("note.txt");

    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    writer.upsert(sample_file(&file_path, 7, 11)).unwrap();
    writer.shutdown().unwrap();

    let reader = SearchDatabase::open_read_only(&db_path).unwrap();
    let batch = reader.search(&SearchQuery::global(1, "needle")).unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].display_name, "note.txt");
}

#[test]
fn cancelled_index_maintenance_discards_write_failure_during_shutdown() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch("DROP TABLE file_search_fts")
        .unwrap();
    let writer = IndexWriter::spawn(database);

    writer.cancel_index_maintenance();
    writer
        .upsert(sample_file(&dir.path().join("note.txt"), 1, 1))
        .unwrap();

    writer.shutdown().unwrap();
}

#[test]
fn shutdown_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());

    writer.shutdown().unwrap();
    writer.shutdown().unwrap();
}

#[test]
fn concurrent_shutdown_calls_share_one_join() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let writer = Arc::new(IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap()));

    let first = {
        let writer = Arc::clone(&writer);
        std::thread::spawn(move || writer.shutdown())
    };
    let second = {
        let writer = Arc::clone(&writer);
        std::thread::spawn(move || writer.shutdown())
    };

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
}

#[test]
fn oversized_file_command_is_rejected_before_writer_handoff() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let file_path = directory.path().join("oversized.txt");
    let writer = IndexWriter::spawn(SearchDatabase::open(&database_path).unwrap());
    let mut file = sample_file(&file_path, 1, 1);
    file.content = Some("x".repeat(MAX_WRITER_FILE_PAYLOAD_BYTES));

    assert!(matches!(
        writer.upsert(file),
        Err(SearchError::PayloadTooLarge {
            boundary: "indexed file command",
            ..
        })
    ));
    assert_eq!(writer.count().unwrap(), 0);
}

#[test]
fn file_batch_commits_many_metadata_rows_with_one_handoff() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&database_path).unwrap());
    let mutations = (0..crate::database::MAX_CLASSIFICATION_BATCH_ENTRIES)
        .map(|index| {
            let path = directory.path().join(format!("file-{index}.bin"));
            let mut file = sample_file(&path, index as u64, index as i64);
            file.content = None;
            file.extraction_status = ExtractionStatus::Unsupported;
            file.stage_state.content = EntryStageProgress::Skipped;
            ScanFileMutation::Observable(file)
        })
        .collect();

    writer.apply_file_batch(mutations).unwrap();
    writer.flush().unwrap();

    assert_eq!(writer.count().unwrap(), 128);
}

#[test]
fn file_batch_rejects_combined_payload_above_the_fixed_budget() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&database_path).unwrap());
    let mutations = (0..2)
        .map(|index| {
            let path = directory.path().join(format!("large-{index}.txt"));
            let mut file = sample_file(&path, index, index as i64);
            file.content = Some("x".repeat(1_300_000));
            ScanFileMutation::Observable(file)
        })
        .collect();

    assert!(matches!(
        writer.apply_file_batch(mutations),
        Err(SearchError::PayloadTooLarge {
            boundary: "scan file batch",
            ..
        })
    ));
    assert_eq!(writer.count().unwrap(), 0);
}
