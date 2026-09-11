use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::transfer_conflict::{
    available_transfer_target_path_candidate, transfer_target_metadata_if_exists,
};
use crate::FileError;

mod batch_rename;
mod convert;
mod copy;
mod copy_verification;
mod recoverable_transfer;
pub use convert::{
    available_conversion_encoders, convert_file_with_controls, locate_media_tool, media_duration,
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionRequest, ConversionTarget, ConvertedFile, ImageTargetFormat, QualitySpec, ResizeSpec,
    VideoTargetFormat,
};
pub(crate) use recoverable_transfer::rename_noreplace;
pub use recoverable_transfer::{
    is_direct_move_segment_candidate, persist_recoverable_source_manifest,
    persist_recoverable_source_manifest_with_controls, prepare_direct_move_intent_segment,
    run_direct_move_batch_to_durable_renamed, run_recoverable_transfer,
    run_recoverable_transfer_to_direct_move_intent, settle_failed_recoverable_transfer,
    ArtifactOwner, ArtifactToken,
    BackupCreationTransfer, CommitPayload, CommitTransfer, CommittedTransfer, CompletedTarget,
    DirectMoveBatchRecord, DirectMoveIntentBatchRecord, DirectMoveIntentBoundary, FileIdentity,
    FileObjectKind, ManifestCheckpointBatchUpdate, MergeChildCompletion, MergeChildOutcome,
    MergeTransfer, ObjectFingerprint, OwnedArtifact, OwnedArtifactKind, OwnedArtifactPlan,
    PreparedTransfer, RecoverableTransferError, RecoverableTransferOperation,
    RecoverableTransferOutcome, RecoverableTransferRequest, RenamedDirectMove, RetiredSource,
    SourceDisposition, SourceManifest, SourceManifestEntry, SourceRetirementPlan,
    StagedSourceLocation, StagingTransfer, TransferCheckpoint, TransferCheckpointSwap,
    TransferExecutionKind, TransferFailureIntent, TransferJournal, TransferJournalError,
    TransferJournalFuture, TransferJournalMutation, TransferJournalRecord, TransferWorkKey,
};
mod transfer_metadata;
mod transfer_object;
pub use batch_rename::{batch_rename_paths, BatchRenameItem, CompletedBatchRename};
use copy::copy_path_with_inspected_source;
pub use copy::{
    copy_path, copy_path_with_options, CopyProgress, FileOperationControls, FileOperationRunState,
    FileOperationVerification, FileTransferOptions, ProgressSender, TransferConflictStrategy,
};
use transfer_object::{inspect_transfer_source, TransferSourceKind, TransferSourceObject};

pub async fn rename_path(
    path: impl AsRef<Path>,
    new_name: impl AsRef<OsStr>,
) -> Result<PathBuf, FileError> {
    let path = path.as_ref().to_path_buf();
    let parent = path
        .parent()
        .ok_or_else(|| FileError::InvalidInput {
            path: path.clone(),
            message: "path has no parent".to_owned(),
        })?
        .to_path_buf();
    let target = parent.join(new_name.as_ref());

    fs::rename(&path, &target)
        .await
        .map_err(|source| FileError::Rename {
            from: path.clone(),
            to: target.clone(),
            source,
        })?;

    Ok(target)
}

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

async fn try_collapse_replace_move_into_empty_target_parent(
    from: &Path,
    to: &Path,
    source_metadata: &std::fs::Metadata,
    target_metadata: Option<&std::fs::Metadata>,
) -> Result<Option<PathBuf>, FileError> {
    let Some(target_metadata) = target_metadata else {
        return Ok(None);
    };

    if !source_metadata.is_dir() || !target_metadata.is_dir() {
        return Ok(None);
    }
    if from.parent() != Some(to) || from.file_name() != to.file_name() {
        return Ok(None);
    }

    let source_file_type = fs::symlink_metadata(from)
        .await
        .map_err(|source| FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?
        .file_type();
    if !source_file_type.is_dir() {
        return Ok(None);
    }
    if !target_directory_contains_only_source_path(from, to).await? {
        return Ok(None);
    }

    let collapsed_target =
        available_transfer_target_path_candidate(to)
            .await
            .map_err(|source| FileError::Move {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                source,
            })?;
    fs::rename(from, &collapsed_target)
        .await
        .map_err(|source| FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;

    if let Err(source) = fs::remove_dir(to).await {
        let _ = fs::rename(&collapsed_target, from).await;
        return Err(FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        });
    }

    if let Err(source) = fs::rename(&collapsed_target, to).await {
        // 这里先尽量恢复原始父目录，再把临时目录放回去，避免把源目录丢成孤儿路径。
        let _ = fs::create_dir(to).await;
        let _ = fs::rename(&collapsed_target, from).await;
        return Err(FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        });
    }

    Ok(Some(to.to_path_buf()))
}

async fn target_directory_contains_only_source_path(
    from: &Path,
    to: &Path,
) -> Result<bool, FileError> {
    let mut entries = fs::read_dir(to).await.map_err(|source| FileError::Move {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })?;
    let mut found_source = false;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?
    {
        if entry.path() == from {
            found_source = true;
            continue;
        }

        return Ok(false);
    }

    Ok(found_source)
}

pub async fn move_path(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    cancel: CancellationToken,
    progress: Option<ProgressSender>,
) -> Result<(), FileError> {
    move_path_with_options(
        from,
        to,
        FileTransferOptions::running(cancel).with_optional_progress(progress),
    )
    .await
    .map(|_| ())
}

pub async fn move_path_with_options(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    transfer_options: FileTransferOptions,
) -> Result<Option<PathBuf>, FileError> {
    let from = from.as_ref().to_path_buf();
    let to = to.as_ref().to_path_buf();
    let mut controls = transfer_options.controls;
    let progress = transfer_options.progress;
    let conflict_strategy = transfer_options.conflict_strategy;
    let verification = transfer_options.verification;
    controls.wait_until_running().await?;

    if from == to {
        return Ok(None);
    }

    let source_object = inspect_transfer_source(&from).await?;
    let source_metadata = &source_object.metadata;
    let progress = match &source_object.kind {
        TransferSourceKind::SymbolicLink { .. } => None,
        TransferSourceKind::RegularFile | TransferSourceKind::Directory => progress,
    };

    let total = if progress.is_some() {
        source_object.progress_bytes_total()
    } else {
        0
    };

    let target_metadata = transfer_target_metadata_if_exists(&to)
        .await
        .map_err(|source| FileError::Move {
            from: from.clone(),
            to: to.clone(),
            source,
        })?;

    if conflict_strategy == TransferConflictStrategy::Merge
        && source_object.is_directory()
        && target_metadata
            .as_ref()
            .is_some_and(std::fs::Metadata::is_dir)
    {
        move_directory_merge(&from, &to, &mut controls, verification).await?;
        send_move_progress(progress, from, to.clone(), total);
        return Ok(Some(to));
    }

    if conflict_strategy == TransferConflictStrategy::Replace {
        if let Some(collapsed_target) = try_collapse_replace_move_into_empty_target_parent(
            &from,
            &to,
            source_metadata,
            target_metadata.as_ref(),
        )
        .await?
        {
            send_move_progress(progress, from, collapsed_target.clone(), total);
            return Ok(Some(collapsed_target));
        }
    }

    let Some(to) =
        prepare_move_target(&from, &to, target_metadata.as_ref(), conflict_strategy).await?
    else {
        send_move_progress(progress, from, to, total);
        return Ok(None);
    };

    move_prepared_path(
        &from,
        &to,
        &source_object,
        &mut controls,
        progress.clone(),
        verification,
    )
    .await?;

    send_move_progress(progress, from, to.clone(), total);

    Ok(Some(to))
}

async fn prepare_move_target(
    from: &Path,
    to: &Path,
    target_metadata: Option<&std::fs::Metadata>,
    conflict_strategy: TransferConflictStrategy,
) -> Result<Option<PathBuf>, FileError> {
    let Some(target_metadata) = target_metadata else {
        return Ok(Some(to.to_path_buf()));
    };

    match conflict_strategy {
        TransferConflictStrategy::Fail => Err(FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source: already_exists_error(),
        }),
        TransferConflictStrategy::Replace => {
            ensure_replace_target_does_not_contain_source_path(from, to, target_metadata)?;
            remove_move_target(from, to, target_metadata).await?;
            Ok(Some(to.to_path_buf()))
        }
        TransferConflictStrategy::Skip | TransferConflictStrategy::Merge => Ok(None),
        TransferConflictStrategy::KeepBoth => available_transfer_target_path_candidate(to)
            .await
            .map(Some)
            .map_err(|source| FileError::Move {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                source,
            }),
    }
}

async fn remove_move_target(
    from: &Path,
    to: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), FileError> {
    let result = if metadata.is_dir() {
        fs::remove_dir_all(to).await
    } else {
        fs::remove_file(to).await
    };

    result.map_err(|source| FileError::Move {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })
}

async fn move_prepared_path(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
    controls: &mut FileOperationControls,
    progress: Option<ProgressSender>,
    verification: FileOperationVerification,
) -> Result<(), FileError> {
    match fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(source) if is_cross_device_rename_error(&source) => {
            copy_then_remove_source(from, to, source_object, controls, progress, verification).await
        }
        Err(source) => Err(FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        }),
    }
}

async fn copy_then_remove_source(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
    controls: &mut FileOperationControls,
    progress: Option<ProgressSender>,
    verification: FileOperationVerification,
) -> Result<(), FileError> {
    let copy_options = FileTransferOptions::new(controls.clone())
        .with_optional_progress(progress)
        .with_conflict_strategy(TransferConflictStrategy::Fail)
        .with_verification(verification);
    copy_path_with_inspected_source(from, to, source_object, copy_options).await?;
    remove_moved_source(from, to, source_object).await
}

async fn remove_moved_source(
    from: &Path,
    to: &Path,
    source_object: &TransferSourceObject,
) -> Result<(), FileError> {
    let result = if source_object.is_directory() {
        fs::remove_dir_all(from).await
    } else {
        fs::remove_file(from).await
    };

    result.map_err(|source| FileError::Move {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn is_cross_device_rename_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(18)
}

#[cfg(not(unix))]
fn is_cross_device_rename_error(_error: &io::Error) -> bool {
    false
}

enum MoveDirectoryStep {
    MoveChildren { source: PathBuf, target: PathBuf },
    RemoveSourceDirectory { source: PathBuf, target: PathBuf },
}

async fn move_directory_merge(
    from: &Path,
    to: &Path,
    controls: &mut FileOperationControls,
    verification: FileOperationVerification,
) -> Result<(), FileError> {
    if to.starts_with(from) {
        return Err(FileError::InvalidInput {
            path: to.to_path_buf(),
            message: "cannot move a directory into itself".to_owned(),
        });
    }

    let mut pending_steps = vec![MoveDirectoryStep::MoveChildren {
        source: from.to_path_buf(),
        target: to.to_path_buf(),
    }];
    while let Some(step) = pending_steps.pop() {
        controls.wait_until_running().await?;
        let (source_directory, target_directory) = match step {
            MoveDirectoryStep::MoveChildren { source, target } => (source, target),
            MoveDirectoryStep::RemoveSourceDirectory { source, target } => {
                remove_empty_moved_directory(&source, &target).await?;
                continue;
            }
        };

        pending_steps.push(MoveDirectoryStep::RemoveSourceDirectory {
            source: source_directory.clone(),
            target: target_directory.clone(),
        });
        let mut entries =
            fs::read_dir(&source_directory)
                .await
                .map_err(|source| FileError::Move {
                    from: source_directory.clone(),
                    to: target_directory.clone(),
                    source,
                })?;

        loop {
            controls.wait_until_running().await?;
            let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|source| FileError::Move {
                    from: source_directory.clone(),
                    to: target_directory.clone(),
                    source,
                })?
            else {
                break;
            };

            let source_child = entry.path();
            let target_child = target_directory.join(entry.file_name());
            let source_object = inspect_transfer_source(&source_child).await?;
            let target_metadata = transfer_target_metadata_if_exists(&target_child)
                .await
                .map_err(|source| FileError::Move {
                    from: source_child.clone(),
                    to: target_child.clone(),
                    source,
                })?;

            if let Some(target_metadata) = target_metadata {
                if source_object.is_directory() && target_metadata.is_dir() {
                    pending_steps.push(MoveDirectoryStep::MoveChildren {
                        source: source_child,
                        target: target_child,
                    });
                }
                continue;
            }

            move_prepared_path(
                &source_child,
                &target_child,
                &source_object,
                controls,
                None,
                verification,
            )
            .await?;
        }
    }

    Ok(())
}

async fn remove_empty_moved_directory(from: &Path, to: &Path) -> Result<(), FileError> {
    match fs::remove_dir(from).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(FileError::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        }),
    }
}

fn send_move_progress(progress: Option<ProgressSender>, from: PathBuf, to: PathBuf, total: u64) {
    if let Some(progress) = progress {
        let _ = progress.send(CopyProgress {
            from,
            to,
            bytes_done: total,
            bytes_total: total,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    #[tokio::test]
    async fn copy_keep_both_uses_next_available_path() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        let target_copy1 = directory.path().join("target.txt.copy1");
        let target_copy2 = directory.path().join("target.txt.copy2");

        fs::write(&source, b"new").await.unwrap();
        fs::write(&target, b"old").await.unwrap();
        fs::write(&target_copy1, b"old copy").await.unwrap();

        copy_path_with_options(
            &source,
            &target,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
        )
        .await
        .unwrap();

        assert_eq!(fs::read(&target).await.unwrap(), b"old");
        assert_eq!(fs::read(&target_copy1).await.unwrap(), b"old copy");
        assert_eq!(fs::read(&target_copy2).await.unwrap(), b"new");
    }

    #[tokio::test]
    async fn move_keep_both_uses_next_available_path() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        let target_copy1 = directory.path().join("target.txt.copy1");
        let target_copy2 = directory.path().join("target.txt.copy2");

        fs::write(&source, b"new").await.unwrap();
        fs::write(&target, b"old").await.unwrap();
        fs::write(&target_copy1, b"old copy").await.unwrap();

        move_path_with_options(
            &source,
            &target,
            FileTransferOptions::running(CancellationToken::new())
                .with_conflict_strategy(TransferConflictStrategy::KeepBoth),
        )
        .await
        .unwrap();

        assert!(fs::metadata(&source).await.is_err());
        assert_eq!(fs::read(&target).await.unwrap(), b"old");
        assert_eq!(fs::read(&target_copy1).await.unwrap(), b"old copy");
        assert_eq!(fs::read(&target_copy2).await.unwrap(), b"new");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_then_remove_source_preserves_symbolic_link_identity() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source-link");
        let target = directory.path().join("target-link");
        symlink("missing-target", &source).unwrap();
        let source_object = inspect_transfer_source(&source).await.unwrap();
        let mut controls = FileOperationControls::running(CancellationToken::new());

        copy_then_remove_source(
            &source,
            &target,
            &source_object,
            &mut controls,
            None,
            FileOperationVerification::BasicMetadata,
        )
        .await
        .unwrap();

        assert!(fs::symlink_metadata(&source).await.is_err());
        assert!(fs::symlink_metadata(&target)
            .await
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&target).await.unwrap(),
            Path::new("missing-target")
        );
    }

    #[tokio::test]
    async fn copy_then_remove_source_keeps_source_after_hard_copy_failure() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        fs::write(&source, b"source").await.unwrap();
        fs::write(&target, b"target").await.unwrap();
        let source_object = inspect_transfer_source(&source).await.unwrap();
        let mut controls = FileOperationControls::running(CancellationToken::new());

        let error = copy_then_remove_source(
            &source,
            &target,
            &source_object,
            &mut controls,
            None,
            FileOperationVerification::BasicMetadata,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, FileError::Copy { .. }));
        assert_eq!(fs::read(&source).await.unwrap(), b"source");
        assert_eq!(fs::read(&target).await.unwrap(), b"target");
    }
}
