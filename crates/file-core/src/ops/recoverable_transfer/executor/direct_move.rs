use super::commit::verify_prepared_source_with_controls;
use super::{
    path_exists, persist_checkpoint, sync_rename_parents, target_parent, wait_until_running,
};
use crate::ops::recoverable_transfer::{
    inspect_file_identity, plan_owned_artifact, rename_noreplace,
    CommittedTransfer, NoReplaceRenameError, OwnedArtifactKind, PreparedTransfer,
    RecoverableTransferError, RenamedDirectMove, SourceDisposition, TransferCheckpoint,
    TransferExecutionKind, TransferJournal, TransferJournalRecord, ProofContext, TransferFingerprint,
};
use crate::transfer_conflict::available_transfer_target_path_candidate;
use crate::{FileTransferOptions, TransferConflictStrategy};

/// Result of the rename-only step of a Basic DirectMove intent.
pub(super) enum DirectMoveRenameStep {
    /// The source was atomically renamed to the final target (or recovery
    /// observed that an earlier run already performed the rename). Parent
    /// directories have not been synced and no post-rename fact is durable yet.
    Renamed { prepared: Box<PreparedTransfer> },
    /// The record left the direct-move path. The carried checkpoint is the
    /// transition to persist later (EXDEV staging or KeepBoth retarget); it is
    /// NOT persisted here so a diverging sibling cannot block the rename wave.
    Diverged(Box<TransferCheckpoint>),
}

/// Perform the atomic rename for a `DirectMoveIntent` without syncing parents or
/// persisting a post-rename fact. The intent is already durable before this is
/// called, so every crash after a rename here recovers through the same source/
/// target matrix on the next run. Divergence is reported as a pending checkpoint
/// transition and never persisted inline.
pub(super) async fn direct_move_rename_only(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    prepared: PreparedTransfer,
) -> Result<DirectMoveRenameStep, RecoverableTransferError> {
    let source_exists = path_exists(&record.request.source).await.map_err(|error| {
        RecoverableTransferError::RecoveryRequired {
            diagnostic: error.to_string(),
        }
    })?;
    let target_exists = path_exists(&prepared.resolved_target)
        .await
        .map_err(|error| RecoverableTransferError::RecoveryRequired {
            diagnostic: error.to_string(),
        })?;
    let target_already_renamed = if target_exists {
        let target_identity = inspect_file_identity(&prepared.resolved_target)
            .await
            .map_err(|error| RecoverableTransferError::RecoveryRequired {
                diagnostic: error.to_string(),
            })?;
        renamed_target_matches_source(&target_identity, &prepared.source_identity)
    } else {
        false
    };

    if target_already_renamed {
        if source_exists {
            return Err(RecoverableTransferError::RecoveryBlocked {
                diagnostic: format!(
                    "direct move target identifies the prepared source, but the source path also exists: {:?} -> {:?}",
                    record.request.source, prepared.resolved_target
                ),
            });
        }
        return Ok(DirectMoveRenameStep::Renamed {
            prepared: Box::new(prepared),
        });
    }

    match (source_exists, target_exists) {
        (true, false) => {
            verify_prepared_source_with_controls(record, &prepared, &transfer_options.controls)
                .await?;
            match rename_noreplace(&record.request.source, &prepared.resolved_target) {
                Ok(()) => Ok(DirectMoveRenameStep::Renamed {
                    prepared: Box::new(prepared),
                }),
                Err(NoReplaceRenameError::CrossDevice) => {
                    let mut prepared = prepared;
                    prepared.execution = TransferExecutionKind::MoveToStage;
                    prepared.staging_plan = Some(plan_owned_artifact(
                        target_parent(&prepared.resolved_target)?,
                        OwnedArtifactKind::TargetStaging,
                        record.owner(0),
                    )?);
                    Ok(DirectMoveRenameStep::Diverged(Box::new(
                        TransferCheckpoint::StageCreationIntent(prepared),
                    )))
                }
                Err(NoReplaceRenameError::TargetExists)
                    if record.request.conflict_strategy == TransferConflictStrategy::KeepBoth =>
                {
                    let prepared = select_keep_both_candidate(record, prepared).await?;
                    Ok(DirectMoveRenameStep::Diverged(Box::new(
                        TransferCheckpoint::DirectMoveIntent(prepared),
                    )))
                }
                Err(error) => {
                    Err(error
                        .into_transfer_error(&record.request.source, &prepared.resolved_target))
                }
            }
        }
        (true, true) => {
            wait_until_running(transfer_options).await?;
            if record.request.conflict_strategy == TransferConflictStrategy::KeepBoth {
                let prepared = select_keep_both_candidate(record, prepared).await?;
                Ok(DirectMoveRenameStep::Diverged(Box::new(
                    TransferCheckpoint::DirectMoveIntent(prepared),
                )))
            } else {
                Err(RecoverableTransferError::TargetConflict {
                    path: prepared.resolved_target,
                })
            }
        }
        (false, true) => Err(RecoverableTransferError::RecoveryBlocked {
            diagnostic: format!(
                "direct move target does not identify the prepared source: {:?}",
                prepared.resolved_target
            ),
        }),
        (false, false) => Err(RecoverableTransferError::RecoveryBlocked {
            diagnostic: format!(
                "direct move source and target are both missing: {:?} -> {:?}",
                record.request.source, prepared.resolved_target
            ),
        }),
    }
}

/// Advance a durable DirectMoveIntent through rename, parent sync and the
/// post-rename durable checkpoint. UI facts are emitted by the caller only
/// after that checkpoint has succeeded.
pub(super) async fn advance_direct_move_intent<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    transfer_options: &FileTransferOptions,
    prepared: PreparedTransfer,
) -> Result<(), RecoverableTransferError> {
    match direct_move_rename_only(record, transfer_options, prepared).await? {
        DirectMoveRenameStep::Renamed { prepared } => {
            let prepared = *prepared;
            sync_rename_parents(&record.request.source, &prepared.resolved_target)
                .await
                .map_err(|error| RecoverableTransferError::RecoveryRequired {
                    diagnostic: error.to_string(),
                })?;
            let target_identity = inspect_file_identity(&prepared.resolved_target)
                .await
                .map_err(|error| RecoverableTransferError::RecoveryRequired {
                    diagnostic: error.to_string(),
                })?;
            if !renamed_target_matches_source(&target_identity, &prepared.source_identity) {
                return Err(RecoverableTransferError::RecoveryBlocked {
                    diagnostic: format!(
                        "direct move target does not identify the prepared source: {:?}",
                        prepared.resolved_target
                    ),
                });
            }
            persist_checkpoint(
                record,
                journal,
                TransferCheckpoint::DirectMoveRenamed(RenamedDirectMove {
                    prepared,
                    target_identity,
                }),
            )
            .await
        }
        DirectMoveRenameStep::Diverged(checkpoint) => {
            persist_checkpoint(record, journal, *checkpoint).await
        }
    }
}

pub(super) async fn advance_direct_move_renamed<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    renamed: RenamedDirectMove,
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    let identity_before = inspect_file_identity(&renamed.prepared.resolved_target).await?;
    if identity_before != renamed.target_identity {
        return Err(RecoverableTransferError::TargetConflict {
            path: renamed.prepared.resolved_target,
        });
    }
    let fingerprint = TransferFingerprint::Blake3(
        proof.fingerprint_object(&renamed.prepared.resolved_target).await?,
    );
    let identity_after = inspect_file_identity(&renamed.prepared.resolved_target).await?;
    if identity_after != identity_before {
        return Err(RecoverableTransferError::TargetConflict {
            path: renamed.prepared.resolved_target,
        });
    }
    persist_checkpoint(
        record,
        journal,
        TransferCheckpoint::TargetCommitted(CommittedTransfer {
            final_target: renamed.prepared.resolved_target,
            target_identity: identity_after,
            fingerprint,
            artifact: None,
            source_disposition: SourceDisposition::MovedByCommit,
            backup_identity: None,
            backup_fingerprint: None,
            backup_cleanup_index: 0,
            backup_cleanup_intent: None,
        }),
    )
    .await
}

async fn select_keep_both_candidate(
    record: &TransferJournalRecord,
    mut prepared: PreparedTransfer,
) -> Result<PreparedTransfer, RecoverableTransferError> {
    prepared.resolved_target =
        available_transfer_target_path_candidate(&record.request.requested_target)
            .await
            .map_err(|source| {
                RecoverableTransferError::file_system(
                    "select keep-both target for",
                    &record.request.requested_target,
                    source,
                )
            })?;
    Ok(prepared)
}

pub(super) fn renamed_target_matches_source(
    current: &crate::FileIdentity,
    expected: &crate::FileIdentity,
) -> bool {
    current.same_object(expected)
        && current.size == expected.size
        && current.modified_seconds == expected.modified_seconds
        && current.modified_nanoseconds == expected.modified_nanoseconds
        && current.symbolic_link_target == expected.symbolic_link_target
}
