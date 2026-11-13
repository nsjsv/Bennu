use std::path::Path;

use tokio_util::sync::CancellationToken;

use super::retirement::{
    advance_owned_tree_entry_deletion, prepare_owned_tree_entry_deletion_intent,
    reverse_manifest_entry, OwnedTreeEntryDeletionAdvance,
};
use super::{
    commit_artifact, commit_payload_path, manifest_root_identity, next_recovered_path, path_exists,
    record_manifest, record_replacement_manifest, target_parent,
};
use crate::ops::recoverable_transfer::{
    fingerprint_object_with_controls, inspect_file_identity, plan_owned_artifact,
    remove_empty_owned_artifact, rename_noreplace, validate_owned_artifact,
    verify_source_manifest_with_controls, BackupCreationTransfer, CommitBarrier, CommitPayload,
    CommitTransfer, CommittedTransfer, CompletedTarget, FileIdentity, NoReplaceRenameError,
    OwnedArtifactKind, PreparedTransfer, ProofContext, RecoverableTransferError,
    RecoverableTransferOperation, SourceDisposition, SourceRetirementPlan, StagedSourceLocation,
    TransferCheckpoint, TransferExecutionKind, TransferFingerprint, TransferJournalRecord,
    TransferTransition,
};
use crate::ops::FileOperationControls;
use crate::transfer_conflict::first_available_entry_path;
use crate::EntryNameKind;

pub(super) async fn commit_transfer(
    record: &TransferJournalRecord,
    mut commit: CommitTransfer,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    if commit.prepared.expected_target_identity.is_some() && commit.backup_identity.is_none() {
        if record.request.verification != crate::FileOperationVerification::Strong
            || commit.prepared.expected_target_fingerprint.is_none()
        {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "replacement commit has no verified backup identity".to_owned(),
            });
        }
        let backup_fingerprint = commit.fingerprint.as_blake3().ok_or_else(|| {
            RecoverableTransferError::InvalidCheckpoint {
                message: "replace commit requires a content fingerprint".to_owned(),
            }
        })?;
        return Ok(TransferTransition::Checkpoint(
            TransferCheckpoint::BackupCreationIntent(BackupCreationTransfer {
                prepared: commit.prepared,
                payload: commit.payload,
                fingerprint: backup_fingerprint,
                metadata_loss: commit.metadata_loss,
            }),
        ));
    }

    let payload_path = commit_payload_path(record, &commit);
    let payload_exists = path_exists(&payload_path).await?;
    let target_exists = path_exists(&commit.prepared.resolved_target).await?;

    if target_exists && !payload_exists {
        return persist_inferred_commit(record, commit, proof).await;
    }
    if !payload_exists {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    let payload_identity = inspect_file_identity(&payload_path).await?;
    if payload_identity != *commit_payload_identity(&commit)
        || !proof.matches(&payload_path, &commit.fingerprint).await?
    {
        return Err(RecoverableTransferError::FingerprintMismatch { path: payload_path });
    }

    if let Some(expected_target_identity) = &commit.backup_identity {
        verify_expected_backup(&commit, expected_target_identity, proof).await?;
    } else if target_exists {
        if record.request.conflict_strategy == crate::TransferConflictStrategy::KeepBoth {
            commit.prepared.resolved_target = first_available_entry_path(
                &record.request.requested_target,
                EntryNameKind::from(commit.prepared.source_identity.object_kind),
            )
            .await?;
            return Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::CommitIntent(commit),
            ));
        }
        return Err(RecoverableTransferError::TargetConflict {
            path: commit.prepared.resolved_target,
        });
    }

    match rename_noreplace(&payload_path, &commit.prepared.resolved_target) {
        Ok(()) => {}
        Err(NoReplaceRenameError::CrossDevice)
            if matches!(commit.payload, CommitPayload::DirectSource { .. }) =>
        {
            let mut prepared = commit.prepared;
            prepared.execution = TransferExecutionKind::MoveToStage;
            prepared.staging_plan = Some(plan_owned_artifact(
                target_parent(&prepared.resolved_target)?,
                OwnedArtifactKind::TargetStaging,
                record.owner(0),
            )?);
            return Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::StageCreationIntent(prepared),
            ));
        }
        Err(NoReplaceRenameError::TargetExists)
            if record.request.conflict_strategy == crate::TransferConflictStrategy::KeepBoth =>
        {
            commit.prepared.resolved_target = first_available_entry_path(
                &record.request.requested_target,
                EntryNameKind::from(commit.prepared.source_identity.object_kind),
            )
            .await?;
            return Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::CommitIntent(commit),
            ));
        }
        Err(error) => {
            return Err(error.into_transfer_error(&payload_path, &commit.prepared.resolved_target));
        }
    }
    barrier.require_rename(&payload_path, &commit.prepared.resolved_target);
    let committed = committed_transfer(record, &commit, proof).await?;
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::TargetCommitted(committed),
    ))
}

async fn verify_expected_backup(
    commit: &CommitTransfer,
    expected_backup_identity: &FileIdentity,
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    let artifact = commit_artifact(commit)?;
    let backup_path = artifact.plan.backup_path();
    let expected_fingerprint = commit.prepared.expected_target_fingerprint.ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "replacement commit has no backup fingerprint".to_owned(),
        }
    })?;
    let backup_identity = inspect_file_identity(&backup_path).await?;
    if backup_identity != *expected_backup_identity
        || proof.fingerprint_object(&backup_path).await? != expected_fingerprint
    {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }
    Ok(())
}

pub(super) async fn create_replace_backup(
    mut backup: BackupCreationTransfer,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    let artifact = match &backup.payload {
        CommitPayload::Artifact { artifact, .. } => artifact,
        CommitPayload::DirectSource { .. } => {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "replacement backup creation requires an owned artifact".to_owned(),
            });
        }
    };
    validate_owned_artifact(artifact).await?;
    let payload_path = match &backup.payload {
        CommitPayload::Artifact { artifact, .. } => artifact.plan.payload_path(),
        CommitPayload::DirectSource { .. } => unreachable!(),
    };
    let payload_identity = inspect_file_identity(&payload_path).await?;
    if payload_identity != *backup_payload_identity(&backup)
        || proof.fingerprint_object(&payload_path).await? != backup.fingerprint
    {
        return Err(RecoverableTransferError::FingerprintMismatch { path: payload_path });
    }

    let expected_target_identity = backup
        .prepared
        .expected_target_identity
        .as_ref()
        .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "backup creation has no expected target identity".to_owned(),
        })?;
    let backup_path = artifact.plan.backup_path();
    let target_exists = path_exists(&backup.prepared.resolved_target).await?;
    let backup_exists = path_exists(&backup_path).await?;
    match (target_exists, backup_exists) {
        (true, false) => {
            let current_target = inspect_file_identity(&backup.prepared.resolved_target).await?;
            if current_target != *expected_target_identity {
                return Err(RecoverableTransferError::TargetConflict {
                    path: backup.prepared.resolved_target,
                });
            }
            if let Some(expected_fingerprint) = backup.prepared.expected_target_fingerprint {
                if proof
                    .fingerprint_object(&backup.prepared.resolved_target)
                    .await?
                    != expected_fingerprint
                {
                    return Err(RecoverableTransferError::TargetConflict {
                        path: backup.prepared.resolved_target,
                    });
                }
            }
            rename_noreplace(&backup.prepared.resolved_target, &backup_path).map_err(|error| {
                error.into_transfer_error(&backup.prepared.resolved_target, &backup_path)
            })?;
            barrier.require_rename(&backup.prepared.resolved_target, &backup_path);
        }
        (false, true) => {}
        _ => {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "replacement target and backup slots are ambiguous".to_owned(),
            });
        }
    }

    let backup_identity = inspect_file_identity(&backup_path).await?;
    if !backup_identity.same_object(expected_target_identity)
        || !backup_identity.matches_staging_snapshot(expected_target_identity)
    {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }
    let backup_fingerprint = proof.fingerprint_object(&backup_path).await?;
    if inspect_file_identity(&backup_path).await? != backup_identity {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }
    if let Some(expected_fingerprint) = backup.prepared.expected_target_fingerprint {
        if backup_fingerprint != expected_fingerprint {
            return Err(RecoverableTransferError::TargetConflict { path: backup_path });
        }
    }
    backup.prepared.expected_target_fingerprint = Some(backup_fingerprint);
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::CommitIntent(CommitTransfer {
            prepared: backup.prepared,
            payload: backup.payload,
            fingerprint: TransferFingerprint::Blake3(backup.fingerprint),
            backup_identity: Some(backup_identity),
            metadata_loss: backup.metadata_loss,
        }),
    ))
}

fn backup_payload_identity(backup: &BackupCreationTransfer) -> &FileIdentity {
    match &backup.payload {
        CommitPayload::Artifact {
            payload_identity, ..
        } => payload_identity,
        CommitPayload::DirectSource { identity } => identity,
    }
}

pub(super) fn commit_payload_identity(commit: &CommitTransfer) -> &FileIdentity {
    match &commit.payload {
        CommitPayload::DirectSource { identity } => identity,
        CommitPayload::Artifact {
            payload_identity, ..
        } => payload_identity,
    }
}

async fn persist_inferred_commit(
    record: &TransferJournalRecord,
    commit: CommitTransfer,
    proof: &ProofContext,
) -> Result<TransferTransition, RecoverableTransferError> {
    let target_identity = inspect_file_identity(&commit.prepared.resolved_target).await?;
    if !matches_object_verification(
        &commit.prepared.resolved_target,
        &target_identity,
        commit_payload_identity(&commit),
        &commit.fingerprint,
        proof,
    )
    .await?
    {
        return Err(RecoverableTransferError::TargetConflict {
            path: commit.prepared.resolved_target,
        });
    }
    let committed = committed_transfer(record, &commit, proof).await?;
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::TargetCommitted(committed),
    ))
}

async fn committed_transfer(
    record: &TransferJournalRecord,
    commit: &CommitTransfer,
    proof: &ProofContext,
) -> Result<CommittedTransfer, RecoverableTransferError> {
    let target_identity = inspect_file_identity(&commit.prepared.resolved_target).await?;
    if !matches_object_verification(
        &commit.prepared.resolved_target,
        &target_identity,
        commit_payload_identity(commit),
        &commit.fingerprint,
        proof,
    )
    .await?
    {
        return Err(RecoverableTransferError::FingerprintMismatch {
            path: commit.prepared.resolved_target.clone(),
        });
    }
    let (artifact, staged_source_location) = match &commit.payload {
        CommitPayload::DirectSource { .. } => (None, None),
        CommitPayload::Artifact {
            artifact,
            source_location,
            ..
        } => (Some(artifact.clone()), Some(*source_location)),
    };
    let source_disposition = match record.request.operation {
        RecoverableTransferOperation::Copy => SourceDisposition::Preserved,
        RecoverableTransferOperation::Move
            if staged_source_location == Some(StagedSourceLocation::OriginalPath) =>
        {
            SourceDisposition::RequiresRetirement
        }
        RecoverableTransferOperation::Move => SourceDisposition::MovedByCommit,
    };
    Ok(CommittedTransfer {
        final_target: commit.prepared.resolved_target.clone(),
        target_identity,
        fingerprint: commit.fingerprint.clone(),
        artifact,
        source_disposition,
        backup_identity: commit.backup_identity.clone(),
        backup_fingerprint: commit.prepared.expected_target_fingerprint,
        backup_cleanup_index: 0,
        backup_cleanup_intent: None,
        metadata_loss: commit.metadata_loss.clone(),
    })
}

pub(super) async fn advance_committed_transfer(
    record: &TransferJournalRecord,
    mut committed: CommittedTransfer,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    if let Err(target_error) = verify_committed_target(&committed, proof).await {
        if let Some(recovered_backup) =
            recover_complete_backup_after_target_conflict(record, &committed, proof, barrier)
                .await?
        {
            return Err(RecoverableTransferError::RecoveryBlocked {
                diagnostic: format!(
                    "committed target verification failed: {target_error}; previous target recovered to {}",
                    recovered_backup.display()
                ),
            });
        }
        return Err(target_error);
    }
    if record.replacement_manifest.is_some() && committed.backup_identity.is_none() {
        let artifact = committed.artifact.as_ref().ok_or_else(|| {
            RecoverableTransferError::InvalidCheckpoint {
                message: "replacement checkpoint has no owned artifact".to_owned(),
            }
        })?;
        let backup_path = artifact.plan.backup_path();
        if path_exists(&backup_path).await? {
            let expected_fingerprint = committed.backup_fingerprint.ok_or_else(|| {
                RecoverableTransferError::InvalidCheckpoint {
                    message: "replacement checkpoint has no backup fingerprint".to_owned(),
                }
            })?;
            let current = inspect_file_identity(&backup_path).await?;
            if !matches_object_verification(
                &backup_path,
                &current,
                manifest_root_identity(record_replacement_manifest(record)?)?,
                &TransferFingerprint::Blake3(expected_fingerprint),
                proof,
            )
            .await?
            {
                return Err(RecoverableTransferError::TargetConflict { path: backup_path });
            }
            committed.backup_identity = Some(current);
            return Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::TargetCommitted(committed),
            ));
        }
        if committed.backup_cleanup_index < record_replacement_manifest(record)?.entries.len() {
            return Err(RecoverableTransferError::SourceChanged { path: backup_path });
        }
    }
    if let Some(artifact) = committed.artifact.clone() {
        if record.replacement_manifest.is_some() {
            let backup_fingerprint = committed.backup_fingerprint.ok_or_else(|| {
                RecoverableTransferError::InvalidCheckpoint {
                    message: "replacement checkpoint has no backup fingerprint".to_owned(),
                }
            })?;
            let manifest_entries = &record_replacement_manifest(record)?.entries;
            if committed.backup_cleanup_index > manifest_entries.len() {
                return Err(RecoverableTransferError::InvalidCheckpoint {
                    message: "replacement backup cleanup cursor exceeds the manifest".to_owned(),
                });
            }
            let current_entry =
                reverse_manifest_entry(manifest_entries, committed.backup_cleanup_index);
            if let Some(intent) = committed.backup_cleanup_intent.clone() {
                let expected = current_entry.as_ref().ok_or_else(|| {
                    RecoverableTransferError::InvalidCheckpoint {
                        message: "replacement backup cleanup intent has no manifest entry"
                            .to_owned(),
                    }
                })?;
                if &intent.entry != expected {
                    return Err(RecoverableTransferError::InvalidCheckpoint {
                        message: "replacement backup cleanup intent does not match the manifest"
                            .to_owned(),
                    });
                }
                match advance_owned_tree_entry_deletion(
                    &artifact,
                    &artifact.plan.backup_path(),
                    &artifact.plan.payload_path(),
                    &committed.final_target,
                    &intent,
                    proof,
                    barrier,
                )
                .await?
                {
                    OwnedTreeEntryDeletionAdvance::Persist(updated) => {
                        committed.backup_cleanup_intent = Some(*updated);
                    }
                    OwnedTreeEntryDeletionAdvance::Complete => {
                        committed.backup_cleanup_index += 1;
                        committed.backup_cleanup_intent = None;
                    }
                }
                return Ok(TransferTransition::Checkpoint(
                    TransferCheckpoint::TargetCommitted(committed),
                ));
            }
            if let Some(entry) = current_entry.as_ref() {
                committed.backup_cleanup_intent = Some(
                    prepare_owned_tree_entry_deletion_intent(
                        &artifact,
                        &artifact.plan.backup_path(),
                        entry,
                        committed.backup_identity.as_ref(),
                        backup_fingerprint,
                        proof,
                    )
                    .await?,
                );
                return Ok(TransferTransition::Checkpoint(
                    TransferCheckpoint::TargetCommitted(committed),
                ));
            }
        } else if committed.backup_fingerprint.is_some() {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "non-replacement checkpoint has a backup fingerprint".to_owned(),
            });
        }
        if path_exists(&artifact.plan.root).await? {
            remove_empty_owned_artifact(&artifact, barrier).await?;
        }
        committed.artifact = None;
    }
    match committed.source_disposition {
        SourceDisposition::RequiresRetirement => {
            let source_parent = target_parent(&record.request.source)?;
            let artifact_plan = plan_owned_artifact(
                source_parent,
                OwnedArtifactKind::SourceRetirement,
                record.owner(1),
            )?;
            Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::SourceRetirementIntent(Box::new(SourceRetirementPlan {
                    committed,
                    artifact_plan,
                    artifact: None,
                })),
            ))
        }
        SourceDisposition::Preserved | SourceDisposition::MovedByCommit => {
            let completed = completed_target(&committed);
            Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::Completed(completed),
            ))
        }
    }
}

async fn recover_complete_backup_after_target_conflict(
    record: &TransferJournalRecord,
    committed: &CommittedTransfer,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<Option<std::path::PathBuf>, RecoverableTransferError> {
    let Some(artifact) = &committed.artifact else {
        return Ok(None);
    };
    if record.replacement_manifest.is_none() {
        return Ok(None);
    }
    let expected_fingerprint = committed.backup_fingerprint.ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "replacement checkpoint has no backup fingerprint".to_owned(),
        }
    })?;
    let expected_identity = committed.backup_identity.as_ref().ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "replacement checkpoint has no backup identity".to_owned(),
        }
    })?;
    let backup_path = artifact.plan.backup_path();
    if !path_exists(&backup_path).await? {
        return Ok(None);
    }
    validate_owned_artifact(artifact).await?;
    let backup_identity = inspect_file_identity(&backup_path).await?;
    if backup_identity != *expected_identity
        || proof.fingerprint_object(&backup_path).await? != expected_fingerprint
    {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }

    let recovered_path = next_recovered_path(&committed.final_target).await?;
    rename_noreplace(&backup_path, &recovered_path)
        .map_err(|error| error.into_transfer_error(&backup_path, &recovered_path))?;
    barrier.require_rename(&backup_path, &recovered_path);
    Ok(Some(recovered_path))
}

pub(super) fn completed_target(committed: &CommittedTransfer) -> CompletedTarget {
    CompletedTarget {
        path: committed.final_target.clone(),
        identity: committed.target_identity.clone(),
        fingerprint: committed.fingerprint.clone(),
        metadata_loss: committed.metadata_loss.clone(),
    }
}

pub(super) async fn verify_completed_target(
    completed: &CompletedTarget,
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    let current_identity = inspect_file_identity(&completed.path).await?;
    if current_identity != completed.identity
        || !proof
            .matches(&completed.path, &completed.fingerprint)
            .await?
    {
        return Err(RecoverableTransferError::TargetConflict {
            path: completed.path.clone(),
        });
    }
    Ok(())
}

pub(super) async fn verify_committed_target(
    committed: &CommittedTransfer,
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    let current_identity = inspect_file_identity(&committed.final_target).await?;
    if current_identity != committed.target_identity
        || !proof
            .matches(&committed.final_target, &committed.fingerprint)
            .await?
    {
        return Err(RecoverableTransferError::TargetConflict {
            path: committed.final_target.clone(),
        });
    }
    Ok(())
}

pub(super) async fn verify_prepared_source(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    let controls = FileOperationControls::running(CancellationToken::new());
    verify_prepared_source_with_controls(record, prepared, &controls).await
}

pub(super) async fn verify_prepared_source_with_controls(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
    controls: &FileOperationControls,
) -> Result<(), RecoverableTransferError> {
    let manifest = record_manifest(record)?;
    let mut manifest_controls = controls.clone();
    verify_source_manifest_with_controls(manifest, &mut manifest_controls).await?;
    let source_identity = inspect_file_identity(&record.request.source).await?;
    if source_identity != prepared.source_identity {
        return Err(RecoverableTransferError::SourceChanged {
            path: record.request.source.clone(),
        });
    }
    match prepared.source_fingerprint {
        Some(expected)
            if fingerprint_object_with_controls(&record.request.source, controls).await?
                == expected => {}
        Some(_) => {
            return Err(RecoverableTransferError::SourceChanged {
                path: record.request.source.clone(),
            });
        }
        None if record.request.verification == crate::FileOperationVerification::BasicMetadata => {}
        None => {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "strong prepared checkpoint has no source fingerprint".to_owned(),
            });
        }
    }
    Ok(())
}

pub(super) async fn matches_object_verification(
    path: &Path,
    current_identity: &FileIdentity,
    expected_identity: &FileIdentity,
    expected_fingerprint: &TransferFingerprint,
    proof: &ProofContext,
) -> Result<bool, RecoverableTransferError> {
    if !current_identity.same_object(expected_identity) {
        return Ok(false);
    }
    proof.matches(path, expected_fingerprint).await
}
