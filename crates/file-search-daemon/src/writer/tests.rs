use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tempfile::tempdir;
use tracing_subscriber::prelude::*;

use crate::database::{EntryStageProgress, IndexedEntryStageState, SearchDatabase};
use crate::extractor::ExtractionStatus;
use crate::model::{SearchFileKind, SearchQuery};
use crate::query_state::QueryState;

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
fn every_failed_write_is_logged_unless_maintenance_was_cancelled() {
    let error_events = ErrorEventCounter::default();
    let subscriber = tracing_subscriber::registry().with(error_events.clone());
    let maintenance_cancelled = AtomicBool::new(false);

    tracing::subscriber::with_default(subscriber, || {
        for message in ["first", "second"] {
            assert!(logged_write::<()>(
                Err(SearchError::WorkerFailed(message.to_owned())),
                &maintenance_cancelled,
            )
            .is_err());
        }
        maintenance_cancelled.store(true, Ordering::Release);
        assert!(logged_write::<()>(
            Err(SearchError::WorkerFailed("interrupted".to_owned())),
            &maintenance_cancelled,
        )
        .is_err());
    });

    assert_eq!(error_events.0.load(Ordering::Relaxed), 2);
}

#[test]
fn every_failed_write_reaches_its_own_caller() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());
    let schema_connection = rusqlite::Connection::open(&db_path).unwrap();
    schema_connection
        .execute_batch("ALTER TABLE files RENAME TO files_unavailable")
        .unwrap();

    assert!(writer
        .upsert(sample_file(&dir.path().join("first.txt"), 1, 1))
        .is_err());
    assert!(writer.delete_scope(dir.path().join("second.txt")).is_err());

    schema_connection
        .execute_batch("ALTER TABLE files_unavailable RENAME TO files")
        .unwrap();
    // 之前的写入失败已经交给了各自的调用方，后面的读写只拿自己的结果。
    assert_eq!(writer.count().unwrap(), 0);
    assert!(writer
        .directory_snapshot(dir.path().to_path_buf())
        .unwrap()
        .is_none());
    writer
        .upsert(sample_file(&dir.path().join("third.txt"), 3, 3))
        .unwrap();
    assert_eq!(writer.count().unwrap(), 1);
    writer.shutdown().unwrap();
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
    writer.commit_pending().unwrap();

    // 写库线程还活着、仍持有写连接时，只读连接就能看到已提交的写入。
    let batch = reader
        .search(&SearchQuery::global(1, "needle"), &QueryState::default())
        .unwrap();
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

    writer.delete_scope(gone).unwrap();
    writer.commit_pending().unwrap();

    assert_eq!(writer.count().unwrap(), 1);
    let reader = SearchDatabase::open_read_only(&db_path).unwrap();
    let batch = reader
        .search(&SearchQuery::global(1, "needle"), &QueryState::default())
        .unwrap();
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
    let batch = reader
        .search(&SearchQuery::global(1, "needle"), &QueryState::default())
        .unwrap();
    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].display_name, "note.txt");
}

#[test]
fn write_failure_after_cancellation_goes_to_its_caller_and_not_to_shutdown() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch("DROP TABLE file_search_fts")
        .unwrap();
    let writer = IndexWriter::spawn(database);

    writer.cancel_index_maintenance();
    assert!(writer
        .upsert(sample_file(&dir.path().join("note.txt"), 1, 1))
        .is_err());

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

fn committed_names(database_path: &Path) -> Vec<String> {
    let reader = SearchDatabase::open_read_only(database_path).unwrap();
    let mut names: Vec<String> = reader
        .search(&SearchQuery::global(1, "needle"), &QueryState::default())
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.display_name)
        .collect();
    names.sort();
    names
}

fn add_files_trigger(database_path: &Path, display_name: &str, raise: &str) {
    rusqlite::Connection::open(database_path)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER reject_{raise} BEFORE INSERT ON files
             WHEN NEW.display_name = '{display_name}'
             BEGIN SELECT RAISE({raise}, 'rejected by test'); END;"
        ))
        .unwrap();
}

#[test]
fn an_open_batch_is_read_by_the_writer_but_not_by_other_connections() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());

    writer
        .upsert(sample_file(&dir.path().join("note.txt"), 1, 1))
        .unwrap();

    // 维护工作的读命令走写库连接，必须看到自己还没提交的写入。
    assert_eq!(writer.count().unwrap(), 1);
    assert!(committed_names(&db_path).is_empty());
    writer.commit_pending().unwrap();
    assert_eq!(committed_names(&db_path), ["note.txt"]);
}

#[test]
fn a_failed_write_only_undoes_itself_within_its_batch() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    add_files_trigger(&db_path, "rejected.txt", "ABORT");
    let writer = IndexWriter::spawn(database);

    writer
        .upsert(sample_file(&dir.path().join("before.txt"), 1, 1))
        .unwrap();
    assert!(writer
        .upsert(sample_file(&dir.path().join("rejected.txt"), 2, 2))
        .is_err());
    writer
        .upsert(sample_file(&dir.path().join("after.txt"), 3, 3))
        .unwrap();
    writer.commit_pending().unwrap();

    assert_eq!(committed_names(&db_path), ["after.txt", "before.txt"]);
}

#[test]
fn a_batch_rolled_back_by_sqlite_is_forgotten_by_the_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    // 与取消维护时被中断的写语句相同：SQLite 自己回滚了整个批。
    add_files_trigger(&db_path, "rolled-back.txt", "ROLLBACK");
    let writer = IndexWriter::spawn(database);

    writer
        .upsert(sample_file(&dir.path().join("lost.txt"), 1, 1))
        .unwrap();
    assert!(writer
        .upsert(sample_file(&dir.path().join("rolled-back.txt"), 2, 2))
        .is_err());
    writer
        .upsert(sample_file(&dir.path().join("next.txt"), 3, 3))
        .unwrap();
    writer.commit_pending().unwrap();

    assert_eq!(committed_names(&db_path), ["next.txt"]);
}

#[test]
fn an_open_batch_commits_by_itself_once_it_is_old_enough() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let writer = IndexWriter::spawn(SearchDatabase::open(&db_path).unwrap());

    writer
        .upsert(sample_file(&dir.path().join("note.txt"), 1, 1))
        .unwrap();

    let deadline = Instant::now() + WRITE_BATCH_MAX_AGE * 5;
    while committed_names(&db_path).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the batch was never committed by age"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_batch_commits_as_soon_as_it_holds_enough_changes() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    let maintenance_cancelled = AtomicBool::new(false);
    let mut batch = WriteBatch::default();
    let file = |index: u64| {
        sample_file(
            &dir.path().join(format!("file-{index}.txt")),
            index,
            index as i64,
        )
    };

    batch
        .write(&database, &maintenance_cancelled, |database| {
            database.upsert_file(&file(0))
        })
        .unwrap();
    assert!(database.write_batch_is_open());

    let mutations: Vec<_> = (1..=5_000)
        .map(|index| ScanFileMutation::Observable(file(index)))
        .collect();
    let changes_before = database.total_changes();
    batch
        .write(&database, &maintenance_cancelled, |database| {
            database.apply_file_batch(&mutations)
        })
        .unwrap();

    assert!(database.total_changes() - changes_before >= WRITE_BATCH_MAX_CHANGES);
    assert!(!database.write_batch_is_open());
}

#[test]
fn a_timed_commit_failure_reaches_the_next_write_exactly_once() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("search.sqlite");
    let database = SearchDatabase::open(&db_path).unwrap();
    let maintenance_cancelled = AtomicBool::new(false);
    let mut batch = WriteBatch {
        open: None,
        undelivered_commit_failure: Some(SearchError::WorkerFailed("timed commit".to_owned())),
    };

    let outcome = batch.write(&database, &maintenance_cancelled, |database| {
        database.upsert_file(&sample_file(&dir.path().join("note.txt"), 1, 1))
    });

    assert!(
        matches!(outcome, Err(SearchError::WorkerFailed(message)) if message == "timed commit")
    );
    batch.commit(&database).unwrap();
    assert_eq!(committed_names(&db_path), ["note.txt"]);
}
