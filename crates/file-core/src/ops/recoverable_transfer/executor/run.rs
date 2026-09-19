use std::path::Path;

use super::super::{
    RecoverableTransferError, RecoverableTransferOutcome, TransferCheckpoint, TransferJournal,
    TransferJournalRecord,
};
use super::recovery::{cancel_recoverable_transfer, fail_recoverable_transfer};
use super::{
    advance_recoverable_transfer, checkpoint_requires_forward_recovery, ProofContext,
    TransferAdvance,
};
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
            let proof = ProofContext::blank_proof();
            fail_recoverable_transfer(&mut record, journal, diagnostic, &proof).await?;
            Ok(record)
        }
    }
}

async fn run_recoverable_transfer_to_boundary<J: TransferJournal>(
    mut record: TransferJournalRecord,
    journal: &J,
    mut transfer_options: FileTransferOptions,
    boundary: TransferRunBoundary,
) -> Result<TransferRunStop, RecoverableTransferError> {
    // Basic 校验默认携带运行内证明记忆:一次前向运行内每对象至多全文哈希
    // 一次,崩溃重启后 memo 随运行重建,恢复路径仍全文重验。调用方已附加的
    // memo(跨记录共享)原样保留。
    if record.request.verification == crate::ops::FileOperationVerification::BasicMetadata
        && transfer_options.proof_memo.is_none()
    {
        transfer_options.proof_memo = Some(crate::ops::recoverable_transfer::ProofMemo::shared());
    }
    if transfer_options.strategy_engine.is_none() {
        // 引擎探测需要源与目标父目录的设备号;任一侧拿不到就保持 None,
        // 搬运退回历史路径,不阻塞传输。
        let source_device = tokio::fs::symlink_metadata(&record.request.source)
            .await
            .ok()
            .map(|metadata| std::os::unix::fs::MetadataExt::dev(&metadata));
        let target_parent = record
            .request
            .requested_target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| record.request.requested_target.clone());
        let parent_probe = tokio::fs::symlink_metadata(&target_parent).await.ok();
        if let (Some(source_device), Some(parent_metadata)) = (source_device, parent_probe) {
            let target_device = std::os::unix::fs::MetadataExt::dev(&parent_metadata);
            transfer_options.strategy_engine = Some(std::sync::Arc::new(
                crate::ops::transfer_strategy::TransferStrategyEngine::probe(
                    source_device,
                    target_device,
                    &target_parent,
                ),
            ));
        }
    }

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

        let advance_result =
            advance_recoverable_transfer(&mut record, journal, &transfer_options).await;
        match advance_result {
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
                    let proof = ProofContext::new(
                        record.request.verification,
                        transfer_options.proof_memo.clone(),
                    );
                    if let Err(cancel_error) =
                        cancel_recoverable_transfer(&mut record, journal, &proof).await
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
                let proof = ProofContext::new(
                    record.request.verification,
                    transfer_options.proof_memo.clone(),
                );
                if let Err(cleanup_error) =
                    fail_recoverable_transfer(&mut record, journal, diagnostic.clone(), &proof)
                        .await
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
