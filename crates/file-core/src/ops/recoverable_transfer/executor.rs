use std::path::{Path, PathBuf};

use tokio::fs;

use super::super::copy::{
    copy_path_with_inspected_source, FileOperationControls, FileTransferOptions,
    TransferConflictStrategy,
};
use super::super::ensure_replace_target_does_not_contain_source_path;
use super::super::metadata_loss::MetadataLossSummary;
use super::super::transfer_metadata::apply_transfer_metadata;
use super::super::transfer_object::inspect_transfer_source;
use super::super::transfer_strategy::copy_regular_file_payload;
use super::super::transfer_strategy::RegularFilePayloadCopy;
use super::{
    build_source_manifest_with_controls, fingerprint_object_with_controls, inspect_file_identity,
    plan_owned_artifact, recover_owned_artifact, remove_owned_artifact, rename_noreplace,
    verify_source_manifest_with_controls, BackupCreationTransfer, CommitBarrier, CommitPayload,
    CommitTransfer, FileIdentity, FileObjectKind, NoReplaceRenameError, OwnedArtifact,
    OwnedArtifactKind, PreparedTransfer, ProofContext, RecoverableTransferError,
    RecoverableTransferOperation, RecoverableTransferOutcome, SourceManifest, StagedSourceLocation,
    StagingTransfer, TransferCheckpoint, TransferExecutionKind, TransferFingerprint,
    TransferJournalError, TransferJournalRecord, TransferTransition,
};
use crate::transfer_conflict::{first_available_entry_path, transfer_target_metadata_if_exists};
use crate::EntryNameKind;

mod commit;
mod direct_move;
mod merge;
mod recovery;
mod retirement;
mod run;
#[cfg(unix)]
mod staging_parallel;
mod validation;

use commit::{
    advance_committed_transfer, commit_transfer, create_replace_backup, verify_completed_target,
    verify_prepared_source_with_controls,
};
use direct_move::{advance_direct_move_intent, advance_direct_move_renamed};
use merge::{advance_merge_transfer, prepare_merge_transfer};
use recovery::{finish_cancel, finish_failure};
use retirement::{advance_retired_source, retire_source};
pub use run::{
    advance_recoverable_transfer, durable_direct_move_commit, persist_source_manifests,
    run_recoverable_segment, run_recoverable_transfer, settle_failed_recoverable_transfers,
    DurableDirectMoveCommit, TransferSegmentEvent,
};
use validation::validate_checkpoint_semantics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferAdvance {
    Continue,
    Complete(RecoverableTransferOutcome),
}

/// What one step left for its record: a transition the runner commits once the
/// step's effects are durable, or the finished transfer.
enum TransferStep {
    Transition(Box<TransferTransition>),
    Complete(RecoverableTransferOutcome),
}

/// Runs the effects of the record's current checkpoint and returns the next
/// one. Durability of those effects is registered on `barrier`; the caller
/// flushes it before committing the transition.
async fn step_recoverable_transfer(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    barrier: &mut CommitBarrier,
) -> Result<TransferStep, RecoverableTransferError> {
    let proof = ProofContext::new(
        record.request.verification,
        transfer_options.proof_memo.clone(),
    );
    let checkpoint = record.checkpoint.clone();
    validate_checkpoint_semantics(record, &checkpoint)?;
    if checkpoint_accepts_controls(&checkpoint) {
        wait_until_running(transfer_options).await?;
    }
    let transition = match checkpoint {
        TransferCheckpoint::AwaitingManifest => prepare_transfer(record, transfer_options).await?,
        TransferCheckpoint::Merging(merge) => {
            advance_merge_transfer(record, transfer_options, merge, barrier).await?
        }
        TransferCheckpoint::StageCreationIntent(prepared) => {
            let staging_plan = prepared.staging_plan.clone().ok_or_else(|| {
                RecoverableTransferError::InvalidCheckpoint {
                    message: "staging intent has no owned artifact plan".to_owned(),
                }
            })?;
            verify_prepared_source_with_controls(record, &prepared, &transfer_options.controls)
                .await?;
            let artifact = recover_owned_artifact(staging_plan, barrier).await?;
            TransferTransition::Checkpoint(TransferCheckpoint::Staging(StagingTransfer {
                prepared,
                artifact,
            }))
        }
        TransferCheckpoint::Staging(staging) => {
            stage_transfer(record, transfer_options, staging, &proof, barrier).await?
        }
        TransferCheckpoint::DirectMoveIntent(prepared) => {
            advance_direct_move_intent(record, transfer_options, prepared, barrier).await?
        }
        TransferCheckpoint::DirectMoveRenamed(renamed) => {
            advance_direct_move_renamed(renamed, &proof).await?
        }
        TransferCheckpoint::BackupCreationIntent(backup) => {
            create_replace_backup(backup, &proof, barrier).await?
        }
        TransferCheckpoint::CommitIntent(commit) => {
            if path_exists(&commit_payload_path(record, &commit)).await? {
                wait_until_running(transfer_options).await?;
            }
            commit_transfer(record, commit, &proof, barrier).await?
        }
        TransferCheckpoint::TargetCommitted(committed) => {
            advance_committed_transfer(record, committed, &proof, barrier).await?
        }
        TransferCheckpoint::SourceRetirementIntent(retirement) => {
            retire_source(record, *retirement, &proof, barrier).await?
        }
        TransferCheckpoint::SourceRetired(retired) => {
            advance_retired_source(record, *retired, &proof, barrier).await?
        }
        TransferCheckpoint::Completed(completed) => {
            verify_completed_target(&completed, &proof).await?;
            return Ok(TransferStep::Complete(RecoverableTransferOutcome {
                source: record.request.source.clone(),
                final_target: Some(completed.path),
                metadata_loss: completed.metadata_loss,
            }));
        }
        TransferCheckpoint::CancelIntent(previous) => {
            finish_cancel(record, *previous, &proof, barrier).await?
        }
        TransferCheckpoint::Canceled { .. } => return Err(crate::FileError::Cancelled.into()),
        TransferCheckpoint::FailureIntent(failure) => {
            finish_failure(record, failure, &proof, barrier).await?
        }
        TransferCheckpoint::Failed { diagnostic, .. } => {
            return Err(RecoverableTransferError::RecordedFailure { diagnostic });
        }
        TransferCheckpoint::Skipped => {
            return Ok(TransferStep::Complete(RecoverableTransferOutcome {
                source: record.request.source.clone(),
                final_target: None,
                metadata_loss: MetadataLossSummary::default(),
            }));
        }
    };
    Ok(TransferStep::Transition(Box::new(transition)))
}

async fn prepare_transfer(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
) -> Result<TransferTransition, RecoverableTransferError> {
    let mut controls = transfer_options.controls.clone();
    // 红线修订三:manifest 从 journal 恢复(进程曾经中断)必须全文重验;
    // 本进程刚构建的 manifest 距构建仅一个 journal 边界、期间无任何副作用,
    // 同 tick 重扫是纯重复——源稳定性由开工前复核与完工后夹心保证。
    let manifest_was_persisted = record.manifest.is_some();
    let manifest = match &record.manifest {
        Some(manifest) => manifest.clone(),
        None => build_source_manifest_with_controls(&record.request.source, &mut controls).await?,
    };
    let source_fingerprint = match record.request.verification {
        crate::ops::FileOperationVerification::BasicMetadata => None,
        crate::ops::FileOperationVerification::Strong => {
            Some(fingerprint_object_with_controls(&record.request.source, &controls).await?)
        }
    };
    if manifest_was_persisted {
        verify_source_manifest_with_controls(&manifest, &mut controls).await?;
    }
    let source_identity = manifest_root_identity(&manifest)?.clone();
    let requested_target_identity =
        inspect_optional_identity(&record.request.requested_target).await?;

    if record.request.source == record.request.requested_target
        && record.request.conflict_strategy != TransferConflictStrategy::KeepBoth
    {
        return Ok(TransferTransition::Checkpoint(TransferCheckpoint::Skipped));
    }

    if record.request.conflict_strategy == TransferConflictStrategy::Merge {
        if let Some(target_identity) = requested_target_identity.as_ref() {
            if source_identity.object_kind == FileObjectKind::Directory
                && target_identity.object_kind == FileObjectKind::Directory
            {
                return prepare_merge_transfer(record, manifest, target_identity.clone()).await;
            }
            return Ok(TransferTransition::Checkpoint(TransferCheckpoint::Skipped));
        }
    }

    let (
        resolved_target,
        expected_target_identity,
        expected_target_fingerprint,
        replacement_manifest,
    ) = match requested_target_identity {
        None => (record.request.requested_target.clone(), None, None, None),
        Some(target_identity) => match record.request.conflict_strategy {
            TransferConflictStrategy::Fail => {
                return Err(RecoverableTransferError::TargetConflict {
                    path: record.request.requested_target.clone(),
                });
            }
            TransferConflictStrategy::Skip => {
                return Ok(TransferTransition::Checkpoint(TransferCheckpoint::Skipped));
            }
            TransferConflictStrategy::KeepBoth => (
                first_available_entry_path(
                    &record.request.requested_target,
                    EntryNameKind::from(source_identity.object_kind),
                )
                .await?,
                None,
                None,
                None,
            ),
            TransferConflictStrategy::Replace => {
                let metadata = fs::symlink_metadata(&record.request.requested_target)
                    .await
                    .map_err(|source| {
                        RecoverableTransferError::file_system(
                            "read replace target metadata for",
                            &record.request.requested_target,
                            source,
                        )
                    })?;
                ensure_replace_target_does_not_contain_source_path(
                    &record.request.source,
                    &record.request.requested_target,
                    &metadata,
                )?;
                let replacement_manifest = build_source_manifest_with_controls(
                    &record.request.requested_target,
                    &mut controls,
                )
                .await?;
                let fingerprint = match record.request.verification {
                    crate::ops::FileOperationVerification::BasicMetadata => None,
                    crate::ops::FileOperationVerification::Strong => Some(
                        fingerprint_object_with_controls(
                            &record.request.requested_target,
                            &controls,
                        )
                        .await?,
                    ),
                };
                verify_source_manifest_with_controls(&replacement_manifest, &mut controls).await?;
                if manifest_root_identity(&replacement_manifest)? != &target_identity {
                    return Err(RecoverableTransferError::TargetConflict {
                        path: record.request.requested_target.clone(),
                    });
                }
                (
                    record.request.requested_target.clone(),
                    Some(target_identity),
                    fingerprint,
                    Some(replacement_manifest),
                )
            }
            TransferConflictStrategy::Merge => {
                return Err(RecoverableTransferError::InvalidCheckpoint {
                    message: "directory merge requires child journal expansion".to_owned(),
                });
            }
        },
    };

    let execution = match (record.request.operation, record.request.verification) {
        (RecoverableTransferOperation::Copy, _) => TransferExecutionKind::CopyToStage,
        (RecoverableTransferOperation::Move, crate::FileOperationVerification::BasicMetadata)
            if expected_target_identity.is_none() =>
        {
            TransferExecutionKind::MoveDirect
        }
        (RecoverableTransferOperation::Move, _) if expected_target_identity.is_some() => {
            TransferExecutionKind::MoveToStage
        }
        (RecoverableTransferOperation::Move, crate::FileOperationVerification::Strong) => {
            TransferExecutionKind::MoveDirect
        }
        (RecoverableTransferOperation::Move, crate::FileOperationVerification::BasicMetadata) => {
            unreachable!("basic move with a replacement target must use staging")
        }
    };
    let staging_plan = match execution {
        TransferExecutionKind::CopyToStage | TransferExecutionKind::MoveToStage => {
            let parent = target_parent(&resolved_target)?;
            Some(plan_owned_artifact(
                parent,
                OwnedArtifactKind::TargetStaging,
                record.owner(0),
            )?)
        }
        TransferExecutionKind::MoveDirect => None,
        TransferExecutionKind::MergeDirectory => unreachable!(),
    };
    let prepared = PreparedTransfer {
        source_identity,
        resolved_target,
        expected_target_identity,
        expected_target_fingerprint,
        source_fingerprint,
        execution,
        staging_plan,
    };
    let checkpoint = match (execution, record.request.verification) {
        (TransferExecutionKind::MoveDirect, crate::FileOperationVerification::BasicMetadata) => {
            TransferCheckpoint::DirectMoveIntent(prepared)
        }
        (TransferExecutionKind::MoveDirect, crate::FileOperationVerification::Strong) => {
            TransferCheckpoint::CommitIntent(CommitTransfer {
                prepared: prepared.clone(),
                payload: CommitPayload::DirectSource {
                    identity: prepared.source_identity.clone(),
                },
                fingerprint: TransferFingerprint::Blake3(prepared.source_fingerprint.ok_or_else(
                    || RecoverableTransferError::InvalidCheckpoint {
                        message: "direct move has no preflight content fingerprint".to_owned(),
                    },
                )?),
                backup_identity: None,
                // 同盘 rename 不改动元数据,没有可丢的。
                metadata_loss: MetadataLossSummary::default(),
            })
        }
        (TransferExecutionKind::CopyToStage | TransferExecutionKind::MoveToStage, _) => {
            TransferCheckpoint::StageCreationIntent(prepared)
        }
        (TransferExecutionKind::MergeDirectory, _) => unreachable!(),
    };
    Ok(TransferTransition::ManifestAndCheckpoint {
        manifest,
        replacement_manifest,
        checkpoint,
    })
}

async fn stage_transfer(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    staging: StagingTransfer,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    super::validate_owned_artifact(&staging.artifact).await?;
    let payload_path = staging.artifact.plan.payload_path();
    let source_exists = path_exists(&record.request.source).await?;
    let payload_exists = path_exists(&payload_path).await?;

    let kernel_clone_staging = !payload_exists
        && kernel_clone_staging_candidate(record, &staging)
        && transfer_options.strategy_engine.is_some();
    let mut kernel_clone_used = false;
    // 暂存时发现的元数据问题;rename 进暂存区的对象元数据原样保留,没有可丢的。
    let (source_location, staged_metadata_loss) = if payload_exists {
        let payload_identity = inspect_file_identity(&payload_path).await?;
        if staging.prepared.execution == TransferExecutionKind::MoveToStage
            && !source_exists
            && payload_identity.same_object(&staging.prepared.source_identity)
        {
            (
                StagedSourceLocation::ArtifactPayload,
                MetadataLossSummary::default(),
            )
        } else if source_exists {
            remove_owned_artifact(&staging.artifact, barrier).await?;
            let mut prepared = staging.prepared;
            prepared.staging_plan = Some(plan_owned_artifact(
                target_parent(&prepared.resolved_target)?,
                OwnedArtifactKind::TargetStaging,
                record.owner(0),
            )?);
            return Ok(TransferTransition::Checkpoint(
                TransferCheckpoint::StageCreationIntent(prepared),
            ));
        } else {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "staged payload exists but no matching source object remains".to_owned(),
            });
        }
    } else {
        if !source_exists {
            return Err(RecoverableTransferError::SourceChanged {
                path: record.request.source.clone(),
            });
        }
        verify_prepared_source_with_controls(record, &staging.prepared, &transfer_options.controls)
            .await?;
        match staging.prepared.execution {
            TransferExecutionKind::CopyToStage if kernel_clone_staging => {
                // Basic 单文件同盘复制:优先 FICLONE 原子克隆直达 payload。
                // 只有策略确实命中克隆时才允许 KernelClone 证明;阶梯降级
                // (内核快拷/用户态循环)都是真实数据搬运,必须回退全文哈希。
                let mut controls = transfer_options.controls.clone();
                let engine = transfer_options
                    .strategy_engine
                    .as_ref()
                    .expect("clone candidacy requires an engine");
                let payload_outcome = copy_regular_file_payload(RegularFilePayloadCopy {
                    source: &record.request.source,
                    target: &payload_path,
                    source_device: staging.prepared.source_identity.device,
                    bytes_total: staging.prepared.source_identity.size,
                    engine,
                    controls: &mut controls,
                    progress: transfer_options.progress.as_ref(),
                    source_hasher: None,
                })
                .await
                .map_err(RecoverableTransferError::FileOperation)?;
                kernel_clone_used = payload_outcome.strategy
                    == crate::ops::transfer_strategy::PayloadCopyStrategy::KernelClone;
                // 快照比对按源 mtime(2 秒桶)校验,FICLONE 只克隆数据块,
                // payload 的时间戳仍是克隆时刻——必须像普通路径一样在提交
                // 前恢复源元数据,保真与快照校验都依赖它。
                let clone_source = inspect_transfer_source(&record.request.source).await?;
                let lost =
                    apply_transfer_metadata(&record.request.source, &payload_path, &clone_source)
                        .await?;
                let mut metadata_loss = MetadataLossSummary::default();
                metadata_loss.record(&payload_path, lost);
                (StagedSourceLocation::OriginalPath, metadata_loss)
            }
            TransferExecutionKind::CopyToStage => (
                StagedSourceLocation::OriginalPath,
                copy_to_payload(record, transfer_options, &payload_path).await?,
            ),
            TransferExecutionKind::MoveToStage => {
                match rename_noreplace(&record.request.source, &payload_path) {
                    Ok(()) => {
                        barrier.require_rename(&record.request.source, &payload_path);
                        (
                            StagedSourceLocation::ArtifactPayload,
                            MetadataLossSummary::default(),
                        )
                    }
                    Err(NoReplaceRenameError::CrossDevice) => (
                        StagedSourceLocation::OriginalPath,
                        copy_to_payload(record, transfer_options, &payload_path).await?,
                    ),
                    Err(error) => {
                        return Err(
                            error.into_transfer_error(&record.request.source, &payload_path)
                        );
                    }
                }
            }
            TransferExecutionKind::MoveDirect | TransferExecutionKind::MergeDirectory => {
                return Err(RecoverableTransferError::InvalidCheckpoint {
                    message: "invalid execution kind for staging".to_owned(),
                });
            }
        }
    };

    if source_location == StagedSourceLocation::OriginalPath {
        verify_prepared_source_with_controls(record, &staging.prepared, &transfer_options.controls)
            .await?;
    }
    barrier.require_parent_of(&payload_path);
    let payload_identity = inspect_file_identity(&payload_path).await?;
    // 克隆 staging 的证明 = 内核原子克隆 + 此处 payload 身份与后续夹心;
    // 其余路径按"身份→哈希→身份"夹心产出 Blake3 证明(Basic 走运行内 memo)。
    let payload_fingerprint = if kernel_clone_used {
        TransferFingerprint::KernelClone {
            payload_identity: payload_identity.clone(),
        }
    } else {
        let fingerprint = proof
            .fingerprint_object_with_controls(&payload_path, &transfer_options.controls)
            .await?;
        TransferFingerprint::Blake3(fingerprint)
    };
    if inspect_file_identity(&payload_path).await? != payload_identity {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    match (&payload_fingerprint, staging.prepared.source_fingerprint) {
        (TransferFingerprint::Blake3(fingerprint), Some(expected_fingerprint)) => {
            if fingerprint != &expected_fingerprint {
                return Err(RecoverableTransferError::FingerprintMismatch { path: payload_path });
            }
        }
        (TransferFingerprint::KernelClone { .. }, Some(_)) => {
            return Err(RecoverableTransferError::FingerprintMismatch { path: payload_path });
        }
        _ => {}
    }
    if !payload_identity.matches_staging_snapshot(&staging.prepared.source_identity) {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    let metadata_loss =
        staged_metadata_loss.rebased(&payload_path, &staging.prepared.resolved_target);
    let payload = CommitPayload::Artifact {
        artifact: staging.artifact,
        payload_identity,
        source_location,
    };
    let checkpoint = if staging.prepared.expected_target_identity.is_some() {
        // Replace 备份证明永远要求全文哈希;克隆 staging 不进入该分支
        // (候选条件已排除 Replace),此处还原 Blake3 即可。
        TransferCheckpoint::BackupCreationIntent(BackupCreationTransfer {
            prepared: staging.prepared,
            payload,
            fingerprint: payload_fingerprint.as_blake3().ok_or_else(|| {
                RecoverableTransferError::InvalidCheckpoint {
                    message: "replace staging produced a non-content fingerprint".to_owned(),
                }
            })?,
            metadata_loss,
        })
    } else {
        TransferCheckpoint::CommitIntent(CommitTransfer {
            prepared: staging.prepared,
            payload,
            fingerprint: payload_fingerprint,
            backup_identity: None,
            metadata_loss,
        })
    };
    Ok(TransferTransition::Checkpoint(checkpoint))
}

/// KernelClone staging 的候选条件:Basic 校验、顶层普通文件、Copy 且无
/// Replace 备份。目录树、Merge、Move、Strong、Replace 一律走原路径。
fn kernel_clone_staging_candidate(
    record: &TransferJournalRecord,
    staging: &StagingTransfer,
) -> bool {
    staging.prepared.execution == TransferExecutionKind::CopyToStage
        && record.request.operation == RecoverableTransferOperation::Copy
        && record.request.verification == crate::ops::FileOperationVerification::BasicMetadata
        && staging.prepared.expected_target_identity.is_none()
        && staging.prepared.source_identity.object_kind == FileObjectKind::RegularFile
        && staging.prepared.source_fingerprint.is_none()
}

async fn copy_to_payload(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    payload_path: &Path,
) -> Result<MetadataLossSummary, RecoverableTransferError> {
    let source_object = inspect_transfer_source(&record.request.source).await?;

    // 目录树且有引擎:并行 staging(两阶段);其余保持原串行路径。
    #[cfg(unix)]
    if matches!(
        source_object.kind,
        crate::ops::TransferSourceKind::Directory
    ) {
        if let Some(engine) = transfer_options.strategy_engine.as_ref() {
            return staging_parallel::copy_tree_to_payload_parallel(
                &record.request.source,
                payload_path,
                engine,
                &transfer_options.controls,
                transfer_options.progress.as_ref(),
            )
            .await;
        }
    }

    let options = FileTransferOptions::new(transfer_options.controls.clone())
        .with_optional_progress(transfer_options.progress.clone())
        .with_conflict_strategy(TransferConflictStrategy::Fail)
        .with_verification(record.request.verification);
    let copied = copy_path_with_inspected_source(
        &record.request.source,
        payload_path,
        &source_object,
        options,
    )
    .await?;
    if copied.target.as_deref() != Some(payload_path) {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "staging copy returned no target".to_owned(),
        });
    }
    Ok(copied.metadata_loss)
}

fn record_manifest(
    record: &TransferJournalRecord,
) -> Result<&SourceManifest, RecoverableTransferError> {
    record
        .manifest
        .as_ref()
        .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "checkpoint requires a source manifest".to_owned(),
        })
}

fn record_replacement_manifest(
    record: &TransferJournalRecord,
) -> Result<&SourceManifest, RecoverableTransferError> {
    record.replacement_manifest.as_ref().ok_or_else(|| {
        RecoverableTransferError::InvalidCheckpoint {
            message: "checkpoint requires a replacement target manifest".to_owned(),
        }
    })
}

fn manifest_root_identity(
    manifest: &SourceManifest,
) -> Result<&FileIdentity, RecoverableTransferError> {
    manifest
        .entries
        .iter()
        .find(|entry| entry.relative_path.as_os_str().is_empty())
        .map(|entry| &entry.identity)
        .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "source manifest has no root identity".to_owned(),
        })
}

fn commit_payload_path(record: &TransferJournalRecord, commit: &CommitTransfer) -> PathBuf {
    match &commit.payload {
        CommitPayload::DirectSource { .. } => record.request.source.clone(),
        CommitPayload::Artifact { artifact, .. } => artifact.plan.payload_path(),
    }
}

fn commit_artifact(commit: &CommitTransfer) -> Result<&OwnedArtifact, RecoverableTransferError> {
    match &commit.payload {
        CommitPayload::Artifact { artifact, .. } => Ok(artifact),
        CommitPayload::DirectSource { .. } => Err(RecoverableTransferError::InvalidCheckpoint {
            message: "replace commit has no owned backup artifact".to_owned(),
        }),
    }
}

async fn inspect_optional_identity(
    path: &Path,
) -> Result<Option<FileIdentity>, RecoverableTransferError> {
    match transfer_target_metadata_if_exists(path).await {
        Ok(Some(_)) => inspect_file_identity(path).await.map(Some),
        Ok(None) => Ok(None),
        Err(source) => Err(RecoverableTransferError::file_system(
            "read transfer target metadata for",
            path,
            source,
        )),
    }
}

async fn path_exists(path: &Path) -> Result<bool, RecoverableTransferError> {
    Ok(inspect_optional_identity(path).await?.is_some())
}

fn target_parent(path: &Path) -> Result<&Path, RecoverableTransferError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: format!("path has no usable parent: {path:?}"),
        })
}

async fn next_recovered_path(path: &Path) -> Result<PathBuf, RecoverableTransferError> {
    for sequence in 1..=10_000 {
        let candidate = super::rename::recovered_name_candidate(path, sequence);
        if !path_exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    Err(RecoverableTransferError::TargetConflict {
        path: path.to_path_buf(),
    })
}

pub(super) fn checkpoint_requires_forward_recovery(checkpoint: &TransferCheckpoint) -> bool {
    matches!(
        checkpoint,
        TransferCheckpoint::DirectMoveRenamed(_)
            | TransferCheckpoint::BackupCreationIntent(_)
            | TransferCheckpoint::TargetCommitted(_)
            | TransferCheckpoint::SourceRetirementIntent(_)
            | TransferCheckpoint::SourceRetired(_)
            | TransferCheckpoint::CancelIntent(_)
            | TransferCheckpoint::FailureIntent(_)
    )
}

fn checkpoint_accepts_controls(checkpoint: &TransferCheckpoint) -> bool {
    match checkpoint {
        TransferCheckpoint::AwaitingManifest
        | TransferCheckpoint::Merging(_)
        | TransferCheckpoint::StageCreationIntent(_)
        | TransferCheckpoint::Staging(_) => true,
        TransferCheckpoint::DirectMoveIntent(_)
        | TransferCheckpoint::DirectMoveRenamed(_)
        | TransferCheckpoint::BackupCreationIntent(_)
        | TransferCheckpoint::CommitIntent(_)
        | TransferCheckpoint::TargetCommitted(_)
        | TransferCheckpoint::SourceRetirementIntent(_)
        | TransferCheckpoint::SourceRetired(_)
        | TransferCheckpoint::Completed(_)
        | TransferCheckpoint::CancelIntent(_)
        | TransferCheckpoint::Canceled { .. }
        | TransferCheckpoint::FailureIntent(_)
        | TransferCheckpoint::Failed { .. }
        | TransferCheckpoint::Skipped => false,
    }
}

async fn wait_until_running(
    transfer_options: &FileTransferOptions,
) -> Result<(), RecoverableTransferError> {
    let mut controls: FileOperationControls = transfer_options.controls.clone();
    controls.wait_until_running().await.map_err(Into::into)
}

pub(super) fn validate_next_revision(
    current: u64,
    next: u64,
) -> Result<(), RecoverableTransferError> {
    if current.checked_add(1) == Some(next) {
        Ok(())
    } else {
        Err(RecoverableTransferError::Journal {
            message: format!("journal returned revision {next} after {current}"),
        })
    }
}

pub(super) fn journal_error(error: TransferJournalError) -> RecoverableTransferError {
    match error {
        TransferJournalError::UserCancelled => {
            RecoverableTransferError::FileOperation(crate::FileError::Cancelled)
        }
        TransferJournalError::ApplicationStopping => {
            RecoverableTransferError::FileOperation(crate::FileError::ApplicationStopping)
        }
        TransferJournalError::StaleRevision => RecoverableTransferError::Journal {
            message: "stale transfer revision".to_owned(),
        },
        TransferJournalError::Storage(message) => RecoverableTransferError::Journal { message },
    }
}
