use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::fs;

use crate::entry_naming::{entry_name_candidates, EntryNameKind};
use crate::FileError;

const TRANSFER_CONFLICT_METADATA_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferConflictCheck {
    pub source: PathBuf,
    pub target: PathBuf,
}

impl TransferConflictCheck {
    pub fn new(source: PathBuf, target: PathBuf) -> Self {
        Self { source, target }
    }
}

#[derive(Debug, Clone)]
pub struct TransferConflictMetadata {
    pub is_directory: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct TransferConflictItem {
    pub source: PathBuf,
    pub target: PathBuf,
    pub source_metadata: TransferConflictMetadata,
    pub target_metadata: TransferConflictMetadata,
}

impl TransferConflictItem {
    pub fn can_merge(&self) -> bool {
        self.source_metadata.is_directory && self.target_metadata.is_directory
    }
}

/// 读不到某一项源或目标的信息时返回错误：那一项是否冲突无从判断，不能当作没有冲突放行。
/// 多项失败时按输入顺序返回第一个。
pub async fn check_transfer_conflicts(
    transfers: Vec<TransferConflictCheck>,
) -> Result<Vec<TransferConflictItem>, FileError> {
    let mut transfers = transfers.into_iter();
    let mut conflicts = Vec::new();

    loop {
        let handles: Vec<_> = transfers
            .by_ref()
            .take(TRANSFER_CONFLICT_METADATA_CONCURRENCY)
            .map(|transfer| {
                (
                    transfer.source.clone(),
                    tokio::spawn(transfer_conflict(transfer)),
                )
            })
            .collect();

        if handles.is_empty() {
            break;
        }

        // 同一批按 spawn 顺序等待，结果自然保持输入顺序。
        for (source, handle) in handles {
            let conflict = handle.await.map_err(|join_error| FileError::Metadata {
                path: source,
                source: io::Error::other(join_error),
            })??;
            if let Some(conflict) = conflict {
                conflicts.push(conflict);
            }
        }
    }

    Ok(conflicts)
}

pub async fn is_transfer_target_available(path: impl AsRef<Path>) -> Result<bool, FileError> {
    let path = path.as_ref().to_path_buf();
    transfer_target_metadata_if_exists(&path)
        .await
        .map(|metadata| metadata.is_none())
        .map_err(|source| FileError::Metadata { path, source })
}

pub(crate) async fn transfer_target_metadata_if_exists(
    path: &Path,
) -> io::Result<Option<std::fs::Metadata>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// "保留两者"的目标选择：从请求的名字开始沿共享候选序列找第一个未被占用的路径。
/// 这里只是选候选，调用方随后用不覆盖的原语落名，期间被抢占时由各自协议重选；
/// 读不到某个候选的信息时不能当作可用，否则会把读失败的位置当成空位。
pub(crate) async fn first_available_entry_path(
    requested: &Path,
    kind: EntryNameKind,
) -> Result<PathBuf, FileError> {
    let (parent, name) = entry_parent_and_name(requested)?;
    for candidate in entry_name_candidates(&name, kind) {
        let path = parent.join(candidate);
        match transfer_target_metadata_if_exists(&path).await {
            Ok(None) => return Ok(path),
            Ok(Some(_)) => {}
            Err(source) => return Err(FileError::Metadata { path, source }),
        }
    }
    Err(FileError::EntryNameCandidatesExhausted {
        directory: parent,
        base: name,
    })
}

/// 目标不存在就没有冲突。源不存在也没有冲突要问：传输执行时会报源不存在。
async fn transfer_conflict(
    transfer: TransferConflictCheck,
) -> Result<Option<TransferConflictItem>, FileError> {
    let metadata_if_exists = |path: PathBuf| async move {
        transfer_target_metadata_if_exists(&path)
            .await
            .map_err(|source| FileError::Metadata { path, source })
    };
    let Some(source_metadata) = metadata_if_exists(transfer.source.clone()).await? else {
        return Ok(None);
    };
    let Some(target_metadata) = metadata_if_exists(transfer.target.clone()).await? else {
        return Ok(None);
    };

    Ok(Some(TransferConflictItem {
        source: transfer.source,
        target: transfer.target,
        source_metadata: transfer_conflict_metadata(source_metadata),
        target_metadata: transfer_conflict_metadata(target_metadata),
    }))
}

fn transfer_conflict_metadata(metadata: std::fs::Metadata) -> TransferConflictMetadata {
    TransferConflictMetadata {
        is_directory: metadata.is_dir(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    }
}

fn entry_parent_and_name(path: &Path) -> Result<(PathBuf, OsString), FileError> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent.to_path_buf(), name.to_os_string())),
        _ => Err(FileError::InvalidInput {
            path: path.to_path_buf(),
            message: "target path has no file name".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_available_path_numbers_files_before_the_extension() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.txt"), b"taken").unwrap();
        std::fs::write(directory.path().join("a 2.txt"), b"taken").unwrap();

        let path = first_available_entry_path(&directory.path().join("a.txt"), EntryNameKind::File)
            .await
            .unwrap();

        assert_eq!(path, directory.path().join("a 3.txt"));
    }

    #[tokio::test]
    async fn first_available_path_numbers_directories_after_the_whole_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("photos.2024")).unwrap();

        let path = first_available_entry_path(
            &directory.path().join("photos.2024"),
            EntryNameKind::Directory,
        )
        .await
        .unwrap();

        assert_eq!(path, directory.path().join("photos.2024 2"));
    }

    #[tokio::test]
    async fn unreadable_candidates_are_errors_not_free_names() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, b"not a directory").unwrap();

        let outcome = first_available_entry_path(&file.join("child"), EntryNameKind::File).await;

        assert!(matches!(outcome, Err(FileError::Metadata { .. })));
    }

    #[tokio::test]
    async fn exhausted_candidates_are_errors_not_the_conflicting_name() {
        let directory = tempfile::tempdir().unwrap();
        for name in entry_name_candidates(std::ffi::OsStr::new("a.txt"), EntryNameKind::File) {
            std::fs::write(directory.path().join(name), b"taken").unwrap();
        }

        let outcome =
            first_available_entry_path(&directory.path().join("a.txt"), EntryNameKind::File).await;

        assert!(matches!(
            outcome,
            Err(FileError::EntryNameCandidatesExhausted { .. })
        ));
    }
}
