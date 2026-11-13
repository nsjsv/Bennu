use std::io;
use std::path::PathBuf;

use tokio::fs;

use super::commit::verify_completed_target;
use super::run::apply_transition;
use super::{record_manifest, step_recoverable_transfer, TransferStep};
use crate::ops::metadata_loss::MetadataLossSummary;
use crate::ops::recoverable_transfer::CommitBarrier;
use crate::ops::recoverable_transfer::{
    inspect_file_identity, verify_source_manifest, CompletedTarget, FileIdentity,
    MergeChildCompletion, MergeChildOutcome, MergeTransfer, ProofContext, RecoverableTransferError,
    RecoverableTransferOperation, RecoverableTransferRequest, SourceManifest, TransferCheckpoint,
    TransferFingerprint, TransferJournalRecord, TransferTransition, TransferWorkKey,
};
use crate::{FileTransferOptions, TransferConflictStrategy};

pub(super) async fn prepare_merge_transfer(
    record: &TransferJournalRecord,
    manifest: SourceManifest,
    target_identity: FileIdentity,
) -> Result<TransferTransition, RecoverableTransferError> {
    if record
        .request
        .requested_target
        .starts_with(&record.request.source)
    {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "cannot merge a directory into itself".to_owned(),
        });
    }
    let child_names = merge_child_names(&manifest);
    Ok(TransferTransition::ManifestAndCheckpoint {
        manifest,
        replacement_manifest: None,
        checkpoint: TransferCheckpoint::Merging(MergeTransfer {
            target_root_identity: target_identity,
            next_child: 0,
            active_child: None,
            child_names,
            completed_children: Vec::new(),
            completed_prefix_verified: true,
        }),
    })
}

pub(super) async fn advance_merge_transfer(
    record: &TransferJournalRecord,
    transfer_options: &FileTransferOptions,
    mut merge: MergeTransfer,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    let proof = ProofContext::new(
        record.request.verification,
        transfer_options.proof_memo.clone(),
    );
    let current_target_identity = inspect_file_identity(&record.request.requested_target).await?;
    if !current_target_identity.same_object(&merge.target_root_identity) {
        return Err(RecoverableTransferError::TargetConflict {
            path: record.request.requested_target.clone(),
        });
    }
    if merge.child_names.is_empty() {
        merge.child_names = merge_child_names(record_manifest(record)?);
    }
    if !merge.completed_prefix_verified {
        verify_completed_prefix(record, &merge, &proof).await?;
        merge.completed_prefix_verified = true;
    }
    if merge.next_child < merge.child_names.len() {
        if merge.active_child.is_none() {
            let child_name = merge.child_names[merge.next_child].clone();
            merge.active_child = Some(Box::new(merge_child_record(record, child_name)));
        }
        let child = merge.active_child.as_mut().ok_or_else(|| {
            RecoverableTransferError::InvalidCheckpoint {
                message: "merge cursor has no active child".to_owned(),
            }
        })?;
        if child.manifest.is_none() {
            child.manifest = Some(merge_child_manifest(record_manifest(record)?, child)?);
        }
        if matches!(child.checkpoint, TransferCheckpoint::AwaitingManifest) {
            verify_source_manifest(record_manifest(child)?).await?;
        }
        let child_step =
            Box::pin(step_recoverable_transfer(child, transfer_options, barrier)).await?;
        // 子记录嵌在父 checkpoint 里，不单独提交：它的转移随父记录这一次提交落盘。
        let completion = match child_step {
            TransferStep::Transition(transition) => {
                let transition = *transition;
                let nested_completion = match &transition {
                    TransferTransition::MergeCompletionAndCheckpoint { completion, .. } => {
                        Some(completion.clone())
                    }
                    _ => None,
                };
                child.revision += 1;
                apply_transition(child, transition);
                nested_completion
            }
            TransferStep::Complete(_) => {
                let completion = terminal_child_completion(&record.key, child)?;
                merge.active_child = None;
                merge.next_child += 1;
                merge.completed_children.push(completion.clone());
                Some(completion)
            }
        };
        let checkpoint = TransferCheckpoint::Merging(merge);
        return Ok(match completion {
            Some(completion) => TransferTransition::MergeCompletionAndCheckpoint {
                completion,
                checkpoint,
            },
            None => TransferTransition::Checkpoint(checkpoint),
        });
    }
    if merge.active_child.is_some() {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "merge cursor passed the manifest while a child is still active".to_owned(),
        });
    }
    verify_completed_prefix(record, &merge, &proof).await?;

    if record.request.operation == RecoverableTransferOperation::Copy {
        verify_source_manifest(record_manifest(record)?).await?;
    } else {
        match fs::remove_dir(&record.request.source).await {
            Ok(()) => barrier.require_parent_of(&record.request.source),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
                ) => {}
            Err(source) => {
                return Err(RecoverableTransferError::file_system(
                    "remove merged source directory",
                    &record.request.source,
                    source,
                ));
            }
        }
    }
    let path = record.request.requested_target.clone();
    // Merge 顶层完成事实保持 Blake3(Merge 不走克隆);Basic 子项的哈希
    // 已在各子 advance 内经 memo 去重。
    let proof = ProofContext::new(
        record.request.verification,
        transfer_options.proof_memo.clone(),
    );
    // 根是已存在的目标目录,按规范不覆盖它的元数据,摘要只来自各子项。
    let mut metadata_loss = MetadataLossSummary::default();
    for completion in &merge.completed_children {
        if let MergeChildOutcome::Committed(completed) = &completion.outcome {
            metadata_loss.absorb(completed.metadata_loss.clone());
        }
    }
    let completed = CompletedTarget {
        identity: inspect_file_identity(&path).await?,
        fingerprint: TransferFingerprint::Blake3(proof.fingerprint_object(&path).await?),
        path,
        metadata_loss,
    };
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::Completed(completed),
    ))
}

fn merge_child_names(manifest: &SourceManifest) -> Vec<PathBuf> {
    let mut child_names = manifest
        .entries
        .iter()
        .filter_map(|entry| entry.relative_path.iter().next().map(PathBuf::from))
        .collect::<Vec<_>>();
    child_names.sort();
    child_names.dedup();
    child_names
}

fn merge_child_record(
    parent: &TransferJournalRecord,
    child_name: PathBuf,
) -> TransferJournalRecord {
    TransferJournalRecord {
        task_id: parent.task_id,
        key: TransferWorkKey {
            transfer_index: parent.key.transfer_index,
            relative_path: parent.key.relative_path.join(&child_name),
        },
        request: RecoverableTransferRequest {
            source: parent.request.source.join(&child_name),
            requested_target: parent.request.requested_target.join(child_name),
            operation: parent.request.operation,
            conflict_strategy: TransferConflictStrategy::Merge,
            verification: parent.request.verification,
        },
        checkpoint: TransferCheckpoint::AwaitingManifest,
        revision: 0,
        manifest: None,
        replacement_manifest: None,
    }
}

fn merge_child_manifest(
    parent_manifest: &SourceManifest,
    child: &TransferJournalRecord,
) -> Result<SourceManifest, RecoverableTransferError> {
    let child_name = child
        .request
        .source
        .strip_prefix(&parent_manifest.root)
        .map_err(|_| RecoverableTransferError::InvalidCheckpoint {
            message: "merge child source is outside the parent manifest".to_owned(),
        })?;
    let entries = parent_manifest
        .entries
        .iter()
        .filter_map(|entry| {
            entry
                .relative_path
                .strip_prefix(child_name)
                .ok()
                .map(|relative_path| crate::SourceManifestEntry {
                    relative_path: relative_path.to_path_buf(),
                    identity: entry.identity.clone(),
                })
        })
        .collect::<Vec<_>>();
    if entries
        .first()
        .is_none_or(|entry| !entry.relative_path.as_os_str().is_empty())
    {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "merge child is missing from the parent manifest".to_owned(),
        });
    }
    Ok(SourceManifest {
        root: child.request.source.clone(),
        entries,
    })
}

async fn verify_completed_prefix(
    record: &TransferJournalRecord,
    merge: &MergeTransfer,
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    if merge.next_child > merge.child_names.len()
        || merge.completed_children.len() != merge.next_child
    {
        return Err(RecoverableTransferError::InvalidCheckpoint {
            message: "merge completion facts do not match the durable cursor".to_owned(),
        });
    }
    for (child_name, completion) in merge
        .child_names
        .iter()
        .take(merge.next_child)
        .zip(&merge.completed_children)
    {
        let expected_child_key = TransferWorkKey {
            transfer_index: record.key.transfer_index,
            relative_path: record.key.relative_path.join(child_name),
        };
        if completion.parent_key != record.key || completion.child_key != expected_child_key {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "merge completion fact does not match its manifest child".to_owned(),
            });
        }
        if let MergeChildOutcome::Committed(completed) = &completion.outcome {
            verify_completed_target(completed, proof).await?;
        }
    }
    Ok(())
}

fn terminal_child_completion(
    parent_key: &TransferWorkKey,
    child: &TransferJournalRecord,
) -> Result<MergeChildCompletion, RecoverableTransferError> {
    let outcome = match &child.checkpoint {
        TransferCheckpoint::Completed(completed) => {
            MergeChildOutcome::Committed(Box::new(completed.clone()))
        }
        TransferCheckpoint::Skipped => MergeChildOutcome::Skipped,
        _ => {
            return Err(RecoverableTransferError::InvalidCheckpoint {
                message: "merge child completed without a terminal checkpoint".to_owned(),
            });
        }
    };
    Ok(MergeChildCompletion {
        parent_key: parent_key.clone(),
        child_key: child.key.clone(),
        outcome,
    })
}
