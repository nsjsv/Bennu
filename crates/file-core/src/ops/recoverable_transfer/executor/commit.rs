use std::path::Path;

use tokio::fs;
use tokio_util::sync::CancellationToken;

use super::{
    commit_artifact, commit_payload_path, manifest_root_identity, next_recovered_path, path_exists,
    persist_checkpoint, record_manifest, record_replacement_manifest, sync_rename_parents,
    target_parent,
};
use crate::ops::recoverable_transfer::{
    fingerprint_object, fingerprint_object_with_controls, inspect_file_identity,
    plan_owned_artifact, recover_owned_artifact, remove_empty_owned_artifact, rename_noreplace,
    validate_owned_artifact, verify_source_manifest_with_controls, BackupCreationTransfer,
    CommitPayload, CommitTransfer, CommittedTransfer, CompletedTarget, FileIdentity,
    FileObjectKind, NoReplaceRenameError, OwnedArtifact, OwnedArtifactKind,
    OwnedTreeEntryDeletionIntent, PreparedTransfer, RecoverableTransferError,
    RecoverableTransferOperation, RetiredSource, SourceDisposition, SourceManifestEntry,
    SourceRetirementPlan, StagedSourceLocation, TransferCheckpoint, TransferExecutionKind,
    TransferJournal, TransferJournalRecord,
};
use crate::ops::FileOperationControls;
use crate::transfer_conflict::available_transfer_target_path_candidate;

pub(super) async fn commit_transfer<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    mut commit: CommitTransfer,
) -> Result<(), RecoverableTransferError> {
    if commit.prepared.expected_target_identity.is_some() && commit.backup_identity.is_none() {
        if record.request.verification != crate::FileOperationVerification::Strong
            || commit.prepared.expected_target_fingerprint.is_none()
        {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "replacement commit has no verified backup identity".to_owned(),
            });
        }
        return persist_checkpoint(
            record,
            journal,
            TransferCheckpoint::BackupCreationIntent(BackupCreationTransfer {
                prepared: commit.prepared,
                payload: commit.payload,
                fingerprint: commit.fingerprint,
            }),
        )
        .await;
    }

    let payload_path = commit_payload_path(record, &commit);
    let payload_exists = path_exists(&payload_path).await?;
    let target_exists = path_exists(&commit.prepared.resolved_target).await?;

    if target_exists && !payload_exists {
        return persist_inferred_commit(record, journal, commit).await;
    }
    if !payload_exists {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    let payload_identity = inspect_file_identity(&payload_path).await?;
    if payload_identity != *commit_payload_identity(&commit)
        || fingerprint_object(&payload_path).await? != commit.fingerprint
    {
        return Err(RecoverableTransferError::FingerprintMismatch { path: payload_path });
    }

    if let Some(expected_target_identity) = &commit.backup_identity {
        verify_expected_backup(&commit, expected_target_identity).await?;
    } else if target_exists {
        if record.request.conflict_strategy == crate::TransferConflictStrategy::KeepBoth {
            commit.prepared.resolved_target =
                available_transfer_target_path_candidate(&record.request.requested_target)
                    .await
                    .map_err(|source| {
                        RecoverableTransferError::file_system(
                            "select keep-both target for",
                            &record.request.requested_target,
                            source,
                        )
                    })?;
            persist_checkpoint(record, journal, TransferCheckpoint::CommitIntent(commit)).await?;
            return Ok(());
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
            persist_checkpoint(
                record,
                journal,
                TransferCheckpoint::StageCreationIntent(prepared),
            )
            .await?;
            return Ok(());
        }
        Err(NoReplaceRenameError::TargetExists)
            if record.request.conflict_strategy == crate::TransferConflictStrategy::KeepBoth =>
        {
            commit.prepared.resolved_target =
                available_transfer_target_path_candidate(&record.request.requested_target)
                    .await
                    .map_err(|source| {
                        RecoverableTransferError::file_system(
                            "select keep-both target for",
                            &record.request.requested_target,
                            source,
                        )
                    })?;
            persist_checkpoint(record, journal, TransferCheckpoint::CommitIntent(commit)).await?;
            return Ok(());
        }
        Err(error) => {
            return Err(error.into_transfer_error(&payload_path, &commit.prepared.resolved_target));
        }
    }
    sync_rename_parents(&payload_path, &commit.prepared.resolved_target).await?;
    let committed = committed_transfer(record, &commit).await?;
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::TargetCommitted(committed),
    )
    .await
}

async fn verify_expected_backup(
    commit: &CommitTransfer,
    expected_backup_identity: &FileIdentity,
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
        || fingerprint_object(&backup_path).await? != expected_fingerprint
    {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }
    Ok(())
}

pub(super) async fn create_replace_backup<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    mut backup: BackupCreationTransfer,
) -> Result<(), RecoverableTransferError> {
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
        || fingerprint_object(&payload_path).await? != backup.fingerprint
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
                if fingerprint_object(&backup.prepared.resolved_target).await?
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
            sync_rename_parents(&backup.prepared.resolved_target, &backup_path).await?;
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
    let backup_fingerprint = fingerprint_object(&backup_path).await?;
    if inspect_file_identity(&backup_path).await? != backup_identity {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }
    if let Some(expected_fingerprint) = backup.prepared.expected_target_fingerprint {
        if backup_fingerprint != expected_fingerprint {
            return Err(RecoverableTransferError::TargetConflict { path: backup_path });
        }
    }
    backup.prepared.expected_target_fingerprint = Some(backup_fingerprint);
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::CommitIntent(CommitTransfer {
            prepared: backup.prepared,
            payload: backup.payload,
            fingerprint: backup.fingerprint,
            backup_identity: Some(backup_identity),
        }),
    )
    .await
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

async fn persist_inferred_commit<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    commit: CommitTransfer,
) -> Result<(), RecoverableTransferError> {
    let target_identity = inspect_file_identity(&commit.prepared.resolved_target).await?;
    if !matches_object_verification(
        &commit.prepared.resolved_target,
        &target_identity,
        commit_payload_identity(&commit),
        commit.fingerprint,
    )
    .await?
    {
        return Err(RecoverableTransferError::TargetConflict {
            path: commit.prepared.resolved_target,
        });
    }
    let committed = committed_transfer(record, &commit).await?;
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::TargetCommitted(committed),
    )
    .await
}

async fn committed_transfer(
    record: &TransferJournalRecord,
    commit: &CommitTransfer,
) -> Result<CommittedTransfer, RecoverableTransferError> {
    let target_identity = inspect_file_identity(&commit.prepared.resolved_target).await?;
    if !matches_object_verification(
        &commit.prepared.resolved_target,
        &target_identity,
        commit_payload_identity(commit),
        commit.fingerprint,
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
        fingerprint: commit.fingerprint,
        artifact,
        source_disposition,
        backup_identity: commit.backup_identity.clone(),
        backup_fingerprint: commit.prepared.expected_target_fingerprint,
        backup_cleanup_index: 0,
        backup_cleanup_intent: None,
    })
}

pub(super) async fn advance_committed_transfer<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    mut committed: CommittedTransfer,
) -> Result<(), RecoverableTransferError> {
    if let Err(target_error) = verify_committed_target(&committed).await {
        if let Some(recovered_backup) =
            recover_complete_backup_after_target_conflict(record, &committed).await?
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
                expected_fingerprint,
            )
            .await?
            {
                return Err(RecoverableTransferError::TargetConflict { path: backup_path });
            }
            committed.backup_identity = Some(current);
            return persist_checkpoint(
                record,
                journal,
                TransferCheckpoint::TargetCommitted(committed),
            )
            .await;
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
                return persist_checkpoint(
                    record,
                    journal,
                    TransferCheckpoint::TargetCommitted(committed),
                )
                .await;
            }
            if let Some(entry) = current_entry.as_ref() {
                committed.backup_cleanup_intent = Some(
                    prepare_owned_tree_entry_deletion_intent(
                        &artifact,
                        &artifact.plan.backup_path(),
                        entry,
                        committed.backup_identity.as_ref(),
                        backup_fingerprint,
                    )
                    .await?,
                );
                return persist_checkpoint(
                    record,
                    journal,
                    TransferCheckpoint::TargetCommitted(committed),
                )
                .await;
            }
        } else if committed.backup_fingerprint.is_some() {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "non-replacement checkpoint has a backup fingerprint".to_owned(),
            });
        }
        if path_exists(&artifact.plan.root).await? {
            remove_empty_owned_artifact(&artifact).await?;
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
            persist_checkpoint(
                record,
                journal,
                TransferCheckpoint::SourceRetirementIntent(Box::new(SourceRetirementPlan {
                    committed,
                    artifact_plan,
                    artifact: None,
                })),
            )
            .await
        }
        SourceDisposition::Preserved | SourceDisposition::MovedByCommit => {
            let completed = completed_target(&committed);
            persist_checkpoint(record, journal, TransferCheckpoint::Completed(completed)).await
        }
    }
}

async fn recover_complete_backup_after_target_conflict(
    record: &TransferJournalRecord,
    committed: &CommittedTransfer,
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
        || fingerprint_object(&backup_path).await? != expected_fingerprint
    {
        return Err(RecoverableTransferError::TargetConflict { path: backup_path });
    }

    let recovered_path = next_recovered_path(&committed.final_target).await?;
    rename_noreplace(&backup_path, &recovered_path)
        .map_err(|error| error.into_transfer_error(&backup_path, &recovered_path))?;
    sync_rename_parents(&backup_path, &recovered_path).await?;
    Ok(Some(recovered_path))
}

pub(super) async fn retire_source<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    retirement: SourceRetirementPlan,
) -> Result<(), RecoverableTransferError> {
    verify_committed_target(&retirement.committed).await?;
    let Some(artifact) = retirement.artifact.clone() else {
        let artifact = recover_owned_artifact(retirement.artifact_plan.clone()).await?;
        let mut identified_retirement = retirement;
        identified_retirement.artifact = Some(artifact);
        return persist_checkpoint(
            record,
            journal,
            TransferCheckpoint::SourceRetirementIntent(Box::new(identified_retirement)),
        )
        .await;
    };
    crate::ops::recoverable_transfer::validate_owned_artifact(&artifact).await?;
    let prepared = PreparedTransfer {
        source_identity: manifest_root_identity(record_manifest(record)?)?.clone(),
        resolved_target: retirement.committed.final_target.clone(),
        expected_target_identity: None,
        expected_target_fingerprint: None,
        source_fingerprint: Some(retirement.committed.fingerprint),
        execution: TransferExecutionKind::MoveToStage,
        staging_plan: None,
    };
    let payload_path = artifact.plan.payload_path();
    let source_exists = path_exists(&record.request.source).await?;
    let payload_exists = path_exists(&payload_path).await?;
    match (source_exists, payload_exists) {
        (true, false) => {
            verify_prepared_source(record, &prepared).await?;
            rename_noreplace(&record.request.source, &payload_path).map_err(|error| {
                error.into_transfer_error(&record.request.source, &payload_path)
            })?;
            sync_rename_parents(&record.request.source, &payload_path).await?;
        }
        (false, true) => {}
        _ => {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "source retirement paths are ambiguous".to_owned(),
            });
        }
    }
    let payload_identity = inspect_file_identity(&payload_path).await?;
    if !matches_object_verification(
        &payload_path,
        &payload_identity,
        &prepared.source_identity,
        retirement.committed.fingerprint,
    )
    .await?
    {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::SourceRetired(Box::new(RetiredSource {
            committed: retirement.committed,
            artifact,
            payload_identity: Some(payload_identity),
            cleanup_index: 0,
            cleanup_intent: None,
        })),
    )
    .await
}

pub(super) async fn advance_retired_source<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    mut retired: RetiredSource,
) -> Result<(), RecoverableTransferError> {
    verify_committed_target(&retired.committed).await?;
    if retired.payload_identity.is_none() {
        let payload_path = retired.artifact.plan.payload_path();
        let current = inspect_file_identity(&payload_path).await?;
        let expected = manifest_root_identity(record_manifest(record)?)?;
        if !matches_object_verification(
            &payload_path,
            &current,
            expected,
            retired.committed.fingerprint,
        )
        .await?
        {
            return Err(RecoverableTransferError::SourceChanged { path: payload_path });
        }
        retired.payload_identity = Some(current);
        return persist_checkpoint(
            record,
            journal,
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        )
        .await;
    }
    let manifest_entries = &record_manifest(record)?.entries;
    if retired.cleanup_index > manifest_entries.len() {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "source retirement cleanup cursor exceeds the manifest".to_owned(),
        });
    }
    let current_entry = reverse_manifest_entry(manifest_entries, retired.cleanup_index);

    if let Some(intent) = retired.cleanup_intent.clone() {
        let expected =
            current_entry
                .as_ref()
                .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
                    message: "source retirement cleanup intent has no manifest entry".to_owned(),
                })?;
        if &intent.entry != expected {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "source retirement cleanup intent does not match the manifest".to_owned(),
            });
        }
        match advance_owned_tree_entry_deletion(
            &retired.artifact,
            &retired.artifact.plan.payload_path(),
            &retired.artifact.plan.backup_path(),
            &record.request.source,
            &intent,
        )
        .await?
        {
            OwnedTreeEntryDeletionAdvance::Persist(updated) => {
                retired.cleanup_intent = Some(*updated);
            }
            OwnedTreeEntryDeletionAdvance::Complete => {
                retired.cleanup_index += 1;
                retired.cleanup_intent = None;
            }
        }
        return persist_checkpoint(
            record,
            journal,
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        )
        .await;
    }

    if let Some(entry) = current_entry.as_ref() {
        retired.cleanup_intent = Some(
            prepare_owned_tree_entry_deletion_intent(
                &retired.artifact,
                &retired.artifact.plan.payload_path(),
                entry,
                retired.payload_identity.as_ref(),
                retired.committed.fingerprint,
            )
            .await?,
        );
        return persist_checkpoint(
            record,
            journal,
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        )
        .await;
    }

    remove_empty_owned_artifact(&retired.artifact).await?;
    let completed = completed_target(&retired.committed);
    persist_checkpoint(record, journal, TransferCheckpoint::Completed(completed)).await
}

fn reverse_manifest_entry(
    entries: &[SourceManifestEntry],
    completed_entries: usize,
) -> Option<SourceManifestEntry> {
    completed_entries
        .checked_add(1)
        .and_then(|offset| entries.len().checked_sub(offset))
        .and_then(|index| entries.get(index))
        .cloned()
}

async fn prepare_owned_tree_entry_deletion_intent(
    artifact: &OwnedArtifact,
    tree_root: &Path,
    entry: &SourceManifestEntry,
    root_identity: Option<&FileIdentity>,
    root_fingerprint: crate::ops::recoverable_transfer::ObjectFingerprint,
) -> Result<OwnedTreeEntryDeletionIntent, RecoverableTransferError> {
    crate::ops::recoverable_transfer::validate_owned_artifact(artifact).await?;
    let entry_path = owned_tree_entry_path(tree_root, entry);
    let entry_position = if entry.relative_path.as_os_str().is_empty() {
        ManifestEntryPosition::Root
    } else {
        ManifestEntryPosition::Descendant
    };
    let (fingerprint, expected_identity) = if path_exists(&entry_path).await? {
        let expected_identity = if matches!(entry_position, ManifestEntryPosition::Root) {
            root_identity.unwrap_or(&entry.identity)
        } else {
            &entry.identity
        };
        verify_retirement_entry_before_intent(
            &entry_path,
            expected_identity,
            entry_position,
            root_fingerprint,
        )
        .await?;
        let current = inspect_file_identity(&entry_path).await?;
        let fingerprint = fingerprint_object(&entry_path).await?;
        if inspect_file_identity(&entry_path).await? != current {
            return Err(RecoverableTransferError::SourceChanged { path: entry_path });
        }
        (Some(fingerprint), Some(current))
    } else {
        (None, None)
    };
    Ok(OwnedTreeEntryDeletionIntent {
        entry: entry.clone(),
        fingerprint,
        expected_identity,
        deletion_slot_identity: None,
    })
}

#[derive(Debug)]
enum OwnedTreeEntryDeletionAdvance {
    Persist(Box<OwnedTreeEntryDeletionIntent>),
    Complete,
}

async fn advance_owned_tree_entry_deletion(
    artifact: &OwnedArtifact,
    tree_root: &Path,
    deletion_slot: &Path,
    recovered_base: &Path,
    intent: &OwnedTreeEntryDeletionIntent,
) -> Result<OwnedTreeEntryDeletionAdvance, RecoverableTransferError> {
    crate::ops::recoverable_transfer::validate_owned_artifact(artifact).await?;
    let entry_path = owned_tree_entry_path(tree_root, &intent.entry);
    let entry_exists = path_exists(&entry_path).await?;
    let slot_exists = path_exists(deletion_slot).await?;

    match (entry_exists, slot_exists) {
        (true, false) => {
            if intent.deletion_slot_identity.is_some() {
                return Err(RecoverableTransferError::InvalidCheckpoint {
                    message: "verified deletion slot disappeared while source entry exists"
                        .to_owned(),
                });
            }
            if intent.expected_identity.is_none() {
                verify_legacy_deletion_entry(&entry_path, intent).await?;
                let mut upgraded = intent.clone();
                upgraded.expected_identity = Some(inspect_file_identity(&entry_path).await?);
                return Ok(OwnedTreeEntryDeletionAdvance::Persist(Box::new(upgraded)));
            }
            verify_deletion_entry_exact(
                &entry_path,
                intent.expected_identity.as_ref().unwrap(),
                intent.fingerprint,
            )
            .await?;
            rename_noreplace(&entry_path, deletion_slot)
                .map_err(|error| error.into_transfer_error(&entry_path, deletion_slot))?;
            sync_rename_parents(&entry_path, deletion_slot).await?;

            let slot_identity = inspect_file_identity(deletion_slot).await?;
            verify_deletion_entry_exact(deletion_slot, &slot_identity, intent.fingerprint).await?;
            let mut moved = intent.clone();
            moved.deletion_slot_identity = Some(slot_identity);
            Ok(OwnedTreeEntryDeletionAdvance::Persist(Box::new(moved)))
        }
        (false, true) => {
            if intent.deletion_slot_identity.is_none() {
                if let Err(error) = verify_legacy_deletion_entry(deletion_slot, intent).await {
                    restore_unverified_deletion_slot(
                        &entry_path,
                        deletion_slot,
                        recovered_base,
                        intent,
                    )
                    .await?;
                    return Err(error);
                }
                let mut upgraded = intent.clone();
                upgraded.deletion_slot_identity = Some(inspect_file_identity(deletion_slot).await?);
                return Ok(OwnedTreeEntryDeletionAdvance::Persist(Box::new(upgraded)));
            }
            if let Err(error) = verify_deletion_entry_exact(
                deletion_slot,
                intent.deletion_slot_identity.as_ref().unwrap(),
                intent.fingerprint,
            )
            .await
            {
                restore_unverified_deletion_slot(
                    &entry_path,
                    deletion_slot,
                    recovered_base,
                    intent,
                )
                .await?;
                return Err(error);
            }
            remove_retirement_slot(deletion_slot).await?;
            sync_removed_entry_parent(deletion_slot).await?;
            Ok(OwnedTreeEntryDeletionAdvance::Complete)
        }
        (false, false) => Ok(OwnedTreeEntryDeletionAdvance::Complete),
        (true, true) => Err(RecoverableTransferError::InvalidCheckpoint {
            message: "source retirement entry and deletion slot both exist".to_owned(),
        }),
    }
}

async fn restore_unverified_deletion_slot(
    entry_path: &Path,
    deletion_slot: &Path,
    recovered_base: &Path,
    intent: &OwnedTreeEntryDeletionIntent,
) -> Result<(), RecoverableTransferError> {
    let slot_identity = inspect_file_identity(deletion_slot).await?;
    let expected = intent
        .deletion_slot_identity
        .as_ref()
        .or(intent.expected_identity.as_ref())
        .unwrap_or(&intent.entry.identity);
    if slot_identity.same_object(expected) {
        restore_unverified_owned_tree_entry(entry_path, deletion_slot, recovered_base).await?;
    }
    Ok(())
}

async fn verify_legacy_deletion_entry(
    path: &Path,
    intent: &OwnedTreeEntryDeletionIntent,
) -> Result<(), RecoverableTransferError> {
    let expected = intent
        .expected_identity
        .as_ref()
        .unwrap_or(&intent.entry.identity);
    let fingerprint =
        intent
            .fingerprint
            .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
                message: "existing deletion entry has no content fingerprint".to_owned(),
            })?;
    let current = inspect_file_identity(path).await?;
    if current.same_object(expected) && fingerprint_object(path).await? == fingerprint {
        Ok(())
    } else {
        Err(RecoverableTransferError::SourceChanged {
            path: path.to_path_buf(),
        })
    }
}

async fn verify_deletion_entry_exact(
    path: &Path,
    expected_identity: &FileIdentity,
    expected_fingerprint: Option<crate::ops::recoverable_transfer::ObjectFingerprint>,
) -> Result<(), RecoverableTransferError> {
    let expected_fingerprint =
        expected_fingerprint.ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "existing deletion entry has no content fingerprint".to_owned(),
        })?;
    let before = inspect_file_identity(path).await?;
    let fingerprint = fingerprint_object(path).await?;
    let after = inspect_file_identity(path).await?;
    if before == *expected_identity && after == before && fingerprint == expected_fingerprint {
        Ok(())
    } else {
        Err(RecoverableTransferError::SourceChanged {
            path: path.to_path_buf(),
        })
    }
}

fn owned_tree_entry_path(tree_root: &Path, entry: &SourceManifestEntry) -> std::path::PathBuf {
    if entry.relative_path.as_os_str().is_empty() {
        tree_root.to_path_buf()
    } else {
        tree_root.join(&entry.relative_path)
    }
}

#[derive(Clone, Copy)]
enum ManifestEntryPosition {
    Root,
    Descendant,
}

async fn verify_retirement_entry_before_intent(
    path: &Path,
    expected: &FileIdentity,
    position: ManifestEntryPosition,
    root_fingerprint: crate::ops::recoverable_transfer::ObjectFingerprint,
) -> Result<(), RecoverableTransferError> {
    let current = inspect_file_identity(path).await?;
    let matches = match expected.object_kind {
        FileObjectKind::Directory => {
            if current.object_kind != FileObjectKind::Directory || !current.same_object(expected) {
                false
            } else {
                let mut entries = fs::read_dir(path).await.map_err(|source| {
                    RecoverableTransferError::file_system(
                        "read retiring source directory",
                        path,
                        source,
                    )
                })?;
                entries
                    .next_entry()
                    .await
                    .map_err(|source| {
                        RecoverableTransferError::file_system(
                            "read retiring source directory entry",
                            path,
                            source,
                        )
                    })?
                    .is_none()
            }
        }
        FileObjectKind::RegularFile | FileObjectKind::SymbolicLink
            if matches!(position, ManifestEntryPosition::Root) =>
        {
            matches_object_verification(path, &current, expected, root_fingerprint).await?
        }
        FileObjectKind::RegularFile | FileObjectKind::SymbolicLink => current == *expected,
    };
    if matches {
        Ok(())
    } else {
        Err(RecoverableTransferError::SourceChanged {
            path: path.to_path_buf(),
        })
    }
}

async fn restore_unverified_owned_tree_entry(
    original_artifact_path: &Path,
    deletion_slot: &Path,
    recovered_base: &Path,
) -> Result<(), RecoverableTransferError> {
    let restored = if !path_exists(original_artifact_path).await? {
        original_artifact_path.to_path_buf()
    } else {
        super::next_recovered_path(recovered_base).await?
    };
    rename_noreplace(deletion_slot, &restored)
        .map_err(|error| error.into_transfer_error(deletion_slot, &restored))?;
    sync_rename_parents(deletion_slot, &restored).await
}

async fn remove_retirement_slot(path: &Path) -> Result<(), RecoverableTransferError> {
    let metadata = fs::symlink_metadata(path).await.map_err(|source| {
        RecoverableTransferError::file_system("read source deletion slot", path, source)
    })?;
    let removal = if metadata.file_type().is_dir() {
        fs::remove_dir(path).await
    } else {
        fs::remove_file(path).await
    };
    removal.map_err(|source| {
        RecoverableTransferError::file_system("remove verified source entry", path, source)
    })
}

async fn sync_removed_entry_parent(path: &Path) -> Result<(), RecoverableTransferError> {
    let path = path.to_path_buf();
    let error_path = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::ops::recoverable_transfer::sync_parent_blocking(&path)
    })
    .await
    .map_err(|join_error| {
        RecoverableTransferError::file_system(
            "join source deletion sync task for",
            &error_path,
            std::io::Error::other(join_error),
        )
    })?
}

pub(super) fn completed_target(committed: &CommittedTransfer) -> CompletedTarget {
    CompletedTarget {
        path: committed.final_target.clone(),
        identity: committed.target_identity.clone(),
        fingerprint: committed.fingerprint,
    }
}

pub(super) async fn verify_completed_target(
    completed: &CompletedTarget,
) -> Result<(), RecoverableTransferError> {
    let current_identity = inspect_file_identity(&completed.path).await?;
    if current_identity != completed.identity
        || fingerprint_object(&completed.path).await? != completed.fingerprint
    {
        return Err(RecoverableTransferError::TargetConflict {
            path: completed.path.clone(),
        });
    }
    Ok(())
}

pub(super) async fn verify_committed_target(
    committed: &CommittedTransfer,
) -> Result<(), RecoverableTransferError> {
    let current_identity = inspect_file_identity(&committed.final_target).await?;
    if current_identity != committed.target_identity
        || fingerprint_object(&committed.final_target).await? != committed.fingerprint
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
    expected_fingerprint: crate::ops::recoverable_transfer::ObjectFingerprint,
) -> Result<bool, RecoverableTransferError> {
    if !current_identity.same_object(expected_identity) {
        return Ok(false);
    }
    Ok(fingerprint_object(path).await? == expected_fingerprint)
}
