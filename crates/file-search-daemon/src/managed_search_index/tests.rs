use std::fs;
use std::io::{Seek, SeekFrom, Write};

use rusqlite::ffi::{
    Error as SqliteError, ErrorCode, SQLITE_BUSY, SQLITE_CONSTRAINT, SQLITE_CORRUPT, SQLITE_FULL,
    SQLITE_IOERR, SQLITE_LOCKED, SQLITE_NOTADB, SQLITE_READONLY,
};
use rusqlite::{Connection, Error as RusqliteError};
use tempfile::tempdir;

use crate::database::{SearchDatabase, SCHEMA_VERSION};
use crate::error::SearchError;

use super::{index_corruption, sidecar_path, IndexCorruption, ManagedSearchIndex};

#[test]
fn sqlite_corruption_classifier_accepts_only_corrupt_and_notadb() {
    assert_eq!(
        index_corruption(&database_error(SQLITE_CORRUPT)),
        Some(IndexCorruption::DatabaseCorrupt)
    );
    assert_eq!(
        index_corruption(&database_error(SQLITE_NOTADB)),
        Some(IndexCorruption::NotADatabase)
    );

    for code in [
        SQLITE_BUSY,
        SQLITE_LOCKED,
        SQLITE_READONLY,
        SQLITE_IOERR,
        SQLITE_FULL,
        SQLITE_CONSTRAINT,
    ] {
        assert_eq!(index_corruption(&database_error(code)), None);
    }
    assert_eq!(
        index_corruption(&SearchError::UnsupportedDatabaseSchema {
            found: SCHEMA_VERSION + 1,
            supported: SCHEMA_VERSION,
        }),
        None
    );
}

#[test]
fn notadb_main_is_quarantined_before_rebuild() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    fs::write(&database_path, b"not a sqlite database").unwrap();

    let opened = ManagedSearchIndex::new(database_path.clone())
        .open()
        .unwrap();
    let notice = opened.recovery_notice.unwrap();
    assert_eq!(notice.corruption, IndexCorruption::NotADatabase);
    assert_eq!(
        fs::read(notice.quarantine_directory.join("search.sqlite")).unwrap(),
        b"not a sqlite database"
    );
    drop(opened.database);

    let replacement = Connection::open(&database_path).unwrap();
    assert_eq!(
        replacement
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn quarantine_moves_main_wal_and_shm_as_one_managed_set() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let wal_path = sidecar_path(&database_path, "-wal");
    let shm_path = sidecar_path(&database_path, "-shm");
    fs::write(&database_path, b"main evidence").unwrap();
    fs::write(&wal_path, b"wal evidence").unwrap();
    fs::write(&shm_path, b"shm evidence").unwrap();

    let quarantine_directory = ManagedSearchIndex::new(database_path.clone())
        .quarantine_managed_files()
        .unwrap();

    assert!(!database_path.exists());
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());
    assert_eq!(
        fs::read(quarantine_directory.join("search.sqlite")).unwrap(),
        b"main evidence"
    );
    assert_eq!(
        fs::read(quarantine_directory.join("search.sqlite-wal")).unwrap(),
        b"wal evidence"
    );
    assert_eq!(
        fs::read(quarantine_directory.join("search.sqlite-shm")).unwrap(),
        b"shm evidence"
    );
}

#[test]
fn sidecar_move_failure_rolls_back_the_already_moved_main_database() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let wal_path = sidecar_path(&database_path, "-wal");
    let shm_path = sidecar_path(&database_path, "-shm");
    fs::write(&database_path, b"main evidence").unwrap();
    fs::write(&wal_path, b"wal evidence").unwrap();
    fs::write(&shm_path, b"shm evidence").unwrap();
    let quarantine_directory = directory.path().join("quarantine");
    fs::create_dir(&quarantine_directory).unwrap();
    let managed_index = ManagedSearchIndex::new(database_path.clone());
    let managed_paths = managed_index.validate_managed_files().unwrap();

    let error = managed_index
        .move_managed_paths(
            managed_paths,
            &quarantine_directory,
            |source, destination| {
                if source == wal_path {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected sidecar move failure",
                    ));
                }
                fs::rename(source, destination)
            },
        )
        .unwrap_err();

    assert!(matches!(
        error,
        SearchError::ManagedIndexQuarantineFailed {
            quarantine_directory: None,
            ..
        }
    ));
    assert_eq!(fs::read(&database_path).unwrap(), b"main evidence");
    assert_eq!(fs::read(&wal_path).unwrap(), b"wal evidence");
    assert_eq!(fs::read(&shm_path).unwrap(), b"shm evidence");
    assert!(!quarantine_directory.exists());
}

#[test]
fn corrupt_sqlite_page_is_quarantined_and_rebuilt() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    drop(SearchDatabase::open(&database_path).unwrap());
    let original_length = fs::metadata(&database_path).unwrap().len();
    let mut database_file = fs::OpenOptions::new()
        .write(true)
        .open(&database_path)
        .unwrap();
    database_file.seek(SeekFrom::Start(100)).unwrap();
    database_file.write_all(&[0xff]).unwrap();
    database_file.sync_all().unwrap();
    drop(database_file);

    let opened = ManagedSearchIndex::new(database_path.clone())
        .open()
        .unwrap();
    let notice = opened.recovery_notice.unwrap();
    assert_eq!(notice.corruption, IndexCorruption::DatabaseCorrupt);
    assert_eq!(
        fs::metadata(notice.quarantine_directory.join("search.sqlite"))
            .unwrap()
            .len(),
        original_length
    );
    drop(opened.database);
    assert!(fs::metadata(&database_path).unwrap().len() <= original_length);
    assert_eq!(
        Connection::open(&database_path)
            .unwrap()
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn future_schema_and_busy_database_are_not_quarantined() {
    let directory = tempdir().unwrap();
    let future_path = directory.path().join("future.sqlite");
    let connection = Connection::open(&future_path).unwrap();
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    drop(connection);
    let future_bytes = fs::read(&future_path).unwrap();

    let future_error = match ManagedSearchIndex::new(future_path.clone()).open() {
        Ok(_) => panic!("future schema opened successfully"),
        Err(error) => error,
    };
    assert!(matches!(
        future_error,
        SearchError::UnsupportedDatabaseSchema { .. }
    ));
    assert_eq!(fs::read(&future_path).unwrap(), future_bytes);
    assert!(quarantine_directories(directory.path()).is_empty());

    let busy_path = directory.path().join("busy.sqlite");
    drop(SearchDatabase::open(&busy_path).unwrap());
    let locking_connection = Connection::open(&busy_path).unwrap();
    locking_connection
        .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
        .unwrap();
    locking_connection
        .execute_batch("BEGIN IMMEDIATE;")
        .unwrap();
    let busy_error = match ManagedSearchIndex::new(busy_path.clone()).open() {
        Ok(_) => panic!("busy database opened successfully"),
        Err(error) => error,
    };
    assert!(
        matches!(
            busy_error,
            SearchError::Database(ref error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                )
        ),
        "unexpected busy migration error: {busy_error:?}"
    );
    assert!(busy_path.exists());
    assert!(quarantine_directories(directory.path()).is_empty());
    locking_connection.execute_batch("ROLLBACK;").unwrap();
}

#[cfg(unix)]
#[test]
fn managed_index_symlink_is_rejected_without_touching_target() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let external_path = directory.path().join("external.bin");
    let database_path = directory.path().join("search.sqlite");
    fs::write(&external_path, b"external evidence").unwrap();
    symlink(&external_path, &database_path).unwrap();

    let error = match ManagedSearchIndex::new(database_path.clone()).open() {
        Ok(_) => panic!("managed index symlink opened successfully"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        SearchError::InvalidManagedIndexMember { path } if path == database_path
    ));
    assert_eq!(fs::read(&external_path).unwrap(), b"external evidence");
    assert!(fs::symlink_metadata(&database_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(quarantine_directories(directory.path()).is_empty());
}

fn database_error(code: i32) -> SearchError {
    SearchError::Database(RusqliteError::SqliteFailure(SqliteError::new(code), None))
}

fn quarantine_directories(parent: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".search-index-quarantine-"))
        })
        .collect()
}
