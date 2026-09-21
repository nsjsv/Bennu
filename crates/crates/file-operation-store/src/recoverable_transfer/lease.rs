use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::{StoreError, StoreResult};

#[derive(Debug)]
pub struct RecoverableTaskRunnerLease {
    pub(super) _lock_file: File,
}

#[derive(Debug)]
pub struct RecoverableRestoreCoordinatorLease {
    pub(super) _lock_file: File,
}

#[derive(Debug)]
pub struct ClaimedRecoverableTask {
    pub task_id: u64,
    pub runner_lease: RecoverableTaskRunnerLease,
}

pub(super) fn recoverable_restore_coordinator_lock_path(database_path: &Path) -> PathBuf {
    database_path.with_file_name(lock_file_name(database_path, ".restore-coordinator.lock"))
}

pub(super) fn recoverable_task_runner_lock_path(database_path: &Path, task_id: u64) -> PathBuf {
    database_path.with_file_name(lock_file_name(
        database_path,
        &format!(".task-{task_id}.runner.lock"),
    ))
}

fn lock_file_name(database_path: &Path, suffix: &str) -> OsString {
    let mut file_name = database_path
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| OsString::from("file-operation-store"));
    file_name.push(suffix);
    file_name
}

pub(super) fn try_acquire_lock(lock_path: PathBuf) -> StoreResult<Option<File>> {
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    match rustix::fs::flock(
        &lock_file,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    ) {
        Ok(()) => Ok(Some(lock_file)),
        Err(error) if error == rustix::io::Errno::WOULDBLOCK => Ok(None),
        Err(error) => Err(StoreError::Io(std::io::Error::from_raw_os_error(
            error.raw_os_error(),
        ))),
    }
}
