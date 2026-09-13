use std::path::PathBuf;

use super::commit::{
    commit_payload_identity, matches_object_verification, verify_completed_target,
};
use super::direct_move::renamed_target_matches_source;
use super::{next_recovered_path, path_exists, persist_checkpoint, sync_rename_parents};
use crate::ops::recoverable_transfer::{
    fingerprint_object, inspect_file_identity, recover_owned_artifact,
    remove_incomplete_empty_artifact, remove_owned_artifact_if_exists, rename_noreplace,
    CommitPayload, CommitTransfer, FileIdentity, OwnedArtifact, PreparedTransfer,
    RecoverableTransferError, StagedSourceLocation, StagingTransfer, TransferCheckpoint,
    TransferExecutionKind, TransferFailureIntent, TransferJournal, TransferJournalRecord,
};

pub(super) async fn fail_recoverable_transfer<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    diagnostic: String,
) -> Result<(), RecoverableTransferError> {
    let previous = record.checkpoint.clone();
    let failure = TransferFailureIntent {
        previous: Box::new(previous),
        diagnostic,
    };
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::FailureIntent(failure.clone()),
    )
    .await?;
    finish_failure(record, journal, failure).await
}

pub(super) async fn finish_failure<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    failure: TransferFailureIntent,
) -> Result<(), RecoverableTransferError> {
    let final_target = cleanup_checkpoint_files(record, &failure.previous).await?;
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::Failed {
            final_target,
            diagnostic: failure.diagnostic,
        },
    )
    .await
}

pub(super) async fn cancel_recoverable_transfer<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
) -> Result<(), RecoverableTransferError> {
    let previous = match record.checkpoint.clone() {
        TransferCheckpoint::CancelIntent(previous) => *previous,
        TransferCheckpoint::Canceled { .. } => return Ok(()),
        checkpoint => {
            persist_checkpoint(
                record,
                journal,
                TransferCheckpoint::CancelIntent(Box::new(checkpoint.clone())),
            )
            .await?;
            checkpoint
        }
    };
    finish_cancel(record, journal, previous).await
}

pub(super) async fn finish_cancel<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    previous: TransferCheckpoint,
) -> Result<(), RecoverableTransferError> {
    let final_target = cleanup_checkpoint_files(record, &previous).await?;
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::Canceled { final_target },
    )
    .await
}

async fn cleanup_checkpoint_files(
    record: &TransferJournalRecord,
    checkpoint: &TransferCheckpoint,
) -> Result<Option<PathBuf>, RecoverableTransferError> {
    match checkpoint {
        TransferCheckpoint::AwaitingManifest | TransferCheckpoint::Skipped => Ok(None),
        TransferCheckpoint::Merging(merge) => {
            if let Some(child) = merge.active_child.as_ref() {
                Box::pin(cleanup_checkpoint_files(child, &child.checkpoint)).await?;
            }
            Ok(None)
        }
        TransferCheckpoint::StageCreationIntent(prepared) => {
            if let Some(plan) = prepared.staging_plan.clone() {
                cleanup_stage_creation(record, prepared, plan).await?;
            }
            Ok(None)
        }
        TransferCheckpoint::Staging(staging) => {
            restore_or_remove_staging(record, staging).await?;
            Ok(None)
        }
        TransferCheckpoint::DirectMoveIntent(prepared) => {
            cleanup_direct_move_intent(record, prepared).await
        }
        TransferCheckpoint::DirectMoveRenamed(_)
        | TransferCheckpoint::BackupCreationIntent(_)
        | TransferCheckpoint::TargetCommitted(_)
        | TransferCheckpoint::SourceRetirementIntent(_)
        | TransferCheckpoint::SourceRetired(_) => {
            Err(RecoverableTransferError::InvalidCheckpoint {
                message: "forward-only transfer cleanup must only move forward".to_owned(),
            })
        }
        TransferCheckpoint::CommitIntent(commit) => cleanup_commit_intent(record, commit).await,
        TransferCheckpoint::Completed(completed) => {
            verify_completed_target(completed).await?;
            Ok(Some(completed.path.clone()))
        }
        TransferCheckpoint::CancelIntent(previous) => {
            Box::pin(cleanup_checkpoint_files(record, previous)).await
        }
        TransferCheckpoint::Canceled { final_target } => Ok(final_target.clone()),
        TransferCheckpoint::FailureIntent(failure) => {
            Box::pin(cleanup_checkpoint_files(record, &failure.previous)).await
        }
        TransferCheckpoint::Failed { final_target, .. } => Ok(final_target.clone()),
    }
}

async fn cleanup_direct_move_intent(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<Option<PathBuf>, RecoverableTransferError> {
    if path_exists(&record.request.source).await? {
        return Ok(None);
    }
    if !path_exists(&prepared.resolved_target).await? {
        return Err(RecoverableTransferError::SourceChanged {
            path: record.request.source.clone(),
        });
    }
    let target_identity = inspect_file_identity(&prepared.resolved_target).await?;
    if renamed_target_matches_source(&target_identity, &prepared.source_identity) {
        Err(RecoverableTransferError::InvalidCheckpoint {
            message: "a visible direct move can only recover forward".to_owned(),
        })
    } else {
        Err(RecoverableTransferError::TargetConflict {
            path: prepared.resolved_target.clone(),
        })
    }
}

async fn cleanup_stage_creation(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
    plan: crate::ops::recoverable_transfer::OwnedArtifactPlan,
) -> Result<(), RecoverableTransferError> {
    if !path_exists(&plan.root).await? {
        return Ok(());
    }
    if !path_exists(&plan.owner_path()).await? {
        return remove_incomplete_empty_artifact(&plan).await;
    }
    let artifact = recover_owned_artifact(plan).await?;
    let source_location = if prepared.execution == TransferExecutionKind::MoveToStage {
        StagedSourceLocation::ArtifactPayload
    } else {
        StagedSourceLocation::OriginalPath
    };
    restore_or_remove_artifact_payload(
        record,
        &artifact,
        &prepared.source_identity,
        prepared.source_fingerprint,
        source_location,
    )
    .await
}

async fn restore_or_remove_staging(
    record: &TransferJournalRecord,
    staging: &StagingTransfer,
) -> Result<(), RecoverableTransferError> {
    let source_location = if staging.prepared.execution == TransferExecutionKind::MoveToStage {
        StagedSourceLocation::ArtifactPayload
    } else {
        StagedSourceLocation::OriginalPath
    };
    restore_or_remove_artifact_payload(
        record,
        &staging.artifact,
        &staging.prepared.source_identity,
        staging.prepared.source_fingerprint,
        source_location,
    )
    .await
}

async fn restore_or_remove_artifact_payload(
    record: &TransferJournalRecord,
    artifact: &OwnedArtifact,
    source_identity: &FileIdentity,
    source_fingerprint: Option<crate::ops::recoverable_transfer::ObjectFingerprint>,
    source_location: StagedSourceLocation,
) -> Result<(), RecoverableTransferError> {
    if source_location == StagedSourceLocation::ArtifactPayload {
        restore_owned_payload(
            record,
            artifact,
            source_identity,
            source_fingerprint,
            PayloadIdentityExpectation::SameObject,
        )
        .await?;
    }
    remove_owned_artifact_if_exists(artifact).await
}

async fn cleanup_commit_intent(
    record: &TransferJournalRecord,
    commit: &CommitTransfer,
) -> Result<Option<PathBuf>, RecoverableTransferError> {
    if path_exists(&commit.prepared.resolved_target).await? {
        let target_identity = inspect_file_identity(&commit.prepared.resolved_target).await?;
        if matches_object_verification(
            &commit.prepared.resolved_target,
            &target_identity,
            commit_payload_identity(commit),
            commit.fingerprint,
        )
        .await?
        {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "committed target cannot be rolled back by terminal cleanup".to_owned(),
            });
        }

        let CommitPayload::Artifact {
            artifact,
            payload_identity,
            source_location,
        } = &commit.payload
        else {
            if path_exists(&record.request.source).await? {
                return Ok(None);
            }
            return Err(RecoverableTransferError::TargetConflict {
                path: commit.prepared.resolved_target.clone(),
            });
        };
        restore_displaced_backup(commit, artifact).await?;
        if *source_location == StagedSourceLocation::ArtifactPayload {
            restore_owned_payload(
                record,
                artifact,
                payload_identity,
                Some(commit.fingerprint),
                PayloadIdentityExpectation::Exact,
            )
            .await?;
        } else {
            verify_owned_payload(
                artifact,
                payload_identity,
                Some(commit.fingerprint),
                PayloadIdentityExpectation::Exact,
            )
            .await?;
        }
        remove_owned_artifact_if_exists(artifact).await?;
        return Ok(None);
    }

    if let CommitPayload::Artifact {
        artifact,
        payload_identity,
        source_location,
    } = &commit.payload
    {
        let backup_path = artifact.plan.backup_path();
        if path_exists(&backup_path).await? {
            verify_displaced_backup(commit, &backup_path).await?;
            rename_noreplace(&backup_path, &commit.prepared.resolved_target).map_err(|error| {
                error.into_transfer_error(&backup_path, &commit.prepared.resolved_target)
            })?;
            sync_rename_parents(&backup_path, &commit.prepared.resolved_target).await?;
        }
        if *source_location == StagedSourceLocation::ArtifactPayload {
            restore_owned_payload(
                record,
                artifact,
                payload_identity,
                Some(commit.fingerprint),
                PayloadIdentityExpectation::Exact,
            )
            .await?;
        } else {
            verify_owned_payload(
                artifact,
                payload_identity,
                Some(commit.fingerprint),
                PayloadIdentityExpectation::Exact,
            )
            .await?;
        }
        remove_owned_artifact_if_exists(artifact).await?;
    }
    Ok(None)
}

async fn restore_displaced_backup(
    commit: &CommitTransfer,
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    let backup_path = artifact.plan.backup_path();
    if !path_exists(&backup_path).await? {
        return Ok(());
    }
    verify_displaced_backup(commit, &backup_path).await?;
    let restored = next_recovered_path(&commit.prepared.resolved_target).await?;
    rename_noreplace(&backup_path, &restored)
        .map_err(|error| error.into_transfer_error(&backup_path, &restored))?;
    sync_rename_parents(&backup_path, &restored).await
}

async fn verify_displaced_backup(
    commit: &CommitTransfer,
    backup_path: &std::path::Path,
) -> Result<(), RecoverableTransferError> {
    let expected_identity = commit.backup_identity.as_ref().ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "commit backup has no verified backup identity".to_owned(),
        }
    })?;
    let expected_fingerprint = commit.prepared.expected_target_fingerprint.ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "commit backup has no expected target fingerprint".to_owned(),
        }
    })?;
    let backup_identity = inspect_file_identity(backup_path).await?;
    if backup_identity == *expected_identity
        && fingerprint_object(backup_path).await? == expected_fingerprint
    {
        Ok(())
    } else {
        Err(RecoverableTransferError::TargetConflict {
            path: backup_path.to_path_buf(),
        })
    }
}

#[derive(Clone, Copy)]
enum PayloadIdentityExpectation {
    Exact,
    SameObject,
}

async fn verify_owned_payload(
    artifact: &OwnedArtifact,
    expected_identity: &FileIdentity,
    expected_fingerprint: Option<crate::ops::recoverable_transfer::ObjectFingerprint>,
    identity_expectation: PayloadIdentityExpectation,
) -> Result<(), RecoverableTransferError> {
    crate::ops::recoverable_transfer::validate_owned_artifact(artifact).await?;
    let payload_path = artifact.plan.payload_path();
    let payload_identity = inspect_file_identity(&payload_path).await?;
    let payload_matches = match identity_expectation {
        PayloadIdentityExpectation::Exact => match expected_fingerprint {
            Some(expected) => {
                payload_identity == *expected_identity
                    && fingerprint_object(&payload_path).await? == expected
            }
            None => false,
        },
        PayloadIdentityExpectation::SameObject => {
            if !payload_identity.same_object(expected_identity) {
                false
            } else if let Some(expected) = expected_fingerprint {
                fingerprint_object(&payload_path).await? == expected
            } else {
                payload_identity.matches_staging_snapshot(expected_identity)
            }
        }
    };
    if payload_matches {
        Ok(())
    } else {
        Err(RecoverableTransferError::SourceChanged { path: payload_path })
    }
}

async fn restore_owned_payload(
    record: &TransferJournalRecord,
    artifact: &OwnedArtifact,
    expected_identity: &FileIdentity,
    expected_fingerprint: Option<crate::ops::recoverable_transfer::ObjectFingerprint>,
    identity_expectation: PayloadIdentityExpectation,
) -> Result<(), RecoverableTransferError> {
    let payload_path = artifact.plan.payload_path();
    if !path_exists(&payload_path).await? {
        if !path_exists(&record.request.source).await? {
            return Err(RecoverableTransferError::SourceChanged { path: payload_path });
        }
        let source_identity = inspect_file_identity(&record.request.source).await?;
        let source_matches = match identity_expectation {
            PayloadIdentityExpectation::Exact => match expected_fingerprint {
                Some(expected) => {
                    source_identity == *expected_identity
                        && fingerprint_object(&record.request.source).await? == expected
                }
                None => false,
            },
            PayloadIdentityExpectation::SameObject => {
                if !source_identity.same_object(expected_identity) {
                    false
                } else if let Some(expected) = expected_fingerprint {
                    fingerprint_object(&record.request.source).await? == expected
                } else {
                    source_identity.matches_staging_snapshot(expected_identity)
                }
            }
        };
        return if source_matches {
            Ok(())
        } else {
            Err(RecoverableTransferError::SourceChanged {
                path: record.request.source.clone(),
            })
        };
    }
    verify_owned_payload(
        artifact,
        expected_identity,
        expected_fingerprint,
        identity_expectation,
    )
    .await?;
    let restored = if !path_exists(&record.request.source).await? {
        record.request.source.clone()
    } else {
        next_recovered_path(&record.request.source).await?
    };
    rename_noreplace(&payload_path, &restored)
        .map_err(|error| error.into_transfer_error(&payload_path, &restored))?;
    sync_rename_parents(&payload_path, &restored).await
}
