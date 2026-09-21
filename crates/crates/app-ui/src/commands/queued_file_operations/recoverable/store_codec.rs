use std::collections::BTreeMap;

use file_core::{
    FileIdentity, FileObjectKind, FileOperationVerification, MergeChildCompletion,
    RecoverableTransferOperation, RecoverableTransferRequest, SourceManifest, SourceManifestEntry,
    TransferCheckpoint, TransferConflictStrategy, TransferJournalRecord, TransferWorkKey,
};
use file_operation_store::{
    StoreError, StoreResult, StoredFileIdentity, StoredFileObjectKind,
    StoredFileOperationVerification, StoredManifestEntry, StoredMergeChildCompletion, StoredPath,
    StoredTransferCheckpoint, StoredTransferCheckpointKind, StoredTransferConflictStrategy,
    StoredTransferOperation, StoredTransferRecoverySnapshot, StoredTransferWorkKey,
};

pub(super) fn encode_work_key(key: &TransferWorkKey) -> StoredTransferWorkKey {
    StoredTransferWorkKey {
        transfer_index: key.transfer_index,
        relative_path: StoredPath::from_path(&key.relative_path),
    }
}

pub(super) fn encode_checkpoint(
    checkpoint: &TransferCheckpoint,
) -> StoreResult<StoredTransferCheckpoint> {
    StoredTransferCheckpoint::new(
        checkpoint_kind(checkpoint),
        serde_json::to_string(checkpoint)?,
    )
}

pub(super) fn encode_manifest_entries_while(
    transfer_index: u64,
    manifest: &SourceManifest,
    mut continue_encoding: impl FnMut() -> bool,
) -> Option<Vec<StoredManifestEntry>> {
    let mut entries = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        if !continue_encoding() {
            return None;
        }
        entries.push(StoredManifestEntry {
            transfer_index,
            relative_path: StoredPath::from_path(&entry.relative_path),
            identity: encode_identity(&entry.identity),
        });
    }
    Some(entries)
}

pub(super) fn encode_merge_completion(
    completion: &MergeChildCompletion,
) -> StoreResult<StoredMergeChildCompletion> {
    Ok(StoredMergeChildCompletion {
        transfer_index: completion.child_key.transfer_index,
        child_relative_path: StoredPath::from_path(&completion.child_key.relative_path),
        completion_json: serde_json::to_string(completion)?,
    })
}

pub(super) fn decode_recovery_snapshot(
    task_id: u64,
    snapshot: StoredTransferRecoverySnapshot,
) -> StoreResult<Vec<TransferJournalRecord>> {
    let mut journal_indices = BTreeMap::new();
    for entry in &snapshot.journal_entries {
        if !entry.key.relative_path.to_path_buf().as_os_str().is_empty() {
            return invalid("SQLite transfer journal contains a non-top-level work key");
        }
        if journal_indices
            .insert(entry.key.transfer_index, ())
            .is_some()
        {
            return invalid("SQLite transfer journal contains duplicate transfer indices");
        }
    }

    let mut completions_by_transfer = BTreeMap::<u64, Vec<MergeChildCompletion>>::new();
    for stored_completion in snapshot.merge_completions {
        let completion: MergeChildCompletion =
            serde_json::from_str(&stored_completion.completion_json)?;
        let child_suffix = completion
            .child_key
            .relative_path
            .strip_prefix(&completion.parent_key.relative_path)
            .ok();
        if !journal_indices.contains_key(&stored_completion.transfer_index)
            || completion.parent_key.transfer_index != stored_completion.transfer_index
            || completion.child_key.transfer_index != stored_completion.transfer_index
            || completion.child_key.relative_path
                != stored_completion.child_relative_path.to_path_buf()
            || child_suffix.is_none_or(|suffix| suffix.components().count() != 1)
        {
            return invalid("merge completion has no matching transfer journal");
        }
        completions_by_transfer
            .entry(stored_completion.transfer_index)
            .or_default()
            .push(completion);
    }
    for completions in completions_by_transfer.values_mut() {
        completions.sort_by(|left, right| {
            left.child_key
                .relative_path
                .cmp(&right.child_key.relative_path)
        });
    }

    let mut manifests_by_transfer = decode_manifests(
        snapshot.manifest_entries,
        &journal_indices,
        "transfer manifest has no matching journal entry",
    )?;
    let mut replacement_manifests_by_transfer = decode_manifests(
        snapshot.replacement_manifest_entries,
        &journal_indices,
        "replacement manifest has no matching journal entry",
    )?;

    let mut records = Vec::with_capacity(snapshot.journal_entries.len());
    for entry in snapshot.journal_entries {
        let source = entry.source.to_path_buf();
        let requested_target = entry.requested_target.to_path_buf();
        let manifest = manifests_by_transfer
            .remove(&entry.key.transfer_index)
            .filter(|entries| !entries.is_empty())
            .map(|mut entries| {
                entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
                SourceManifest {
                    root: source.clone(),
                    entries,
                }
            });
        let replacement_manifest = replacement_manifests_by_transfer
            .remove(&entry.key.transfer_index)
            .filter(|entries| !entries.is_empty())
            .map(|mut entries| {
                entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
                SourceManifest {
                    root: requested_target.clone(),
                    entries,
                }
            });
        let mut checkpoint = decode_checkpoint(&entry.checkpoint)?;
        if let Some(completions) = completions_by_transfer.remove(&entry.key.transfer_index) {
            attach_merge_completions(&mut checkpoint, &entry.key, &completions);
        }
        records.push(TransferJournalRecord {
            task_id,
            key: TransferWorkKey {
                transfer_index: entry.key.transfer_index,
                relative_path: entry.key.relative_path.to_path_buf(),
            },
            request: RecoverableTransferRequest {
                source,
                requested_target,
                operation: decode_operation(entry.operation),
                conflict_strategy: decode_conflict_strategy(entry.conflict_strategy),
                verification: decode_verification(entry.verification),
            },
            checkpoint,
            revision: entry.revision,
            manifest,
            replacement_manifest,
        });
    }
    Ok(records)
}

fn decode_manifests(
    entries: Vec<StoredManifestEntry>,
    journal_indices: &BTreeMap<u64, ()>,
    missing_journal_message: &'static str,
) -> StoreResult<BTreeMap<u64, Vec<SourceManifestEntry>>> {
    let mut manifests = BTreeMap::<u64, Vec<SourceManifestEntry>>::new();
    for entry in entries {
        if !journal_indices.contains_key(&entry.transfer_index) {
            return invalid(missing_journal_message);
        }
        manifests
            .entry(entry.transfer_index)
            .or_default()
            .push(SourceManifestEntry {
                relative_path: entry.relative_path.to_path_buf(),
                identity: decode_identity(entry.identity),
            });
    }
    Ok(manifests)
}

fn decode_checkpoint(checkpoint: &StoredTransferCheckpoint) -> StoreResult<TransferCheckpoint> {
    let decoded: TransferCheckpoint = serde_json::from_str(&checkpoint.state_json)?;
    if checkpoint.kind != checkpoint_kind(&decoded) {
        return Err(StoreError::InvalidTransferValue {
            field: "checkpoint_kind",
            value: checkpoint.kind.serde_state_name().to_owned(),
        });
    }
    Ok(decoded)
}

fn checkpoint_kind(checkpoint: &TransferCheckpoint) -> StoredTransferCheckpointKind {
    match checkpoint {
        TransferCheckpoint::AwaitingManifest => StoredTransferCheckpointKind::AwaitingManifest,
        TransferCheckpoint::Merging(_) => StoredTransferCheckpointKind::Merging,
        TransferCheckpoint::StageCreationIntent(_) => {
            StoredTransferCheckpointKind::StageCreationIntent
        }
        TransferCheckpoint::Staging(_) => StoredTransferCheckpointKind::Staging,
        TransferCheckpoint::DirectMoveIntent(_) => StoredTransferCheckpointKind::DirectMoveIntent,
        TransferCheckpoint::DirectMoveRenamed(_) => StoredTransferCheckpointKind::DirectMoveRenamed,
        TransferCheckpoint::BackupCreationIntent(_) => {
            StoredTransferCheckpointKind::BackupCreationIntent
        }
        TransferCheckpoint::CommitIntent(_) => StoredTransferCheckpointKind::CommitIntent,
        TransferCheckpoint::TargetCommitted(_) => StoredTransferCheckpointKind::TargetCommitted,
        TransferCheckpoint::SourceRetirementIntent(_) => {
            StoredTransferCheckpointKind::SourceRetirementIntent
        }
        TransferCheckpoint::SourceRetired(_) => StoredTransferCheckpointKind::SourceRetired,
        TransferCheckpoint::Completed(_) => StoredTransferCheckpointKind::Completed,
        TransferCheckpoint::CancelIntent(_) => StoredTransferCheckpointKind::CancelIntent,
        TransferCheckpoint::Canceled { .. } => StoredTransferCheckpointKind::Canceled,
        TransferCheckpoint::FailureIntent(_) => StoredTransferCheckpointKind::FailureIntent,
        TransferCheckpoint::Failed { .. } => StoredTransferCheckpointKind::Failed,
        TransferCheckpoint::Skipped => StoredTransferCheckpointKind::Skipped,
    }
}

fn attach_merge_completions(
    checkpoint: &mut TransferCheckpoint,
    key: &StoredTransferWorkKey,
    completions: &[MergeChildCompletion],
) {
    let TransferCheckpoint::Merging(merge) = checkpoint else {
        return;
    };
    let core_key = TransferWorkKey {
        transfer_index: key.transfer_index,
        relative_path: key.relative_path.to_path_buf(),
    };
    merge.completed_children = completions
        .iter()
        .filter(|completion| completion.parent_key == core_key)
        .cloned()
        .collect();
    merge.completed_children.sort_by(|left, right| {
        left.child_key
            .relative_path
            .cmp(&right.child_key.relative_path)
    });
    merge.completed_prefix_verified = false;
    if let Some(active_child) = merge.active_child.as_mut() {
        let child_key = StoredTransferWorkKey {
            transfer_index: active_child.key.transfer_index,
            relative_path: StoredPath::from_path(&active_child.key.relative_path),
        };
        attach_merge_completions(&mut active_child.checkpoint, &child_key, completions);
    }
}

fn encode_identity(identity: &FileIdentity) -> StoredFileIdentity {
    StoredFileIdentity {
        device: identity.device,
        inode: identity.inode,
        object_kind: match identity.object_kind {
            FileObjectKind::RegularFile => StoredFileObjectKind::RegularFile,
            FileObjectKind::Directory => StoredFileObjectKind::Directory,
            FileObjectKind::SymbolicLink => StoredFileObjectKind::SymbolicLink,
        },
        size: identity.size,
        modified_seconds: identity.modified_seconds,
        modified_nanoseconds: identity.modified_nanoseconds,
        changed_seconds: identity.changed_seconds,
        changed_nanoseconds: identity.changed_nanoseconds,
        symbolic_link_target: identity
            .symbolic_link_target
            .as_deref()
            .map(StoredPath::from_path),
    }
}

fn decode_identity(identity: StoredFileIdentity) -> FileIdentity {
    FileIdentity {
        device: identity.device,
        inode: identity.inode,
        object_kind: match identity.object_kind {
            StoredFileObjectKind::RegularFile => FileObjectKind::RegularFile,
            StoredFileObjectKind::Directory => FileObjectKind::Directory,
            StoredFileObjectKind::SymbolicLink => FileObjectKind::SymbolicLink,
        },
        size: identity.size,
        modified_seconds: identity.modified_seconds,
        modified_nanoseconds: identity.modified_nanoseconds,
        changed_seconds: identity.changed_seconds,
        changed_nanoseconds: identity.changed_nanoseconds,
        symbolic_link_target: identity.symbolic_link_target.map(|path| path.to_path_buf()),
    }
}

fn decode_operation(operation: StoredTransferOperation) -> RecoverableTransferOperation {
    match operation {
        StoredTransferOperation::Copy => RecoverableTransferOperation::Copy,
        StoredTransferOperation::Move => RecoverableTransferOperation::Move,
    }
}

fn decode_conflict_strategy(strategy: StoredTransferConflictStrategy) -> TransferConflictStrategy {
    match strategy {
        StoredTransferConflictStrategy::Fail => TransferConflictStrategy::Fail,
        StoredTransferConflictStrategy::Replace => TransferConflictStrategy::Replace,
        StoredTransferConflictStrategy::Skip => TransferConflictStrategy::Skip,
        StoredTransferConflictStrategy::KeepBoth => TransferConflictStrategy::KeepBoth,
        StoredTransferConflictStrategy::Merge => TransferConflictStrategy::Merge,
    }
}

fn decode_verification(verification: StoredFileOperationVerification) -> FileOperationVerification {
    match verification {
        StoredFileOperationVerification::BasicMetadata => FileOperationVerification::BasicMetadata,
        StoredFileOperationVerification::Strong => FileOperationVerification::Strong,
    }
}

fn invalid<T>(message: &'static str) -> StoreResult<T> {
    Err(StoreError::InvalidRecoverableOperation(message))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use file_core::{
        BackupCreationTransfer, CommitPayload, CompletedTarget, MergeChildOutcome, MergeTransfer,
        ObjectFingerprint, PreparedTransfer, RenamedDirectMove, TransferCheckpoint,
        TransferExecutionKind,
    };

    use file_operation_store::StoredTransferJournalEntry;

    use super::*;

    fn identity(inode: u64) -> FileIdentity {
        FileIdentity {
            device: 1,
            inode,
            object_kind: FileObjectKind::RegularFile,
            size: 3,
            modified_seconds: 4,
            modified_nanoseconds: 5,
            changed_seconds: 6,
            changed_nanoseconds: 7,
            symbolic_link_target: None,
        }
    }

    #[test]
    fn manifest_encoding_stops_before_finishing_the_collection() {
        let manifest = SourceManifest {
            root: PathBuf::from("/tmp/source"),
            entries: (0..100)
                .map(|index| file_core::SourceManifestEntry {
                    relative_path: PathBuf::from(format!("file-{index}")),
                    identity: identity(index),
                })
                .collect(),
        };
        let mut checks = 0;

        let encoded = encode_manifest_entries_while(0, &manifest, || {
            checks += 1;
            checks < 8
        });

        assert!(encoded.is_none());
        assert_eq!(checks, 8);
    }

    #[test]
    fn direct_move_checkpoints_round_trip_with_distinct_kinds() {
        let prepared = PreparedTransfer {
            source_identity: identity(1),
            resolved_target: PathBuf::from("/tmp/target"),
            expected_target_identity: None,
            expected_target_fingerprint: None,
            source_fingerprint: None,
            execution: TransferExecutionKind::MoveDirect,
            staging_plan: None,
        };
        let intent = TransferCheckpoint::DirectMoveIntent(prepared.clone());
        let stored_intent = encode_checkpoint(&intent).unwrap();
        assert_eq!(
            stored_intent.kind,
            StoredTransferCheckpointKind::DirectMoveIntent
        );
        assert_eq!(decode_checkpoint(&stored_intent).unwrap(), intent);

        let renamed = TransferCheckpoint::DirectMoveRenamed(RenamedDirectMove {
            prepared,
            target_identity: identity(1),
        });
        let stored_renamed = encode_checkpoint(&renamed).unwrap();
        assert_eq!(
            stored_renamed.kind,
            StoredTransferCheckpointKind::DirectMoveRenamed
        );
        assert_eq!(decode_checkpoint(&stored_renamed).unwrap(), renamed);
    }

    #[test]
    fn backup_creation_checkpoint_round_trips_with_its_distinct_kind() {
        let checkpoint = TransferCheckpoint::BackupCreationIntent(BackupCreationTransfer {
            prepared: PreparedTransfer {
                source_identity: identity(1),
                resolved_target: PathBuf::from("/tmp/target"),
                expected_target_identity: Some(identity(2)),
                expected_target_fingerprint: None,
                source_fingerprint: None,
                execution: TransferExecutionKind::MoveToStage,
                staging_plan: None,
            },
            payload: CommitPayload::DirectSource {
                identity: identity(1),
            },
            fingerprint: ObjectFingerprint([7; 32]),
        });

        let stored = encode_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            stored.kind,
            StoredTransferCheckpointKind::BackupCreationIntent
        );
        assert_eq!(decode_checkpoint(&stored).unwrap(), checkpoint);
    }

    #[test]
    fn recovery_snapshot_attaches_merge_completion_identity_facts() {
        let source = PathBuf::from("/tmp/source");
        let target = PathBuf::from("/tmp/target");
        let parent_key = TransferWorkKey::top_level(0);
        let child_key = TransferWorkKey {
            transfer_index: 0,
            relative_path: PathBuf::from("child"),
        };
        let completion = MergeChildCompletion {
            parent_key: parent_key.clone(),
            child_key,
            outcome: MergeChildOutcome::Committed(CompletedTarget {
                path: target.join("child"),
                identity: identity(2),
                fingerprint: file_core::ops::TransferFingerprint::Blake3(ObjectFingerprint(
                    [9; 32],
                )),
            }),
        };
        let checkpoint = TransferCheckpoint::Merging(MergeTransfer {
            target_root_identity: identity(1),
            next_child: 1,
            active_child: None,
            child_names: Vec::new(),
            completed_children: Vec::new(),
            completed_prefix_verified: true,
        });
        let snapshot = StoredTransferRecoverySnapshot {
            journal_entries: vec![StoredTransferJournalEntry {
                key: encode_work_key(&parent_key),
                operation: StoredTransferOperation::Copy,
                source: StoredPath::from_path(&source),
                requested_target: StoredPath::from_path(&target),
                conflict_strategy: StoredTransferConflictStrategy::Merge,
                verification: StoredFileOperationVerification::Strong,
                checkpoint: encode_checkpoint(&checkpoint).unwrap(),
                revision: 2,
            }],
            manifest_entries: vec![StoredManifestEntry {
                transfer_index: 0,
                relative_path: StoredPath::from_path(Path::new("")),
                identity: encode_identity(&identity(3)),
            }],
            replacement_manifest_entries: Vec::new(),
            merge_completions: vec![encode_merge_completion(&completion).unwrap()],
        };

        let mut invalid_snapshot = snapshot.clone();
        let mut non_direct_completion = completion.clone();
        non_direct_completion.child_key.relative_path = PathBuf::from("nested/grandchild");
        invalid_snapshot.merge_completions[0] =
            encode_merge_completion(&non_direct_completion).unwrap();
        assert!(decode_recovery_snapshot(7, invalid_snapshot).is_err());

        let records = decode_recovery_snapshot(7, snapshot).unwrap();
        let TransferCheckpoint::Merging(reloaded) = &records[0].checkpoint else {
            panic!("merge checkpoint expected");
        };
        assert_eq!(reloaded.completed_children, vec![completion]);
        assert!(!reloaded.completed_prefix_verified);
    }
}
