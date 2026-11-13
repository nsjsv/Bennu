//! Move to another filesystem: once the target is committed, the source is
//! retired into an owned artifact and deleted entry by entry, every deletion
//! guarded by its own intent so a crash can neither lose nor double-delete.

use std::path::Path;

use tokio::fs;

use super::commit::{
    completed_target, matches_object_verification, verify_committed_target, verify_prepared_source,
};
use super::{manifest_root_identity, path_exists, record_manifest};
use crate::ops::recoverable_transfer::{
    inspect_file_identity, recover_owned_artifact, remove_empty_owned_artifact, rename_noreplace,
    CommitBarrier, FileIdentity, FileObjectKind, OwnedArtifact, OwnedTreeEntryDeletionIntent,
    PreparedTransfer, ProofContext, RecoverableTransferError, RetiredSource, SourceManifestEntry,
    SourceRetirementPlan, TransferCheckpoint, TransferExecutionKind, TransferFingerprint,
    TransferJournalRecord, TransferTransition,
};

pub(super) async fn retire_source(
    record: &TransferJournalRecord,
    retirement: SourceRetirementPlan,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    verify_committed_target(&retirement.committed, proof).await?;
    let Some(artifact) = retirement.artifact.clone() else {
        let artifact = recover_owned_artifact(retirement.artifact_plan.clone(), barrier).await?;
        let mut identified_retirement = retirement;
        identified_retirement.artifact = Some(artifact);
        return Ok(TransferTransition::Checkpoint(
            TransferCheckpoint::SourceRetirementIntent(Box::new(identified_retirement)),
        ));
    };
    crate::ops::recoverable_transfer::validate_owned_artifact(&artifact).await?;
    // 源退休的证明链要求全文哈希:KernelClone 证明不可能出现在 Move
    // (克隆候选已排除 Move),此处还原 Blake3 即可;若未来出现则拒绝。
    let committed_fingerprint = retirement
        .committed
        .fingerprint
        .as_blake3()
        .ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "source retirement requires a content fingerprint".to_owned(),
        })?;
    let prepared = PreparedTransfer {
        source_identity: manifest_root_identity(record_manifest(record)?)?.clone(),
        resolved_target: retirement.committed.final_target.clone(),
        expected_target_identity: None,
        expected_target_fingerprint: None,
        source_fingerprint: Some(committed_fingerprint),
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
            barrier.require_rename(&record.request.source, &payload_path);
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
        &retirement.committed.fingerprint,
        proof,
    )
    .await?
    {
        return Err(RecoverableTransferError::SourceChanged { path: payload_path });
    }
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::SourceRetired(Box::new(RetiredSource {
            committed: retirement.committed,
            artifact,
            payload_identity: Some(payload_identity),
            cleanup_index: 0,
            cleanup_intent: None,
        })),
    ))
}

pub(super) async fn advance_retired_source(
    record: &TransferJournalRecord,
    mut retired: RetiredSource,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
) -> Result<TransferTransition, RecoverableTransferError> {
    verify_committed_target(&retired.committed, proof).await?;
    if retired.payload_identity.is_none() {
        let payload_path = retired.artifact.plan.payload_path();
        let current = inspect_file_identity(&payload_path).await?;
        let expected = manifest_root_identity(record_manifest(record)?)?;
        if !matches_object_verification(
            &payload_path,
            &current,
            expected,
            &retired.committed.fingerprint,
            proof,
        )
        .await?
        {
            return Err(RecoverableTransferError::SourceChanged { path: payload_path });
        }
        retired.payload_identity = Some(current);
        return Ok(TransferTransition::Checkpoint(
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        ));
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
            proof,
            barrier,
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
        return Ok(TransferTransition::Checkpoint(
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        ));
    }

    if let Some(entry) = current_entry.as_ref() {
        let root_fingerprint = retired.committed.fingerprint.as_blake3().ok_or_else(|| {
            RecoverableTransferError::InvalidCheckpoint {
                message: "source retirement requires a content fingerprint".to_owned(),
            }
        })?;
        retired.cleanup_intent = Some(
            prepare_owned_tree_entry_deletion_intent(
                &retired.artifact,
                &retired.artifact.plan.payload_path(),
                entry,
                retired.payload_identity.as_ref(),
                root_fingerprint,
                proof,
            )
            .await?,
        );
        return Ok(TransferTransition::Checkpoint(
            TransferCheckpoint::SourceRetired(Box::new(retired)),
        ));
    }

    remove_empty_owned_artifact(&retired.artifact, barrier).await?;
    let completed = completed_target(&retired.committed);
    Ok(TransferTransition::Checkpoint(
        TransferCheckpoint::Completed(completed),
    ))
}

pub(super) fn reverse_manifest_entry(
    entries: &[SourceManifestEntry],
    completed_entries: usize,
) -> Option<SourceManifestEntry> {
    completed_entries
        .checked_add(1)
        .and_then(|offset| entries.len().checked_sub(offset))
        .and_then(|index| entries.get(index))
        .cloned()
}

pub(super) async fn prepare_owned_tree_entry_deletion_intent(
    artifact: &OwnedArtifact,
    tree_root: &Path,
    entry: &SourceManifestEntry,
    root_identity: Option<&FileIdentity>,
    root_fingerprint: crate::ops::recoverable_transfer::ObjectFingerprint,
    proof: &ProofContext,
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
            proof,
        )
        .await?;
        let current = inspect_file_identity(&entry_path).await?;
        let fingerprint = proof.fingerprint_object(&entry_path).await?;
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
pub(super) enum OwnedTreeEntryDeletionAdvance {
    Persist(Box<OwnedTreeEntryDeletionIntent>),
    Complete,
}

pub(super) async fn advance_owned_tree_entry_deletion(
    artifact: &OwnedArtifact,
    tree_root: &Path,
    deletion_slot: &Path,
    recovered_base: &Path,
    intent: &OwnedTreeEntryDeletionIntent,
    proof: &ProofContext,
    barrier: &mut CommitBarrier,
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
                verify_legacy_deletion_entry(&entry_path, intent, proof).await?;
                let mut upgraded = intent.clone();
                upgraded.expected_identity = Some(inspect_file_identity(&entry_path).await?);
                return Ok(OwnedTreeEntryDeletionAdvance::Persist(Box::new(upgraded)));
            }
            verify_deletion_entry_exact(
                &entry_path,
                intent.expected_identity.as_ref().unwrap(),
                intent.fingerprint,
                proof,
            )
            .await?;
            rename_noreplace(&entry_path, deletion_slot)
                .map_err(|error| error.into_transfer_error(&entry_path, deletion_slot))?;
            barrier.require_rename(&entry_path, deletion_slot);

            let slot_identity = inspect_file_identity(deletion_slot).await?;
            verify_deletion_entry_exact(deletion_slot, &slot_identity, intent.fingerprint, proof)
                .await?;
            let mut moved = intent.clone();
            moved.deletion_slot_identity = Some(slot_identity);
            Ok(OwnedTreeEntryDeletionAdvance::Persist(Box::new(moved)))
        }
        (false, true) => {
            if intent.deletion_slot_identity.is_none() {
                if let Err(error) = verify_legacy_deletion_entry(deletion_slot, intent, proof).await
                {
                    restore_unverified_deletion_slot(
                        &entry_path,
                        deletion_slot,
                        recovered_base,
                        intent,
                        barrier,
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
                proof,
            )
            .await
            {
                restore_unverified_deletion_slot(
                    &entry_path,
                    deletion_slot,
                    recovered_base,
                    intent,
                    barrier,
                )
                .await?;
                return Err(error);
            }
            remove_retirement_slot(deletion_slot).await?;
            barrier.require_parent_of(deletion_slot);
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
    barrier: &mut CommitBarrier,
) -> Result<(), RecoverableTransferError> {
    let slot_identity = inspect_file_identity(deletion_slot).await?;
    let expected = intent
        .deletion_slot_identity
        .as_ref()
        .or(intent.expected_identity.as_ref())
        .unwrap_or(&intent.entry.identity);
    if slot_identity.same_object(expected) {
        restore_unverified_owned_tree_entry(entry_path, deletion_slot, recovered_base, barrier)
            .await?;
    }
    Ok(())
}

async fn verify_legacy_deletion_entry(
    path: &Path,
    intent: &OwnedTreeEntryDeletionIntent,
    proof: &ProofContext,
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
    if current.same_object(expected) && proof.fingerprint_object(path).await? == fingerprint {
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
    proof: &ProofContext,
) -> Result<(), RecoverableTransferError> {
    let expected_fingerprint =
        expected_fingerprint.ok_or_else(|| RecoverableTransferError::InvalidCheckpoint {
            message: "existing deletion entry has no content fingerprint".to_owned(),
        })?;
    let before = inspect_file_identity(path).await?;
    let fingerprint = proof.fingerprint_object(path).await?;
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
    proof: &ProofContext,
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
            matches_object_verification(
                path,
                &current,
                expected,
                &TransferFingerprint::Blake3(root_fingerprint),
                proof,
            )
            .await?
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
    barrier: &mut CommitBarrier,
) -> Result<(), RecoverableTransferError> {
    let restored = if !path_exists(original_artifact_path).await? {
        original_artifact_path.to_path_buf()
    } else {
        super::next_recovered_path(recovered_base).await?
    };
    rename_noreplace(deletion_slot, &restored)
        .map_err(|error| error.into_transfer_error(deletion_slot, &restored))?;
    barrier.require_rename(deletion_slot, &restored);
    Ok(())
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
