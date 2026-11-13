use super::commit::verify_prepared_source_with_controls;
use super::{path_exists, target_parent, wait_until_running};
use crate::ops::metadata_loss::MetadataLossSummary;
use crate::ops::recoverable_transfer::CommitBarrier;
use crate::ops::recoverable_transfer::{
    inspect_file_identity, plan_owned_artifact, rename_noreplace, CommittedTransfer,
    NoReplaceRenameError, OwnedArtifactKind, PreparedTransfer, ProofContext,
    RecoverableTransferError, RenamedDirectMove, SourceDisposition, TransferCheckpoint,
    TransferExecutionKind, TransferFingerprint, TransferJournalRecord, TransferTransition,
};
use crate::transfer_conflict::first_available_entry_path;
use crate::{EntryNameKind, FileTransferOptions, TransferConflictStrategy};

/// Renames the source onto the prepared target. The intent is durable before
/// this runs, so a crash after the rename recovers through the same
/// source/target matrix: a target that already identifies the source means an
/// earlier run renamed it. Leaving the direct path (another filesystem, or a
/// KeepBoth name taken meanwhile) is a transition like any other.
pub(super) async fn advance_direct_move_intent(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    prepared: PreparedTransfer,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
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
        return renamed_transition(record, prepared, barrier).await;
    }

    match (source_exists, target_exists) {
        (true, false) => {
            verify_prepared_source_with_controls(record, &prepared, &transfer_options.controls)
                .await?;
            match rename_noreplace(&record.request.source, &prepared.resolved_target) {
                Ok(()) => renamed_transition(record, prepared, barrier).await,
                Err(NoReplaceRenameError::CrossDevice) => {
                    let mut prepared = prepared;
                    prepared.execution = TransferExecutionKind::MoveToStage;
                    prepared.staging_plan = Some(plan_owned_artifact(
                        target_parent(&prepared.resolved_target)?,
                        OwnedArtifactKind::TargetStaging,
                        record.owner(0),
                    )?);
                    Ok(TransferTransition::Checkpoint(
                        TransferCheckpoint::StageCreationIntent(prepared),
                    ))
                }
                Err(NoReplaceRenameError::TargetExists)
                    if record.request.conflict_strategy == TransferConflictStrategy::KeepBoth =>
                {
                    let prepared = select_keep_both_candidate(record, prepared).await?;
                    Ok(TransferTransition::Checkpoint(
                        TransferCheckpoint::DirectMoveIntent(prepared),
                    ))
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
                Ok(TransferTransition::Checkpoint(
                    TransferCheckpoint::DirectMoveIntent(prepared),
                ))
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

async fn renamed_transition(
    record: &TransferJournalRecord,
    prepared: PreparedTransfer,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    barrier.require_rename(&record.request.source, &prepared.resolved_target);
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
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::DirectMoveRenamed(RenamedDirectMove {
            prepared,
            target_identity,
        }),
    ))
}

pub(super) async fn advance_direct_move_renamed(
    renamed: RenamedDirectMove,
    proof: &ProofContext,
) -> Result<TransferTransition, RecoverableTransferError> {
    let identity_before = inspect_file_identity(&renamed.prepared.resolved_target).await?;
    if identity_before != renamed.target_identity {
        return Err(RecoverableTransferError::TargetConflict {
            path: renamed.prepared.resolved_target,
        });
    }
    let fingerprint = TransferFingerprint::Blake3(
        proof
            .fingerprint_object(&renamed.prepared.resolved_target)
            .await?,
    );
    let identity_after = inspect_file_identity(&renamed.prepared.resolved_target).await?;
    if identity_after != identity_before {
        return Err(RecoverableTransferError::TargetConflict {
            path: renamed.prepared.resolved_target,
        });
    }
    Ok(TransferTransition::Checkpoint(
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
            // 同盘 rename 不改动元数据,没有可丢的。
            metadata_loss: MetadataLossSummary::default(),
        }),
    ))
}

async fn select_keep_both_candidate(
    record: &TransferJournalRecord,
    mut prepared: PreparedTransfer,
) -> Result<PreparedTransfer, RecoverableTransferError> {
    prepared.resolved_target = first_available_entry_path(
        &record.request.requested_target,
        EntryNameKind::from(prepared.source_identity.object_kind),
    )
    .await?;
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
