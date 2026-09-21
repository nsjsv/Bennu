use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use tokio::fs;

use crate::FileError;

#[derive(Debug, Clone)]
pub(super) enum TransferSourceKind {
    RegularFile,
    Directory,
    SymbolicLink { target: PathBuf },
}

#[derive(Debug, Clone)]
pub(super) struct TransferSourceObject {
    pub(super) kind: TransferSourceKind,
    pub(super) metadata: std::fs::Metadata,
}

impl TransferSourceObject {
    pub(super) fn is_directory(&self) -> bool {
        matches!(self.kind, TransferSourceKind::Directory)
    }

    pub(super) fn progress_bytes_total(&self) -> u64 {
        match self.kind {
            TransferSourceKind::RegularFile | TransferSourceKind::Directory => self.metadata.len(),
            TransferSourceKind::SymbolicLink { .. } => 0,
        }
    }
}

pub(super) async fn inspect_transfer_source(
    path: &Path,
) -> Result<TransferSourceObject, FileError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|source| FileError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
    let file_type = metadata.file_type();

    let kind = if file_type.is_file() {
        TransferSourceKind::RegularFile
    } else if file_type.is_dir() {
        TransferSourceKind::Directory
    } else if file_type.is_symlink() {
        let target = fs::read_link(path)
            .await
            .map_err(|source| FileError::Metadata {
                path: path.to_path_buf(),
                source,
            })?;
        TransferSourceKind::SymbolicLink { target }
    } else {
        return Err(unsupported_transfer_source(path, &file_type));
    };

    Ok(TransferSourceObject { kind, metadata })
}

fn unsupported_transfer_source(path: &Path, file_type: &std::fs::FileType) -> FileError {
    FileError::InvalidInput {
        path: path.to_path_buf(),
        message: format!(
            "cannot transfer unsupported {} object",
            special_file_kind(file_type)
        ),
    }
}

#[cfg(unix)]
fn special_file_kind(file_type: &std::fs::FileType) -> &'static str {
    if file_type.is_fifo() {
        "FIFO"
    } else if file_type.is_socket() {
        "socket"
    } else if file_type.is_block_device() {
        "block device"
    } else if file_type.is_char_device() {
        "character device"
    } else {
        "filesystem"
    }
}

#[cfg(not(unix))]
fn special_file_kind(_file_type: &std::fs::FileType) -> &'static str {
    "filesystem"
}
