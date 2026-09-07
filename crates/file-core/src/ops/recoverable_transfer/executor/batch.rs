use std::collections::BTreeSet;
use std::path::PathBuf;

use super::direct_move::{
    direct_move_rename_only, renamed_target_matches_source, DirectMoveRenameStep,
};
use super::{journal_error, persist_checkpoint, sync_parent, validate_next_revision};
use crate::ops::recoverable_transfer::{
    inspect_file_identity, PreparedTransfer, RecoverableTransferError, RenamedDirectMove,
    TransferCheckpoint, TransferCheckpointSwap, TransferFailureIntent, TransferJournal,
    TransferJournalRecord,
};
use crate::FileTransferOptions;

#[derive(Debug)]
pub enum DirectMoveBatchRecord {
    /// The record's `DirectMoveRenamed` fact is durable via the batch CAS.
    Renamed(TransferJournalRecord),
    /// The record left the direct path and has a durable fallback checkpoint.
    Diverged(TransferJournalRecord),
    /// The record was blocked without stopping already independent siblings.
    Failed {
        record: TransferJournalRecord,
        error: RecoverableTransferError,
    },
}

enum RenameOutcome {
    Renamed {
        record: Box<TransferJournalRecord>,
        prepared: Box<PreparedTransfer>,
    },
    Diverged {
        record: Box<TransferJournalRecord>,
        checkpoint: Box<TransferCheckpoint>,
    },
    Failed {
        record: Box<TransferJournalRecord>,
        error: RecoverableTransferError,
    },
}

// Keep the input order while moving the durable boundary out of the per-item
// loop. A business failure belongs to one record; journal failure or shutdown
// stops the whole segment because continuing would lose recovery ownership.
// `on_renamed` fires after every rename in the batch has already happened on
// the filesystem but before any durability work (parent fsync, journal CAS):
// the moves are real facts at that point, so callers may surface them to the
// UI without waiting for the recoverable-transfer commit.
pub async fn run_direct_move_batch_to_durable_renamed<J: TransferJournal>(
    records: Vec<TransferJournalRecord>,
    journal: &J,
    transfer_options: &FileTransferOptions,
    on_renamed: &mut (dyn FnMut(Vec<(PathBuf, PathBuf)>) + Send),
) -> Result<Vec<DirectMoveBatchRecord>, RecoverableTransferError> {
    let mut outcomes: Vec<Option<RenameOutcome>> = Vec::with_capacity(records.len());
    let mut renamed_moves: Vec<(PathBuf, PathBuf)> = Vec::new();

    for record in records {
        let prepared = match &record.checkpoint {
            TransferCheckpoint::DirectMoveIntent(prepared) => prepared.clone(),
            _ => {
                return Err(RecoverableTransferError::InvalidCheckpoint {
                    message: "direct move batch requires a DirectMoveIntent checkpoint".to_owned(),
                });
            }
        };

        let outcome = match direct_move_rename_only(&record, transfer_options, prepared).await {
            Ok(DirectMoveRenameStep::Renamed { prepared }) => {
                renamed_moves.push((
                    record.request.source.clone(),
                    prepared.resolved_target.clone(),
                ));
                RenameOutcome::Renamed {
                    record: Box::new(record),
                    prepared: Box::new(*prepared),
                }
            }
            Ok(DirectMoveRenameStep::Diverged(checkpoint)) => RenameOutcome::Diverged {
                record: Box::new(record),
                checkpoint,
            },
            Err(error) if is_batch_global_error(&error) => return Err(error),
            Err(error) => RenameOutcome::Failed {
                record: Box::new(record),
                error,
            },
        };
        outcomes.push(Some(outcome));
    }

    let mut parents = BTreeSet::new();
    for outcome in outcomes.iter().flatten() {
        let RenameOutcome::Renamed {
            record, prepared, ..
        } = outcome
        else {
            continue;
        };
        if let Some(parent) = record.request.source.parent() {
            if !parent.as_os_str().is_empty() {
                parents.insert(parent.to_path_buf());
            }
        }
        if let Some(parent) = prepared.resolved_target.parent() {
            if !parent.as_os_str().is_empty() {
                parents.insert(parent.to_path_buf());
            }
        }
    }

    let renamed_positions: Vec<usize> = outcomes
        .iter()
        .enumerate()
        .filter_map(|(index, outcome)| {
            matches!(outcome, Some(RenameOutcome::Renamed { .. })).then_some(index)
        })
        .collect();

    if !renamed_positions.is_empty() {
        if !renamed_moves.is_empty() {
            on_renamed(std::mem::take(&mut renamed_moves));
        }
        for parent in parents {
            sync_parent(&parent).await?;
        }

        let mut swaps = Vec::with_capacity(renamed_positions.len());
        for &position in &renamed_positions {
            let RenameOutcome::Renamed {
                record, prepared, ..
            } = outcomes[position]
                .as_ref()
                .expect("renamed position is present")
            else {
                unreachable!("renamed position changed before batch CAS");
            };
            let target_identity = inspect_file_identity(&prepared.resolved_target).await?;
            if !renamed_target_matches_source(&target_identity, &prepared.source_identity) {
                let previous = outcomes[position]
                    .take()
                    .expect("renamed outcome is present");
                let RenameOutcome::Renamed { record, prepared } = previous else {
                    unreachable!("renamed position changed during target validation");
                };
                let previous = record.checkpoint.clone();
                outcomes[position] = Some(RenameOutcome::Diverged {
                    record,
                    checkpoint: Box::new(TransferCheckpoint::FailureIntent(
                        TransferFailureIntent {
                            previous: Box::new(previous),
                            diagnostic: format!(
                                "direct move target does not identify the prepared source: {:?}",
                                prepared.resolved_target
                            ),
                        },
                    )),
                });
                continue;
            }
            swaps.push(TransferCheckpointSwap {
                task_id: record.task_id,
                key: record.key.clone(),
                expected_revision: record.revision,
                checkpoint: TransferCheckpoint::DirectMoveRenamed(RenamedDirectMove {
                    prepared: (**prepared).clone(),
                    target_identity,
                }),
            });
        }

        if !swaps.is_empty() {
            let swap_count = swaps.len();
            let revisions = journal
                .commit_checkpoint_batch(swaps)
                .await
                .map_err(journal_error)?;
            if revisions.len() != swap_count {
                return Err(RecoverableTransferError::Journal {
                    message: format!(
                        "batch checkpoint commit returned {} revisions for {} swaps",
                        revisions.len(),
                        swap_count
                    ),
                });
            }

            let mut revision_index = 0;
            for position in renamed_positions {
                let Some(RenameOutcome::Renamed { .. }) = outcomes[position].as_ref() else {
                    continue;
                };
                let revision = revisions[revision_index];
                revision_index += 1;
                let previous = outcomes[position]
                    .take()
                    .expect("renamed outcome is present");
                let RenameOutcome::Renamed {
                    mut record,
                    prepared,
                } = previous
                else {
                    unreachable!("renamed position changed before revision update");
                };
                validate_next_revision(record.revision, revision)?;
                let target_identity = inspect_file_identity(&prepared.resolved_target).await?;
                record.revision = revision;
                record.checkpoint = TransferCheckpoint::DirectMoveRenamed(RenamedDirectMove {
                    prepared: (*prepared).clone(),
                    target_identity: target_identity.clone(),
                });
                outcomes[position] = Some(RenameOutcome::Renamed { record, prepared });
            }
            if revision_index != revisions.len() {
                return Err(RecoverableTransferError::Journal {
                    message: "batch checkpoint revision count did not match renamed records"
                        .to_owned(),
                });
            }
        }
    }

    let mut result = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        match outcome.expect("batch outcome is present") {
            RenameOutcome::Renamed { record, .. } => {
                result.push(DirectMoveBatchRecord::Renamed(*record));
            }
            RenameOutcome::Diverged {
                mut record,
                checkpoint,
            } => {
                persist_checkpoint(&mut record, journal, *checkpoint).await?;
                result.push(DirectMoveBatchRecord::Diverged(*record));
            }
            RenameOutcome::Failed { record, error } => {
                result.push(DirectMoveBatchRecord::Failed {
                    record: *record,
                    error,
                });
            }
        }
    }
    Ok(result)
}

fn is_batch_global_error(error: &RecoverableTransferError) -> bool {
    matches!(
        error,
        RecoverableTransferError::Journal { .. }
            | RecoverableTransferError::RecoveryRequired { .. }
            | RecoverableTransferError::FileOperation(crate::FileError::ApplicationStopping)
    )
}
