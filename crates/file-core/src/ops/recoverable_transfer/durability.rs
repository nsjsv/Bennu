use std::collections::{BTreeSet, HashSet};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::RecoverableTransferError;

/// What has to reach the disk before the current batch of checkpoints may be
/// written: a checkpoint must never claim filesystem effects that a crash could
/// still undo. Steps only register where their effects landed; the runner
/// flushes once per batch, so N records advancing through the same step share
/// one flush instead of paying one each.
///
/// The flush is `syncfs` per filesystem rather than `fsync` per directory: it
/// covers directory entries and file data alike, and a batch touches hundreds
/// of distinct staging directories but usually one or two filesystems.
#[derive(Debug, Default)]
pub(crate) struct CommitBarrier {
    directories: BTreeSet<PathBuf>,
}

impl CommitBarrier {
    /// `path`'s own directory entry (created, renamed or removed) or its
    /// contents must be durable.
    pub(crate) fn require_parent_of(&mut self, path: &Path) {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(path);
        if !self.directories.contains(directory) {
            self.directories.insert(directory.to_path_buf());
        }
    }

    pub(crate) fn absorb(&mut self, other: CommitBarrier) {
        self.directories.extend(other.directories);
    }

    pub(crate) fn require_rename(&mut self, from: &Path, to: &Path) {
        self.require_parent_of(from);
        self.require_parent_of(to);
    }

    pub(crate) async fn flush(&mut self) -> Result<(), RecoverableTransferError> {
        if self.directories.is_empty() {
            return Ok(());
        }
        let directories = std::mem::take(&mut self.directories);
        tokio::task::spawn_blocking(move || {
            let mut flushed_devices = HashSet::new();
            for directory in directories {
                let (existing, device) = nearest_existing_directory(&directory)?;
                if flushed_devices.insert(device) {
                    sync_filesystem(&existing)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|join_error| {
            RecoverableTransferError::file_system(
                "join filesystem flush for",
                Path::new("/"),
                io::Error::other(join_error),
            )
        })?
    }
}

/// A registered directory can be gone by flush time (a later effect in the
/// same batch removed it); any ancestor still names the same filesystem.
fn nearest_existing_directory(
    directory: &Path,
) -> Result<(PathBuf, u64), RecoverableTransferError> {
    let mut candidate = directory;
    loop {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) => return Ok((candidate.to_path_buf(), metadata.dev())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(|| {
                    RecoverableTransferError::file_system("find filesystem of", directory, error)
                })?;
            }
            Err(source) => {
                return Err(RecoverableTransferError::file_system(
                    "find filesystem of",
                    directory,
                    source,
                ))
            }
        }
    }
}

fn sync_filesystem(directory: &Path) -> Result<(), RecoverableTransferError> {
    let file = std::fs::File::open(directory).map_err(|source| {
        RecoverableTransferError::file_system("open filesystem to flush", directory, source)
    })?;
    let result = unsafe { libc::syncfs(std::os::unix::io::AsRawFd::as_raw_fd(&file)) };
    if result != 0 {
        return Err(RecoverableTransferError::file_system(
            "flush filesystem of",
            directory,
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}
