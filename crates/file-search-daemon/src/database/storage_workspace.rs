use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::error::{SearchError, SearchResult};

const WORKSPACE_MARKER_NAME: &str = "owner";
pub(super) const WORKSPACE_PENDING_MARKER_NAME: &str = "owner.pending";
const REPLACEMENT_DATABASE_NAME: &str = "replacement.sqlite";
const PREVIOUS_DATABASE_NAME: &str = "previous.sqlite";
const WORKSPACE_MARKER_PREFIX: &[u8] = b"file-manager-search-schema9\n";

pub(super) struct SchemaMigrationWorkspace {
    database_path: PathBuf,
    pub(super) directory_path: PathBuf,
    pub(super) replacement_path: PathBuf,
    previous_path: PathBuf,
    marker_path: PathBuf,
    marker_file: File,
}

impl SchemaMigrationWorkspace {
    pub(super) fn create(database_path: &Path) -> SearchResult<Self> {
        let directory_path = workspace_path(database_path)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory_path)
            .map_err(|source| SearchError::Io {
                path: directory_path.clone(),
                source,
            })?;
        let marker_path = directory_path.join(WORKSPACE_MARKER_NAME);
        let pending_marker_path = directory_path.join(WORKSPACE_PENDING_MARKER_NAME);
        let setup = (|| {
            let mut marker_file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&pending_marker_path)?;
            lock_marker(&marker_file, database_path)?;
            marker_file.write_all(&workspace_marker(database_path)?)?;
            marker_file.sync_all()?;
            fs::rename(&pending_marker_path, &marker_path)?;
            sync_directory(&directory_path)?;
            sync_directory(directory_path.parent().unwrap_or_else(|| Path::new(".")))?;
            Ok(Self {
                database_path: database_path.to_path_buf(),
                replacement_path: directory_path.join(REPLACEMENT_DATABASE_NAME),
                previous_path: directory_path.join(PREVIOUS_DATABASE_NAME),
                directory_path,
                marker_path,
                marker_file,
            })
        })();
        match setup {
            Ok(workspace) => Ok(workspace),
            Err(error) => Err(cleanup_uninitialized_workspace(database_path, error)),
        }
    }

    pub(super) fn remove_interrupted(database_path: &Path) -> SearchResult<()> {
        let directory_path = workspace_path(database_path)?;
        let metadata = match fs::symlink_metadata(&directory_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(SearchError::Io {
                    path: directory_path,
                    source,
                })
            }
        };
        if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(storage_migration_error(
                database_path,
                format!(
                    "untrusted migration workspace at {}",
                    directory_path.display()
                ),
            ));
        }
        let marker_path = directory_path.join(WORKSPACE_MARKER_NAME);
        let marker_file = match open_and_verify_marker(database_path, &marker_path) {
            Ok(marker_file) => marker_file,
            Err(SearchError::ProtocolIo(error)) if error.kind() == io::ErrorKind::NotFound => {
                return remove_uninitialized_workspace(database_path);
            }
            Err(error) => return Err(error),
        };
        lock_marker(&marker_file, database_path)?;
        let workspace = Self {
            database_path: database_path.to_path_buf(),
            replacement_path: directory_path.join(REPLACEMENT_DATABASE_NAME),
            previous_path: directory_path.join(PREVIOUS_DATABASE_NAME),
            directory_path,
            marker_path,
            marker_file,
        };
        workspace.finish_committed()
    }

    pub(super) fn preserve_previous_database(&self, database_path: &Path) -> SearchResult<()> {
        let metadata = fs::symlink_metadata(database_path).map_err(|source| SearchError::Io {
            path: database_path.to_path_buf(),
            source,
        })?;
        if !metadata.file_type().is_file() {
            return Err(storage_migration_error(
                database_path,
                "schema 8 database is not a regular file".to_owned(),
            ));
        }
        fs::hard_link(database_path, &self.previous_path).map_err(|source| SearchError::Io {
            path: self.previous_path.clone(),
            source,
        })?;
        sync_regular_file(&self.previous_path)?;
        sync_directory(&self.directory_path)?;
        sync_directory(self.parent_path())?;
        Ok(())
    }

    pub(super) fn rollback_committed(
        self,
        error: SearchError,
        database_path: &Path,
        replace: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
    ) -> SearchError {
        if let Err(rollback_error) = remove_checkpointed_sidecars(database_path).and_then(|()| {
            atomically_replace_database(&self.previous_path, database_path, |from, to| {
                replace(from, to)
            })
        }) {
            return storage_migration_error(
                database_path,
                format!(
                    "{error}; restoring schema 8 failed: {rollback_error}; preserved backup at {}",
                    self.previous_path.display()
                ),
            );
        }
        let rollback_outcome = sync_directory(self.parent_path());
        let restored_error = match rollback_outcome {
            Ok(()) => storage_migration_error(
                database_path,
                format!("schema 9 commit failed and schema 8 was restored: {error}"),
            ),
            Err(rollback_error) => storage_migration_error(
                database_path,
                format!(
                    "{error}; schema 8 was restored but directory sync failed: {rollback_error}"
                ),
            ),
        };
        self.cleanup_after(restored_error)
    }

    pub(super) fn cleanup_after(self, error: SearchError) -> SearchError {
        let database_path = self.database_path.clone();
        match self.finish_committed() {
            Ok(()) => error,
            Err(cleanup_error) => storage_migration_error(
                &database_path,
                format!("{error}; migration workspace cleanup also failed: {cleanup_error}"),
            ),
        }
    }

    pub(super) fn finish_committed(self) -> SearchResult<()> {
        let parent_path = self.parent_path().to_path_buf();
        for path in workspace_database_paths(&self.directory_path) {
            remove_regular_file_if_present(&path)?;
        }
        remove_regular_file_if_present(&self.directory_path.join(WORKSPACE_PENDING_MARKER_NAME))?;
        sync_directory(&self.directory_path)?;
        drop(self.marker_file);
        fs::remove_file(&self.marker_path)?;
        sync_directory(&self.directory_path)?;
        fs::remove_dir(&self.directory_path)?;
        sync_directory(&parent_path)?;
        Ok(())
    }

    pub(super) fn parent_path(&self) -> &Path {
        self.directory_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
    }
}

fn cleanup_uninitialized_workspace(database_path: &Path, error: SearchError) -> SearchError {
    match SchemaMigrationWorkspace::remove_interrupted(database_path) {
        Ok(()) => error,
        Err(cleanup_error) => storage_migration_error(
            database_path,
            format!("{error}; incomplete workspace cleanup also failed: {cleanup_error}"),
        ),
    }
}

fn remove_uninitialized_workspace(database_path: &Path) -> SearchResult<()> {
    let directory_path = workspace_path(database_path)?;
    remove_regular_file_if_present(&directory_path.join(WORKSPACE_PENDING_MARKER_NAME))?;
    if fs::read_dir(&directory_path)?.next().is_some() {
        return Err(storage_migration_error(
            database_path,
            format!(
                "unowned migration workspace is not empty at {}",
                directory_path.display()
            ),
        ));
    }
    fs::remove_dir(&directory_path)?;
    sync_directory(directory_path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(())
}

pub(super) fn workspace_path(database_path: &Path) -> SearchResult<PathBuf> {
    let file_name = database_path.file_name().ok_or_else(|| {
        storage_migration_error(database_path, "database path has no file name".to_owned())
    })?;
    let mut workspace_name = OsString::from(".");
    workspace_name.push(file_name);
    workspace_name.push(".schema9-migration");
    Ok(database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(workspace_name))
}

fn workspace_marker(database_path: &Path) -> SearchResult<Vec<u8>> {
    let file_name = database_path.file_name().ok_or_else(|| {
        storage_migration_error(database_path, "database path has no file name".to_owned())
    })?;
    let mut marker = WORKSPACE_MARKER_PREFIX.to_vec();
    marker.extend_from_slice(file_name.as_bytes());
    marker.push(b'\n');
    Ok(marker)
}

fn open_and_verify_marker(database_path: &Path, marker_path: &Path) -> SearchResult<File> {
    let expected = workspace_marker(database_path)?;
    let mut marker_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(marker_path)?;
    let metadata = marker_file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() != expected.len() as u64 {
        return Err(storage_migration_error(
            database_path,
            format!(
                "invalid migration owner marker at {}",
                marker_path.display()
            ),
        ));
    }
    let mut actual = Vec::with_capacity(expected.len());
    marker_file.read_to_end(&mut actual)?;
    if actual != expected {
        return Err(storage_migration_error(
            database_path,
            format!(
                "migration owner marker mismatch at {}",
                marker_path.display()
            ),
        ));
    }
    Ok(marker_file)
}

fn lock_marker(marker_file: &File, database_path: &Path) -> SearchResult<()> {
    let return_code =
        unsafe { libc::flock(marker_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if return_code == 0 {
        return Ok(());
    }
    let source = io::Error::last_os_error();
    Err(storage_migration_error(
        database_path,
        format!("could not lock migration workspace: {source}"),
    ))
}

fn workspace_database_paths(directory_path: &Path) -> [PathBuf; 5] {
    let replacement = directory_path.join(REPLACEMENT_DATABASE_NAME);
    [
        replacement.clone(),
        sidecar_path(&replacement, "-wal"),
        sidecar_path(&replacement, "-shm"),
        sidecar_path(&replacement, "-journal"),
        directory_path.join(PREVIOUS_DATABASE_NAME),
    ]
}

pub(super) fn atomically_replace_database(
    replacement_path: &Path,
    database_path: &Path,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> SearchResult<()> {
    replace(replacement_path, database_path).map_err(|source| SearchError::Io {
        path: database_path.to_path_buf(),
        source,
    })
}

pub(super) fn remove_checkpointed_sidecars(database_path: &Path) -> SearchResult<()> {
    for path in [
        sidecar_path(database_path, "-wal"),
        sidecar_path(database_path, "-shm"),
    ] {
        remove_regular_file_if_present(&path)?;
    }
    Ok(())
}

fn remove_regular_file_if_present(path: &Path) -> SearchResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(storage_migration_error(
            path,
            "migration artifact is not a regular file".to_owned(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SearchError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = database_path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

pub(super) fn sync_regular_file(path: &Path) -> SearchResult<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| SearchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !file.metadata()?.file_type().is_file() {
        return Err(storage_migration_error(
            path,
            "migration database artifact is not a regular file".to_owned(),
        ));
    }
    file.sync_all()?;
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> SearchResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(super) fn storage_migration_error(database_path: &Path, message: String) -> SearchError {
    SearchError::DatabaseStorageMigrationFailed {
        database_path: database_path.to_path_buf(),
        message,
    }
}
