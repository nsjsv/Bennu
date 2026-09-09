use file_core::{
    ArchiveCompressionLevel, ArchiveFormat, BatchRenameItem, FileOperationVerification,
    TransferConflictStrategy, TrashRestoreEntry,
};
use file_operation_store::{
    StoredArchiveCompressionLevel, StoredArchiveFormat, StoredBatchRenameItem,
    StoredFileOperationVerification, StoredOperation, StoredPath, StoredSymbolicLinkCreation,
    StoredTransfer, StoredTransferConflictStrategy, StoredTrashEntry, TRANSFER_JOURNAL_VERSION,
};

use super::{QueuedFileOperation, QueuedTransfer};

pub(super) fn queued_operation_to_stored(operation: &QueuedFileOperation) -> StoredOperation {
    match operation {
        QueuedFileOperation::Rename { path, new_name } => StoredOperation::Rename {
            path: StoredPath::from_path(path),
            new_name: new_name.clone(),
        },
        QueuedFileOperation::BatchRename { items } => StoredOperation::BatchRename {
            items: stored_batch_rename_items(items),
        },
        QueuedFileOperation::CreateDirectory { parent } => StoredOperation::CreateDirectory {
            parent: StoredPath::from_path(parent),
        },
        QueuedFileOperation::CreateEmptyFile { parent } => StoredOperation::CreateEmptyFile {
            parent: StoredPath::from_path(parent),
        },
        QueuedFileOperation::Trash { paths } => StoredOperation::Trash {
            paths: paths
                .iter()
                .map(|path| StoredPath::from_path(path))
                .collect(),
        },
        QueuedFileOperation::Restore { entries } => StoredOperation::Restore {
            entries: stored_trash_entries(entries),
        },
        QueuedFileOperation::DeleteTrashEntries { entries } => {
            StoredOperation::DeleteTrashEntries {
                entries: stored_trash_entries(entries),
            }
        }
        QueuedFileOperation::DeletePermanently { paths } => StoredOperation::DeletePermanently {
            paths: paths
                .iter()
                .map(|path| StoredPath::from_path(path))
                .collect(),
        },
        QueuedFileOperation::EmptyTrash => StoredOperation::EmptyTrash,
        QueuedFileOperation::Copy {
            transfers,
            verification,
        }
        | QueuedFileOperation::Duplicate {
            transfers,
            verification,
        } => {
            // 复制副本与复制共用同一存储与恢复语义;重启恢复后按普通复制续跑,
            // 丢掉的只是「完成后选中新副本」这一层收尾。
            StoredOperation::Copy {
                transfers: stored_transfers(transfers),
                verification: stored_verification(*verification),
                recovery_version: Some(TRANSFER_JOURNAL_VERSION),
            }
        }
        QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources } => {
            StoredOperation::GatherSelectionIntoNewFolder {
                directory: StoredPath::from_path(directory),
                sources: sources
                    .iter()
                    .map(|path| StoredPath::from_path(path))
                    .collect(),
            }
        }
        QueuedFileOperation::UngatherNewFolder {
            directory,
            restore_targets,
        } => StoredOperation::UngatherNewFolder {
            directory: StoredPath::from_path(directory),
            restore_targets: restore_targets
                .iter()
                .map(|path| StoredPath::from_path(path))
                .collect(),
        },
        QueuedFileOperation::CreateSymbolicLinks { links } => StoredOperation::CreateSymbolicLinks {
            links: links
                .iter()
                .map(|link| StoredSymbolicLinkCreation {
                    link_path: StoredPath::from_path(&link.link_path),
                    target_path: StoredPath::from_path(&link.target_path),
                })
                .collect(),
        },
        QueuedFileOperation::Move {
            transfers,
            verification,
        } => StoredOperation::Move {
            transfers: stored_transfers(transfers),
            verification: stored_verification(*verification),
            recovery_version: Some(TRANSFER_JOURNAL_VERSION),
        },
        QueuedFileOperation::CreateArchive {
            sources,
            target,
            format,
            compression_level,
            password,
        } => StoredOperation::CreateArchive {
            sources: sources
                .iter()
                .map(|path| StoredPath::from_path(path))
                .collect(),
            target: StoredPath::from_path(target),
            format: stored_archive_format(*format),
            compression_level: stored_archive_compression_level(*compression_level),
            password_required: password.is_some(),
        },
        QueuedFileOperation::ExtractArchive { request } => StoredOperation::ExtractArchive {
            archive: StoredPath::from_path(&request.archive),
            destination: StoredPath::from_path(&request.destination),
            password_required: request.password.is_some(),
        },
        QueuedFileOperation::Convert { requests } => StoredOperation::Convert {
            sources: requests
                .iter()
                .map(|request| StoredPath::from_path(&request.source))
                .collect(),
            output_extensions: requests
                .iter()
                .map(|request| request.target.extension().to_owned())
                .collect(),
        },
    }
}

pub(super) fn queued_operation_from_stored(
    operation: StoredOperation,
) -> Option<QueuedFileOperation> {
    match operation {
        StoredOperation::Copy {
            transfers,
            verification,
            recovery_version: Some(TRANSFER_JOURNAL_VERSION),
        } => Some(QueuedFileOperation::Copy {
            transfers: queued_transfers(transfers),
            verification: queued_verification(verification),
        }),
        StoredOperation::Move {
            transfers,
            verification,
            recovery_version: Some(TRANSFER_JOURNAL_VERSION),
        } => Some(QueuedFileOperation::Move {
            transfers: queued_transfers(transfers),
            verification: queued_verification(verification),
        }),
        _ => None,
    }
}

fn queued_transfers(transfers: Vec<StoredTransfer>) -> Vec<QueuedTransfer> {
    transfers
        .into_iter()
        .map(|transfer| QueuedTransfer {
            source: transfer.source.to_path_buf(),
            target: transfer.target.to_path_buf(),
            conflict_strategy: queued_conflict_strategy(transfer.conflict_strategy),
        })
        .collect()
}

fn queued_conflict_strategy(
    conflict_strategy: StoredTransferConflictStrategy,
) -> TransferConflictStrategy {
    match conflict_strategy {
        StoredTransferConflictStrategy::Fail => TransferConflictStrategy::Fail,
        StoredTransferConflictStrategy::Replace => TransferConflictStrategy::Replace,
        StoredTransferConflictStrategy::Skip => TransferConflictStrategy::Skip,
        StoredTransferConflictStrategy::KeepBoth => TransferConflictStrategy::KeepBoth,
        StoredTransferConflictStrategy::Merge => TransferConflictStrategy::Merge,
    }
}

fn queued_verification(verification: StoredFileOperationVerification) -> FileOperationVerification {
    match verification {
        StoredFileOperationVerification::BasicMetadata => FileOperationVerification::BasicMetadata,
        StoredFileOperationVerification::Strong => FileOperationVerification::Strong,
    }
}

fn stored_transfers(transfers: &[QueuedTransfer]) -> Vec<StoredTransfer> {
    transfers
        .iter()
        .map(|transfer| StoredTransfer {
            source: StoredPath::from_path(&transfer.source),
            target: StoredPath::from_path(&transfer.target),
            conflict_strategy: stored_conflict_strategy(transfer.conflict_strategy),
        })
        .collect()
}

fn stored_conflict_strategy(
    conflict_strategy: file_core::TransferConflictStrategy,
) -> StoredTransferConflictStrategy {
    match conflict_strategy {
        file_core::TransferConflictStrategy::Fail => StoredTransferConflictStrategy::Fail,
        file_core::TransferConflictStrategy::Replace => StoredTransferConflictStrategy::Replace,
        file_core::TransferConflictStrategy::Skip => StoredTransferConflictStrategy::Skip,
        file_core::TransferConflictStrategy::KeepBoth => StoredTransferConflictStrategy::KeepBoth,
        file_core::TransferConflictStrategy::Merge => StoredTransferConflictStrategy::Merge,
    }
}

fn stored_verification(
    verification: file_core::FileOperationVerification,
) -> StoredFileOperationVerification {
    match verification {
        file_core::FileOperationVerification::BasicMetadata => {
            StoredFileOperationVerification::BasicMetadata
        }
        file_core::FileOperationVerification::Strong => StoredFileOperationVerification::Strong,
    }
}

fn stored_batch_rename_items(items: &[BatchRenameItem]) -> Vec<StoredBatchRenameItem> {
    items
        .iter()
        .map(|item| StoredBatchRenameItem {
            from: StoredPath::from_path(&item.from),
            to: StoredPath::from_path(&item.to),
        })
        .collect()
}

fn stored_trash_entries(entries: &[TrashRestoreEntry]) -> Vec<StoredTrashEntry> {
    entries
        .iter()
        .map(|entry| StoredTrashEntry {
            trash_path: StoredPath::from_path(&entry.trash_path),
            info_path: StoredPath::from_path(&entry.info_path),
            original_path: StoredPath::from_path(&entry.original_path),
        })
        .collect()
}

fn stored_archive_format(format: ArchiveFormat) -> StoredArchiveFormat {
    match format {
        ArchiveFormat::Zip => StoredArchiveFormat::Zip,
        ArchiveFormat::SevenZip => StoredArchiveFormat::SevenZip,
        ArchiveFormat::TarGz => StoredArchiveFormat::TarGz,
    }
}

fn stored_archive_compression_level(
    compression_level: ArchiveCompressionLevel,
) -> StoredArchiveCompressionLevel {
    match compression_level {
        ArchiveCompressionLevel::Store => StoredArchiveCompressionLevel::Store,
        ArchiveCompressionLevel::Fast => StoredArchiveCompressionLevel::Fast,
        ArchiveCompressionLevel::Balanced => StoredArchiveCompressionLevel::Balanced,
        ArchiveCompressionLevel::Maximum => StoredArchiveCompressionLevel::Maximum,
    }
}
