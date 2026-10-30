use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::FileError;

mod batch_rename;
mod convert;
mod copy;
mod copy_verification;
mod entry_moves;
mod metadata_loss;
mod recoverable_transfer;
pub use convert::{
    available_conversion_encoders, convert_file_with_controls, locate_media_tool, media_duration,
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionRequest, ConversionTarget, ConvertedFile, ImageTargetFormat, QualitySpec, ResizeSpec,
    VideoTargetFormat,
};
pub(crate) use recoverable_transfer::rename_noreplace;
pub use recoverable_transfer::{
    advance_recoverable_transfer, is_direct_move_segment_candidate,
    persist_recoverable_source_manifest, persist_recoverable_source_manifest_with_controls,
    prepare_direct_move_intent_segment, run_direct_move_batch_to_durable_renamed,
    run_recoverable_transfer, run_recoverable_transfer_to_direct_move_intent,
    settle_failed_recoverable_transfer, ArtifactOwner, ArtifactToken, BackupCreationTransfer,
    CommitPayload, CommitTransfer, CommittedTransfer, CompletedTarget, DirectMoveBatchRecord,
    DirectMoveIntentBatchRecord, DirectMoveIntentBoundary, FileIdentity, FileObjectKind,
    ManifestCheckpointBatchUpdate, MergeChildCompletion, MergeChildOutcome, MergeTransfer,
    ObjectFingerprint, OwnedArtifact, OwnedArtifactKind, OwnedArtifactPlan, PreparedTransfer,
    RecoverableTransferError, RecoverableTransferOperation, RecoverableTransferOutcome,
    RecoverableTransferRequest, RenamedDirectMove, RetiredSource, SourceDisposition,
    SourceManifest, SourceManifestEntry, SourceRetirementPlan, StagedSourceLocation,
    StagingTransfer, TransferAdvance, TransferCheckpoint, TransferCheckpointSwap,
    TransferExecutionKind, TransferFailureIntent, TransferFingerprint, TransferJournal,
    TransferJournalError, TransferJournalFuture, TransferJournalMutation, TransferJournalRecord,
    TransferWorkKey,
};
mod transfer_metadata;
mod transfer_object;
mod transfer_strategy;
pub use batch_rename::{batch_rename_paths, BatchRenameItem, CompletedBatchRename};
pub use copy::{
    copy_path, copy_path_with_options, CopiedTarget, CopyProgress, FileOperationControls,
    FileOperationRunState, FileOperationVerification, FileTransferOptions, ProgressSender,
    TransferConflictStrategy,
};
pub use entry_moves::{rename_entry, UnrestoredEntry};
pub use metadata_loss::{
    MetadataKind, MetadataLossSample, MetadataLossSummary, METADATA_LOSS_SAMPLE_LIMIT,
};
use transfer_object::{TransferSourceKind, TransferSourceObject};
pub use transfer_strategy::{
    copy_regular_file_payload, ficlone_supported, PayloadCopyOutcome, PayloadCopyStrategy,
    RegularFilePayloadCopy, TransferStrategyEngine,
};

pub async fn create_directory(path: impl AsRef<Path>) -> Result<PathBuf, FileError> {
    let path = path.as_ref().to_path_buf();
    fs::create_dir(&path)
        .await
        .map_err(|source| FileError::CreateDirectory {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

pub async fn create_empty_file(path: impl AsRef<Path>) -> Result<PathBuf, FileError> {
    let path = path.as_ref().to_path_buf();
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|source| FileError::CreateFile {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

pub async fn create_file_with_contents(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> Result<PathBuf, FileError> {
    let path = path.as_ref().to_path_buf();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|source| FileError::CreateFile {
            path: path.clone(),
            source,
        })?;
    file.write_all(contents.as_ref())
        .await
        .map_err(|source| FileError::CreateFile {
            path: path.clone(),
            source,
        })?;
    file.flush().await.map_err(|source| FileError::CreateFile {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

pub async fn delete_path_permanently(path: impl AsRef<Path>) -> Result<(), FileError> {
    let path = path.as_ref().to_path_buf();
    let metadata = fs::symlink_metadata(&path)
        .await
        .map_err(|source| FileError::Delete {
            path: path.clone(),
            source,
        })?;
    let outcome = if metadata.file_type().is_dir() {
        fs::remove_dir_all(&path).await
    } else {
        fs::remove_file(&path).await
    };
    outcome.map_err(|source| FileError::Delete { path, source })
}

fn already_exists_error() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "target already exists")
}

pub(super) fn ensure_replace_target_does_not_contain_source_path(
    from: &Path,
    to: &Path,
    target_metadata: &std::fs::Metadata,
) -> Result<(), FileError> {
    if target_metadata.is_dir() && from.starts_with(to) {
        return Err(FileError::InvalidInput {
            path: to.to_path_buf(),
            message: "cannot replace a target directory that contains the source path".to_owned(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn copy_keep_both_uses_next_available_path() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        let target_numbered_2 = directory.path().join("target 2.txt");
        let target_numbered_3 = directory.path().join("target 3.txt");

        fs::write(&source, b"new").await.unwrap();
        fs::write(&target, b"old").await.unwrap();
        fs::write(&target_numbered_2, b"old copy").await.unwrap();

        copy_path_with_options(
            &source,
            &target,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
        )
        .await
        .unwrap();

        assert_eq!(fs::read(&target).await.unwrap(), b"old");
        assert_eq!(fs::read(&target_numbered_2).await.unwrap(), b"old copy");
        assert_eq!(fs::read(&target_numbered_3).await.unwrap(), b"new");
    }
}
