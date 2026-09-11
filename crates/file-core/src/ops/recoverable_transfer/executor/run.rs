use super::super::{
    RecoverableTransferError, RecoverableTransferOutcome, TransferCheckpoint, TransferJournal,
    TransferJournalRecord,
};
use super::recovery::{cancel_recoverable_transfer, fail_recoverable_transfer};
use super::{advance_recoverable_transfer, checkpoint_requires_forward_recovery, TransferAdvance};
use crate::{FileError, FileTransferOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectMoveIntentBoundary {
    Intent(TransferJournalRecord),
    NotApplicable(TransferJournalRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferRunBoundary {
    Completion,
    DirectMoveIntent,
}

enum TransferRunStop {
    Completed(RecoverableTransferOutcome),
    DirectMoveIntentPrepared(TransferJournalRecord),
    DirectMoveNotApplicable(TransferJournalRecord),
}

pub async fn run_recoverable_transfer<J: TransferJournal>(
    record: TransferJournalRecord,
    journal: &J,
    transfer_options: FileTransferOptions,
) -> Result<RecoverableTransferOutcome, RecoverableTransferError> {
    match run_recoverable_transfer_to_boundary(
        record,
        journal,
        transfer_options,
        TransferRunBoundary::Completion,
    )
    .await?
    {
        TransferRunStop::Completed(outcome) => Ok(outcome),
        TransferRunStop::DirectMoveIntentPrepared(_)
        | TransferRunStop::DirectMoveNotApplicable(_) => {
            unreachable!("completion run cannot stop at direct move intent")
        }
    }
}

// Advance one record only until its `DirectMoveIntent` is durable, i.e. until
// the intent that can independently recover a later rename is persisted and no
// filesystem side effect has happened yet. This is the preparation boundary of
// a Basic DirectMove batch; the rename and its post-rename facts are produced
// separately by `super::batch::run_direct_move_batch_to_durable_renamed`.
pub async fn run_recoverable_transfer_to_direct_move_intent<J: TransferJournal>(
    record: TransferJournalRecord,
    journal: &J,
    transfer_options: FileTransferOptions,
) -> Result<DirectMoveIntentBoundary, RecoverableTransferError> {
    match run_recoverable_transfer_to_boundary(
        record,
        journal,
        transfer_options,
        TransferRunBoundary::DirectMoveIntent,
    )
    .await?
    {
        TransferRunStop::DirectMoveIntentPrepared(record) => {
            Ok(DirectMoveIntentBoundary::Intent(record))
        }
        TransferRunStop::DirectMoveNotApplicable(record) => {
            Ok(DirectMoveIntentBoundary::NotApplicable(record))
        }
        TransferRunStop::Completed(_) => {
            unreachable!("direct move intent run cannot complete a non-direct transfer")
        }
    }
}

/// 任务以普通失败收尾前,把单个仍可回滚的记录就地结算为终态失败。失败与
/// 取消一样必须先关账:队列只有确认恢复日志全部终态后才能写入失败状态并
/// 丢弃恢复细节,否则任务会永远停在中间状态、每次启动都被恢复重跑。
/// 终态记录原样返回;forward-only 检查点(含悬挂的 cancel/failure intent)
/// 禁止就地失败,只能由前向恢复收敛。
pub async fn settle_failed_recoverable_transfer<J: TransferJournal>(
    mut record: TransferJournalRecord,
    journal: &J,
    diagnostic: String,
) -> Result<TransferJournalRecord, RecoverableTransferError> {
    if checkpoint_requires_forward_recovery(&record.checkpoint) {
        return Err(RecoverableTransferError::RecoveryBlocked {
            diagnostic: format!(
                "record must recover forward instead of failing in place: {diagnostic}"
            ),
        });
    }
    match record.checkpoint {
        TransferCheckpoint::Completed(_)
        | TransferCheckpoint::Canceled { .. }
        | TransferCheckpoint::Failed { .. }
        | TransferCheckpoint::Skipped => Ok(record),
        _ => {
            fail_recoverable_transfer(&mut record, journal, diagnostic).await?;
            Ok(record)
        }
    }
}

async fn run_recoverable_transfer_to_boundary<J: TransferJournal>(
    mut record: TransferJournalRecord,
    journal: &J,
    transfer_options: FileTransferOptions,
    boundary: TransferRunBoundary,
) -> Result<TransferRunStop, RecoverableTransferError> {
    loop {
        if boundary == TransferRunBoundary::DirectMoveIntent {
            match record.checkpoint {
                TransferCheckpoint::AwaitingManifest => {}
                TransferCheckpoint::DirectMoveIntent(_) => {
                    return Ok(TransferRunStop::DirectMoveIntentPrepared(record));
                }
                _ => return Ok(TransferRunStop::DirectMoveNotApplicable(record)),
            }
        }

        match advance_recoverable_transfer(&mut record, journal, &transfer_options).await {
            Ok(TransferAdvance::Continue) => {}
            Ok(TransferAdvance::Complete(outcome)) => {
                return Ok(TransferRunStop::Completed(outcome));
            }
            Err(
                error @ RecoverableTransferError::FileOperation(FileError::ApplicationStopping),
            ) => {
                return Err(error);
            }
            Err(error)
                if matches!(
                    error,
                    RecoverableTransferError::FileOperation(FileError::Cancelled)
                ) =>
            {
                if !matches!(record.checkpoint, TransferCheckpoint::Canceled { .. }) {
                    if let Err(cancel_error) =
                        cancel_recoverable_transfer(&mut record, journal).await
                    {
                        return match cancel_error {
                            RecoverableTransferError::Journal { .. } => Err(cancel_error),
                            cancel_error => Err(RecoverableTransferError::RecoveryBlocked {
                                diagnostic: format!(
                                    "cancellation cleanup could not finish: {cancel_error}"
                                ),
                            }),
                        };
                    }
                }
                return Err(error);
            }
            Err(error)
                if matches!(
                    error,
                    RecoverableTransferError::Journal { .. }
                        | RecoverableTransferError::RecoveryRequired { .. }
                        | RecoverableTransferError::RecoveryBlocked { .. }
                        | RecoverableTransferError::RecordedFailure { .. }
                ) =>
            {
                return Err(error);
            }
            Err(error @ RecoverableTransferError::InvalidCheckpoint { .. }) => {
                return Err(RecoverableTransferError::RecoveryBlocked {
                    diagnostic: error.to_string(),
                });
            }
            Err(error) if checkpoint_requires_forward_recovery(&record.checkpoint) => {
                return Err(RecoverableTransferError::RecoveryBlocked {
                    diagnostic: error.to_string(),
                });
            }
            Err(error) => {
                let diagnostic = error.to_string();
                if let Err(cleanup_error) =
                    fail_recoverable_transfer(&mut record, journal, diagnostic.clone()).await
                {
                    return match cleanup_error {
                        RecoverableTransferError::Journal { .. } => Err(cleanup_error),
                        cleanup_error => Err(RecoverableTransferError::RecoveryBlocked {
                            diagnostic: format!(
                                "{diagnostic}; recovery cleanup could not finish: {cleanup_error}"
                            ),
                        }),
                    };
                }
                return Err(error);
            }
        }
    }
}
