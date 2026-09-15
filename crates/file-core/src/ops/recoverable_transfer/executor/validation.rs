use super::super::{
    BackupCreationTransfer, CommitPayload, CommitTransfer, PreparedTransfer,
    RecoverableTransferError, RecoverableTransferOperation, RenamedDirectMove, SourceDisposition,
    StagedSourceLocation, StagingTransfer, TransferCheckpoint, TransferExecutionKind,
    TransferFingerprint, TransferJournalRecord,
};
use super::direct_move::renamed_target_matches_source;

pub(super) fn validate_checkpoint_semantics(
    record: &TransferJournalRecord,
    checkpoint: &TransferCheckpoint,
) -> Result<(), RecoverableTransferError> {
    match checkpoint {
        TransferCheckpoint::AwaitingManifest
        | TransferCheckpoint::Canceled { .. }
        | TransferCheckpoint::Failed { .. } => Ok(()),
        TransferCheckpoint::Completed(_) => validate_manifest_roots(record),
        TransferCheckpoint::Skipped
            if record.request.source == record.request.requested_target
                || matches!(
                    record.request.conflict_strategy,
                    crate::TransferConflictStrategy::Skip | crate::TransferConflictStrategy::Merge
                ) =>
        {
            Ok(())
        }
        TransferCheckpoint::Skipped => {
            invalid("skip checkpoint has incompatible conflict strategy")
        }
        TransferCheckpoint::Merging(merge) => {
            validate_manifest_roots(record)?;
            if record.request.conflict_strategy != crate::TransferConflictStrategy::Merge
                || record.manifest.is_none()
                || merge.next_child < merge.completed_children.len()
            {
                return invalid("merge checkpoint does not match the transfer request");
            }
            if let Some(child) = merge.active_child.as_ref() {
                if child.request.operation != record.request.operation
                    || child.request.conflict_strategy != crate::TransferConflictStrategy::Merge
                    || child.key.transfer_index != record.key.transfer_index
                    || !child
                        .key
                        .relative_path
                        .starts_with(&record.key.relative_path)
                {
                    return invalid("active merge child does not belong to its parent transfer");
                }
            }
            Ok(())
        }
        TransferCheckpoint::StageCreationIntent(prepared) => {
            validate_staged_prepared(record, prepared)
        }
        TransferCheckpoint::Staging(staging) => validate_staging(record, staging),
        TransferCheckpoint::DirectMoveIntent(prepared) => {
            validate_direct_move_prepared(record, prepared)
        }
        TransferCheckpoint::DirectMoveRenamed(renamed) => {
            validate_direct_move_renamed(record, renamed)
        }
        TransferCheckpoint::BackupCreationIntent(backup) => {
            validate_backup_creation(record, backup)
        }
        TransferCheckpoint::CommitIntent(commit) => validate_commit(record, commit),
        TransferCheckpoint::TargetCommitted(committed) => {
            validate_manifest_roots(record)?;
            let valid_disposition = match record.request.operation {
                RecoverableTransferOperation::Copy => {
                    committed.source_disposition == SourceDisposition::Preserved
                }
                RecoverableTransferOperation::Move => {
                    committed.source_disposition != SourceDisposition::Preserved
                }
            };
            if !valid_disposition
                || record.replacement_manifest.is_some()
                    && record.request.conflict_strategy != crate::TransferConflictStrategy::Replace
                || committed.backup_fingerprint.is_some() != record.replacement_manifest.is_some()
                || committed.backup_identity.is_some() != record.replacement_manifest.is_some()
                    && !(record.request.verification == crate::FileOperationVerification::Strong
                        && record.replacement_manifest.is_some()
                        && committed.backup_identity.is_none())
                || record.replacement_manifest.is_some() && committed.artifact.is_none()
            {
                return invalid("committed checkpoint does not match the transfer request");
            }
            Ok(())
        }
        TransferCheckpoint::SourceRetirementIntent(retirement) => {
            validate_manifest_roots(record)?;
            if record.request.operation != RecoverableTransferOperation::Move
                || retirement.committed.source_disposition != SourceDisposition::RequiresRetirement
                || retirement
                    .artifact
                    .as_ref()
                    .is_some_and(|artifact| artifact.plan != retirement.artifact_plan)
            {
                return invalid("source retirement is not owned by a cross-filesystem move");
            }
            Ok(())
        }
        TransferCheckpoint::SourceRetired(retired) => {
            validate_manifest_roots(record)?;
            if record.request.operation != RecoverableTransferOperation::Move
                || retired.committed.source_disposition != SourceDisposition::RequiresRetirement
                || retired.payload_identity.is_none()
                    && record.request.verification != crate::FileOperationVerification::Strong
            {
                return invalid("retired source does not belong to a cross-filesystem move");
            }
            Ok(())
        }
        TransferCheckpoint::CancelIntent(previous) => validate_terminal_intent(record, previous),
        TransferCheckpoint::FailureIntent(failure) => {
            validate_terminal_intent(record, &failure.previous)
        }
    }
}

fn validate_direct_move_prepared(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_prepared_facts(record, prepared)?;
    if record.request.operation == RecoverableTransferOperation::Move
        && record.request.verification == crate::FileOperationVerification::BasicMetadata
        && prepared.execution == TransferExecutionKind::MoveDirect
        && prepared.staging_plan.is_none()
        && prepared.expected_target_identity.is_none()
        && record.replacement_manifest.is_none()
    {
        Ok(())
    } else {
        invalid("direct move intent does not match a basic no-replace move")
    }
}

fn validate_direct_move_renamed(
    record: &TransferJournalRecord,
    renamed: &RenamedDirectMove,
) -> Result<(), RecoverableTransferError> {
    validate_direct_move_prepared(record, &renamed.prepared)?;
    if renamed_target_matches_source(&renamed.target_identity, &renamed.prepared.source_identity) {
        Ok(())
    } else {
        invalid("renamed direct move does not identify the prepared source")
    }
}

fn validate_staging(
    record: &TransferJournalRecord,
    staging: &StagingTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_staged_prepared(record, &staging.prepared)?;
    if staging.prepared.staging_plan.as_ref() != Some(&staging.artifact.plan) {
        return invalid("staging artifact does not match its persisted plan");
    }
    Ok(())
}

fn validate_backup_creation(
    record: &TransferJournalRecord,
    backup: &BackupCreationTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_staged_prepared(record, &backup.prepared)?;
    let valid_payload = match &backup.payload {
        CommitPayload::Artifact { artifact, .. } => {
            backup.prepared.staging_plan.as_ref() == Some(&artifact.plan)
        }
        CommitPayload::DirectSource { .. } => false,
    };
    let valid_target_proof = backup.prepared.expected_target_identity.is_some()
        && record.replacement_manifest.is_some()
        && match record.request.verification {
            crate::FileOperationVerification::BasicMetadata => {
                backup.prepared.expected_target_fingerprint.is_none()
            }
            crate::FileOperationVerification::Strong => {
                backup.prepared.expected_target_fingerprint.is_some()
            }
        };
    if valid_payload && valid_target_proof {
        Ok(())
    } else {
        invalid("backup creation checkpoint does not match a replacement transfer")
    }
}

fn validate_commit(
    record: &TransferJournalRecord,
    commit: &CommitTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_commit_prepared(record, &commit.prepared)?;
    let valid_payload = match (
        record.request.operation,
        commit.prepared.execution,
        &commit.payload,
    ) {
        (
            RecoverableTransferOperation::Copy,
            TransferExecutionKind::CopyToStage,
            CommitPayload::Artifact {
                artifact,
                source_location: StagedSourceLocation::OriginalPath,
                ..
            },
        ) => commit.prepared.staging_plan.as_ref() == Some(&artifact.plan),
        (
            RecoverableTransferOperation::Move,
            TransferExecutionKind::MoveToStage,
            CommitPayload::Artifact { artifact, .. },
        ) => commit.prepared.staging_plan.as_ref() == Some(&artifact.plan),
        (
            RecoverableTransferOperation::Move,
            TransferExecutionKind::MoveDirect,
            CommitPayload::DirectSource { .. },
        ) => true,
        _ => false,
    };
    let strong_payload_matches = record.request.verification
        != crate::FileOperationVerification::Strong
        || match &commit.fingerprint {
            TransferFingerprint::Blake3(fingerprint) => {
                commit.prepared.source_fingerprint.as_ref() == Some(fingerprint)
            }
            // Strong 校验不允许克隆证明
            TransferFingerprint::KernelClone { .. } => false,
        };
    let replacement_proof_matches = if record.replacement_manifest.is_some() {
        commit.prepared.expected_target_fingerprint.is_some()
            && (commit.backup_identity.is_some()
                || record.request.verification == crate::FileOperationVerification::Strong)
    } else {
        commit.backup_identity.is_none()
            && commit.prepared.expected_target_identity.is_none()
            && commit.prepared.expected_target_fingerprint.is_none()
    };
    if !valid_payload || !strong_payload_matches || !replacement_proof_matches {
        return invalid("commit payload does not match the transfer operation");
    }
    Ok(())
}

fn validate_staged_prepared(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_prepared_facts(record, prepared)?;
    let valid = matches!(
        (record.request.operation, prepared.execution),
        (
            RecoverableTransferOperation::Copy,
            TransferExecutionKind::CopyToStage
        ) | (
            RecoverableTransferOperation::Move,
            TransferExecutionKind::MoveToStage
        )
    ) && prepared.staging_plan.is_some();
    if valid {
        Ok(())
    } else {
        invalid("staged preparation does not match the transfer operation")
    }
}

fn validate_commit_prepared(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_prepared_facts(record, prepared)?;
    let valid_plan = match prepared.execution {
        TransferExecutionKind::CopyToStage | TransferExecutionKind::MoveToStage => {
            prepared.staging_plan.is_some()
        }
        TransferExecutionKind::MoveDirect => {
            prepared.staging_plan.is_none()
                && record.request.verification == crate::FileOperationVerification::Strong
        }
        TransferExecutionKind::MergeDirectory => false,
    };
    if valid_plan {
        Ok(())
    } else {
        invalid("commit preparation has an invalid staging plan")
    }
}

fn validate_prepared_facts(
    record: &TransferJournalRecord,
    prepared: &PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    validate_manifest_roots(record)?;
    let source_identity_matches_manifest = record
        .manifest
        .as_ref()
        .and_then(|manifest| {
            manifest
                .entries
                .iter()
                .find(|entry| entry.relative_path.as_os_str().is_empty())
        })
        .is_some_and(|entry| entry.identity == prepared.source_identity);
    let replacement_facts_are_paired = prepared.expected_target_identity.is_some()
        == record.replacement_manifest.is_some()
        && (prepared.expected_target_identity.is_none()
            || record.request.conflict_strategy == crate::TransferConflictStrategy::Replace)
        && !(prepared.expected_target_fingerprint.is_some()
            && prepared.expected_target_identity.is_none())
        && !(record.request.verification == crate::FileOperationVerification::Strong
            && prepared.expected_target_identity.is_some()
            && prepared.expected_target_fingerprint.is_none());
    let source_fingerprint_matches_mode = match record.request.verification {
        crate::FileOperationVerification::BasicMetadata => prepared.source_fingerprint.is_none(),
        crate::FileOperationVerification::Strong => prepared.source_fingerprint.is_some(),
    };
    if !source_identity_matches_manifest
        || !replacement_facts_are_paired
        || !source_fingerprint_matches_mode
    {
        return invalid("prepared checkpoint does not match the transfer request");
    }
    Ok(())
}

fn validate_manifest_roots(record: &TransferJournalRecord) -> Result<(), RecoverableTransferError> {
    let source_manifest_matches = record.manifest.as_ref().is_some_and(|manifest| {
        manifest.root == record.request.source
            && manifest
                .entries
                .iter()
                .any(|entry| entry.relative_path.as_os_str().is_empty())
    });
    let replacement_manifest_matches =
        record.replacement_manifest.as_ref().is_none_or(|manifest| {
            manifest.root == record.request.requested_target
                && manifest
                    .entries
                    .iter()
                    .any(|entry| entry.relative_path.as_os_str().is_empty())
                && record.request.conflict_strategy == crate::TransferConflictStrategy::Replace
        });
    if source_manifest_matches && replacement_manifest_matches {
        Ok(())
    } else {
        invalid("persisted manifest roots do not match the transfer request")
    }
}

fn validate_terminal_intent(
    record: &TransferJournalRecord,
    previous: &TransferCheckpoint,
) -> Result<(), RecoverableTransferError> {
    if matches!(
        previous,
        TransferCheckpoint::AwaitingManifest
            | TransferCheckpoint::Merging(_)
            | TransferCheckpoint::StageCreationIntent(_)
            | TransferCheckpoint::Staging(_)
            | TransferCheckpoint::DirectMoveIntent(_)
            | TransferCheckpoint::CommitIntent(_)
    ) {
        validate_checkpoint_semantics(record, previous)
    } else {
        invalid("terminal intent contains an irreversible or terminal checkpoint")
    }
}

fn invalid(message: &str) -> Result<(), RecoverableTransferError> {
    Err(RecoverableTransferError::InvalidCheckpoint {
        message: message.to_owned(),
    })
}
