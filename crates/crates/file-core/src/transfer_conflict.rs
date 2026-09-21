use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::fs;

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

pub async fn check_transfer_conflicts(
    transfers: Vec<TransferConflictCheck>,
) -> Vec<TransferConflictItem> {
    let mut indexed_transfers = transfers.into_iter().enumerate();
    let mut indexed_conflicts = Vec::new();

    loop {
        let mut handles = Vec::new();
        for _ in 0..TRANSFER_CONFLICT_METADATA_CONCURRENCY {
            let Some((position, transfer)) = indexed_transfers.next() else {
                break;
            };
            handles.push(tokio::spawn(async move {
                (position, transfer_conflict(transfer).await)
            }));
        }

        if handles.is_empty() {
            break;
        }

        for handle in handles {
            if let Ok((position, Some(conflict))) = handle.await {
                indexed_conflicts.push((position, conflict));
            }
        }
    }

    indexed_conflicts.sort_by_key(|(position, _)| *position);
    indexed_conflicts
        .into_iter()
        .map(|(_, conflict)| conflict)
        .collect()
}

pub async fn is_transfer_target_available(path: impl AsRef<Path>) -> Result<bool, FileError> {
    let path = path.as_ref().to_path_buf();
    transfer_target_metadata_if_exists(&path)
        .await
        .map(|metadata| metadata.is_none())
        .map_err(|source| FileError::Metadata { path, source })
}

pub async fn available_transfer_target_path(path: impl AsRef<Path>) -> Result<PathBuf, FileError> {
    let path = path.as_ref().to_path_buf();
    if is_transfer_target_available(&path).await? {
        return Ok(path);
    }

    let (parent, name) = transfer_target_parent_and_name(&path);
    for index in 2..1001 {
        let candidate = transfer_target_copy_candidate(&parent, &name, index);
        if is_transfer_target_available(&candidate).await? {
            return Ok(candidate);
        }
    }

    Ok(path)
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

pub(crate) async fn available_transfer_target_path_candidate(path: &Path) -> io::Result<PathBuf> {
    if transfer_target_metadata_if_exists(path).await?.is_none() {
        return Ok(path.to_path_buf());
    }

    let (parent, name) = transfer_target_parent_and_name(path);
    for index in 2..1001 {
        let candidate = transfer_target_copy_candidate(&parent, &name, index);
        if transfer_target_metadata_if_exists(&candidate)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }

    Ok(path.to_path_buf())
}

async fn transfer_conflict(transfer: TransferConflictCheck) -> Option<TransferConflictItem> {
    let source_metadata = transfer_target_metadata_if_exists(&transfer.source)
        .await
        .ok()
        .flatten()?;
    let target_metadata = transfer_target_metadata_if_exists(&transfer.target)
        .await
        .ok()
        .flatten()?;

    Some(TransferConflictItem {
        source: transfer.source,
        target: transfer.target,
        source_metadata: transfer_conflict_metadata(source_metadata),
        target_metadata: transfer_conflict_metadata(target_metadata),
    })
}

fn transfer_conflict_metadata(metadata: std::fs::Metadata) -> TransferConflictMetadata {
    TransferConflictMetadata {
        is_directory: metadata.is_dir(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    }
}

fn transfer_target_parent_and_name(path: &Path) -> (PathBuf, OsString) {
    let parent = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
    let name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("item"));
    (parent, name)
}

/// 重名自动改名的候选名:序号插在扩展名之前,保住扩展名,
/// 文件格式识别才不受影响(`Screenshot.png` 被占用 → `Screenshot 2.png`)。
/// 无扩展名与 `.bashrc` 这类隐藏文件整体视作主名,序号直接尾追,
/// 扩展名拆分语义与 app-ui `entry_naming` 的 `split_name` 一致。
pub fn numbered_duplicate_name(name: &std::ffi::OsStr, number: usize) -> OsString {
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => {
            let mut candidate = stem.to_os_string();
            candidate.push(format!(" {number}."));
            candidate.push(extension);
            candidate
        }
        _ => {
            let mut candidate = name.to_os_string();
            candidate.push(format!(" {number}"));
            candidate
        }
    }
}

fn transfer_target_copy_candidate(parent: &Path, name: &std::ffi::OsStr, index: usize) -> PathBuf {
    parent.join(numbered_duplicate_name(name, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_number_goes_between_name_and_extension() {
        assert_eq!(
            numbered_duplicate_name(std::ffi::OsStr::new("Screenshot.png"), 2),
            OsString::from("Screenshot 2.png")
        );
    }

    #[test]
    fn duplicate_number_increments_for_each_taken_candidate() {
        assert_eq!(
            numbered_duplicate_name(std::ffi::OsStr::new("report.pdf"), 3),
            OsString::from("report 3.pdf")
        );
    }

    #[test]
    fn extensionless_names_take_the_number_directly() {
        assert_eq!(
            numbered_duplicate_name(std::ffi::OsStr::new("notes"), 2),
            OsString::from("notes 2")
        );
    }

    #[test]
    fn hidden_extensionless_files_use_whole_name_as_stem() {
        assert_eq!(
            numbered_duplicate_name(std::ffi::OsStr::new(".bashrc"), 2),
            OsString::from(".bashrc 2")
        );
    }
}
