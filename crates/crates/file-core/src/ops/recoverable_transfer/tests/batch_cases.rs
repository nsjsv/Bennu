use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use super::*;

struct RecordState {
    revision: u64,
    checkpoint: TransferCheckpoint,
    manifest: Option<SourceManifest>,
    replacement_manifest: Option<SourceManifest>,
}

impl RecordState {
    fn awaiting() -> Self {
        Self {
            revision: 0,
            checkpoint: TransferCheckpoint::AwaitingManifest,
            manifest: None,
            replacement_manifest: None,
        }
    }
}

struct MultiRecordMemoryJournal {
    task_id: u64,
    states: Mutex<BTreeMap<u64, RecordState>>,
    targets: Vec<PathBuf>,
    batch_commits: AtomicUsize,
    manifest_batch_commits: AtomicUsize,
    all_targets_visible_at_batch: AtomicBool,
}

impl MultiRecordMemoryJournal {
    fn new(task_id: u64, transfer_indices: &[u64], targets: Vec<PathBuf>) -> Self {
        Self {
            task_id,
            states: Mutex::new(
                transfer_indices
                    .iter()
                    .map(|index| (*index, RecordState::awaiting()))
                    .collect(),
            ),
            targets,
            batch_commits: AtomicUsize::new(0),
            manifest_batch_commits: AtomicUsize::new(0),
            all_targets_visible_at_batch: AtomicBool::new(false),
        }
    }

    fn record(
        &self,
        transfer_index: u64,
        request: RecoverableTransferRequest,
    ) -> TransferJournalRecord {
        let state = self.states.lock().unwrap();
        let state = state.get(&transfer_index).unwrap();
        TransferJournalRecord {
            task_id: self.task_id,
            key: TransferWorkKey::top_level(transfer_index),
            request,
            checkpoint: state.checkpoint.clone(),
            revision: state.revision,
            manifest: state.manifest.clone(),
            replacement_manifest: state.replacement_manifest.clone(),
        }
    }
}

impl TransferJournal for MultiRecordMemoryJournal {
    fn commit(&self, mutation: TransferJournalMutation) -> TransferJournalFuture<'_> {
        Box::pin(async move {
            let mut states = self.states.lock().unwrap();
            match mutation {
                TransferJournalMutation::InstallManifestAndCheckpoint {
                    task_id,
                    key,
                    expected_revision,
                    manifest,
                    replacement_manifest,
                    checkpoint,
                } => {
                    let state = states
                        .get_mut(&key.transfer_index)
                        .ok_or(TransferJournalError::StaleRevision)?;
                    if task_id != self.task_id || expected_revision != state.revision {
                        return Err(TransferJournalError::StaleRevision);
                    }
                    state.manifest = Some(manifest);
                    state.replacement_manifest = replacement_manifest;
                    state.checkpoint = checkpoint;
                    state.revision += 1;
                    Ok(state.revision)
                }
                TransferJournalMutation::CompareAndSwapCheckpoint {
                    task_id,
                    key,
                    expected_revision,
                    checkpoint,
                } => {
                    let state = states
                        .get_mut(&key.transfer_index)
                        .ok_or(TransferJournalError::StaleRevision)?;
                    if task_id != self.task_id || expected_revision != state.revision {
                        return Err(TransferJournalError::StaleRevision);
                    }
                    state.checkpoint = checkpoint;
                    state.revision += 1;
                    Ok(state.revision)
                }
                TransferJournalMutation::PersistMergeCompletionAndCheckpoint { .. } => {
                    Err(TransferJournalError::StaleRevision)
                }
                TransferJournalMutation::InstallManifestAndCheckpointBatch { .. } => {
                    Err(TransferJournalError::Storage(
                        "multi-record memory journal does not support batch manifest installs"
                            .to_owned(),
                    ))
                }
            }
        })
    }

    fn commit_checkpoint_batch<'a>(
        &'a self,
        swaps: Vec<TransferCheckpointSwap>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.batch_commits.fetch_add(1, Ordering::SeqCst);
            let all_visible = self.targets.iter().all(|target| target.exists());
            self.all_targets_visible_at_batch
                .store(all_visible, Ordering::SeqCst);
            let mut states = self.states.lock().unwrap();
            let mut revisions = Vec::with_capacity(swaps.len());
            for swap in swaps {
                let state = states
                    .get_mut(&swap.key.transfer_index)
                    .ok_or(TransferJournalError::StaleRevision)?;
                if swap.task_id != self.task_id || swap.expected_revision != state.revision {
                    return Err(TransferJournalError::StaleRevision);
                }
                state.checkpoint = swap.checkpoint;
                state.revision += 1;
                revisions.push(state.revision);
            }
            Ok(revisions)
        })
    }

    fn commit_manifest_batch<'a>(
        &'a self,
        updates: Vec<ManifestCheckpointBatchUpdate>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.manifest_batch_commits.fetch_add(1, Ordering::SeqCst);
            let mut states = self.states.lock().unwrap();
            for update in &updates {
                let state = states
                    .get(&update.key.transfer_index)
                    .ok_or(TransferJournalError::StaleRevision)?;
                if update.task_id != self.task_id || update.expected_revision != state.revision {
                    return Err(TransferJournalError::StaleRevision);
                }
            }
            let mut revisions = Vec::with_capacity(updates.len());
            for update in updates {
                let state = states.get_mut(&update.key.transfer_index).unwrap();
                state.manifest = Some(update.manifest);
                state.replacement_manifest = update.replacement_manifest;
                state.checkpoint = update.checkpoint;
                state.revision += 1;
                revisions.push(state.revision);
            }
            Ok(revisions)
        })
    }
}

#[tokio::test]
async fn batch_direct_move_prepares_every_intent_before_any_rename() {
    let directory = tempdir().unwrap();
    let mut sources = Vec::new();
    let mut targets = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        sources.push(source);
        targets.push(target);
    }

    let journal = MultiRecordMemoryJournal::new(300, &[0, 1, 2], targets.clone());
    let records = sources
        .iter()
        .zip(targets.iter())
        .enumerate()
        .map(|(index, (source, target))| {
            journal.record(
                index as u64,
                basic_transfer_request(
                    source.clone(),
                    target.clone(),
                    RecoverableTransferOperation::Move,
                    TransferConflictStrategy::Fail,
                ),
            )
        })
        .collect();
    let prepared =
        prepare_direct_move_intent_segment(records, &journal, &running_transfer_options())
            .await
            .unwrap();
    let records = prepared
        .into_iter()
        .map(|record| match record {
            DirectMoveIntentBatchRecord::Intent(record) => record,
            DirectMoveIntentBatchRecord::NotApplicable(_) => {
                panic!("basic move should prepare a direct move intent")
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(journal.manifest_batch_commits.load(Ordering::SeqCst), 1);

    for source in &sources {
        assert!(fs::symlink_metadata(source).await.is_ok());
    }
    for target in &targets {
        assert!(fs::symlink_metadata(target).await.is_err());
    }
    assert!(records
        .iter()
        .all(|record| matches!(record.checkpoint, TransferCheckpoint::DirectMoveIntent(_))));
}

#[tokio::test]
async fn batch_direct_move_renames_every_record_before_the_batch_commit() {
    let directory = tempdir().unwrap();
    let mut sources = Vec::new();
    let mut targets = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        sources.push(source);
        targets.push(target);
    }

    let journal = MultiRecordMemoryJournal::new(301, &[0, 1, 2], targets.clone());
    let mut records = Vec::new();
    for (index, (source, target)) in sources.iter().zip(targets.iter()).enumerate() {
        let request = basic_transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Fail,
        );
        let intent = run_recoverable_transfer_to_direct_move_intent(
            journal.record(index as u64, request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();
        let DirectMoveIntentBoundary::Intent(record) = intent else {
            panic!("basic move should prepare a direct move intent");
        };
        records.push(record);
    }

    let batch = run_direct_move_batch_to_durable_renamed(
        records,
        &journal,
        &running_transfer_options(),
        &mut |_| {},
    )
    .await
    .unwrap();
    assert_eq!(batch.len(), 3);
    assert!(batch.iter().all(|record| matches!(
        record,
        DirectMoveBatchRecord::Renamed(record)
            if matches!(record.checkpoint, TransferCheckpoint::DirectMoveRenamed(_))
    )));
    assert_eq!(journal.batch_commits.load(Ordering::SeqCst), 1);
    assert!(
        journal.all_targets_visible_at_batch.load(Ordering::SeqCst),
        "every target must be visible before the single batch commit"
    );
    for source in &sources {
        assert!(fs::symlink_metadata(source).await.is_err());
    }
    for target in &targets {
        assert!(fs::symlink_metadata(target).await.is_ok());
    }
}

#[tokio::test]
async fn batch_direct_move_recovers_after_a_crash_between_renames() {
    let directory = tempdir().unwrap();
    let mut sources = Vec::new();
    let mut targets = Vec::new();
    for index in 0..3 {
        let source = directory.path().join(format!("source-{index}"));
        let target = directory.path().join(format!("target-{index}"));
        fs::write(&source, format!("content-{index}"))
            .await
            .unwrap();
        sources.push(source);
        targets.push(target);
    }

    let journal = MultiRecordMemoryJournal::new(302, &[0, 1, 2], targets.clone());
    let mut records = Vec::new();
    for (index, (source, target)) in sources.iter().zip(targets.iter()).enumerate() {
        let request = basic_transfer_request(
            source.clone(),
            target.clone(),
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Fail,
        );
        let intent = run_recoverable_transfer_to_direct_move_intent(
            journal.record(index as u64, request),
            &journal,
            running_transfer_options(),
        )
        .await
        .unwrap();
        let DirectMoveIntentBoundary::Intent(record) = intent else {
            panic!("basic move should prepare a direct move intent");
        };
        records.push(record);
    }

    // Simulate a crash after the first rename but before any sync or batch
    // commit: record 0's source is already at its target.
    rename_noreplace(&sources[0], &targets[0]).unwrap();

    let batch = run_direct_move_batch_to_durable_renamed(
        records,
        &journal,
        &running_transfer_options(),
        &mut |_| {},
    )
    .await
    .unwrap();
    assert_eq!(batch.len(), 3);
    assert!(batch.iter().all(|record| matches!(
        record,
        DirectMoveBatchRecord::Renamed(record)
            if matches!(record.checkpoint, TransferCheckpoint::DirectMoveRenamed(_))
    )));
    for source in &sources {
        assert!(fs::symlink_metadata(source).await.is_err());
    }
    for target in &targets {
        assert!(fs::symlink_metadata(target).await.is_ok());
    }
}
