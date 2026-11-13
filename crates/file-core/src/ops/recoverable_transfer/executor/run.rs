//! The only execution loop. A segment of records advances in rounds: every
//! unfinished record runs one step (filesystem effects only), then the whole
//! round is flushed once and committed in one journal transaction. A single
//! record is a segment of one, so there is no second loop to keep in sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use tokio::sync::mpsc::UnboundedSender;

use super::super::{
    build_counted_source_manifest, CommitBarrier, RecoverableTransferError,
    RecoverableTransferOutcome, TransferCheckpoint, TransferFailureIntent, TransferJournal,
    TransferJournalMutation, TransferJournalRecord, TransferTransition, TransferWorkKey,
};
use super::{
    checkpoint_requires_forward_recovery, journal_error, step_recoverable_transfer,
    validate_next_revision, TransferAdvance, TransferStep,
};
use crate::ops::transfer_strategy::TransferStrategyEngine;
use crate::{FileError, FileOperationControls, FileTransferOptions};

/// A direct move whose rename is now durable in the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableDirectMoveCommit {
    pub work_key: TransferWorkKey,
    pub source: PathBuf,
    pub target: PathBuf,
    pub checkpoint_revision: u64,
}

#[derive(Debug)]
pub enum TransferSegmentEvent {
    /// These moves are visible at their targets but not durable yet; a file
    /// list can refresh without waiting for the flush and the journal.
    MovesRenamed(Vec<(PathBuf, PathBuf)>),
    DirectMovesCommitted(Vec<DurableDirectMoveCommit>),
    /// `index` is the record's position in the segment.
    RecordFinished {
        index: usize,
        result: Result<RecoverableTransferOutcome, RecoverableTransferError>,
    },
}

/// Runs every record to a terminal state. Each record's own failure is
/// reported through `RecordFinished` while its siblings keep going; the
/// returned error is one that stops the whole segment (journal failure, a
/// failed flush, shutdown, or a state that needs startup recovery). Records
/// are then left at their last durable checkpoint, exactly as after a crash.
pub async fn run_recoverable_segment<J: TransferJournal>(
    records: Vec<TransferJournalRecord>,
    journal: &J,
    transfer_options: &FileTransferOptions,
    events: &UnboundedSender<TransferSegmentEvent>,
) -> Result<(), RecoverableTransferError> {
    let mut engines = HashMap::new();
    let proof_memo = transfer_options
        .proof_memo
        .clone()
        .or_else(|| Some(crate::ops::recoverable_transfer::ProofMemo::shared()));
    let mut active = Vec::with_capacity(records.len());
    for (index, record) in records.into_iter().enumerate() {
        let mut options = transfer_options.clone();
        // Basic 校验在一次前向运行内每个对象至多全文哈希一次;崩溃重启后
        // memo 随运行重建,恢复路径仍全文重验。
        if record.request.verification == crate::ops::FileOperationVerification::BasicMetadata {
            options.proof_memo = proof_memo.clone();
        }
        if options.strategy_engine.is_none() {
            options.strategy_engine = strategy_engine_for(&record, &mut engines).await;
        }
        active.push(SegmentRecord {
            index,
            record,
            options,
            settling: None,
        });
    }
    // 同一步里的记录互不相关，并发做能把每次文件操作的往返等待叠起来；
    // 并发数按目标盘的类型取（机械盘只能低并发）。
    let parallelism = active
        .iter()
        .filter_map(|entry| entry.options.strategy_engine.as_ref())
        .map(|engine| engine.parallelism())
        .min()
        .unwrap_or(1);
    run_rounds(active, journal, events, parallelism).await
}

/// One record to the end; its own failure is the error.
pub async fn run_recoverable_transfer<J: TransferJournal>(
    record: TransferJournalRecord,
    journal: &J,
    transfer_options: FileTransferOptions,
) -> Result<RecoverableTransferOutcome, RecoverableTransferError> {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    run_recoverable_segment(vec![record], journal, &transfer_options, &events).await?;
    drop(events);
    while let Some(event) = received.recv().await {
        if let TransferSegmentEvent::RecordFinished { result, .. } = event {
            return result;
        }
    }
    unreachable!("a segment that returns Ok finishes every record")
}

/// One step of one record, flushed and committed; no failure handling. Tests
/// use it to stop a transfer at an exact checkpoint.
pub async fn advance_recoverable_transfer<J: TransferJournal>(
    record: &mut TransferJournalRecord,
    journal: &J,
    transfer_options: &FileTransferOptions,
) -> Result<TransferAdvance, RecoverableTransferError> {
    let mut barrier = CommitBarrier::default();
    let step = step_recoverable_transfer(record, transfer_options, &mut barrier).await;
    barrier.flush().await?;
    match step? {
        TransferStep::Complete(outcome) => Ok(TransferAdvance::Complete(outcome)),
        TransferStep::Transition(transition) => {
            commit_round(journal, vec![(record, *transition)]).await?;
            Ok(TransferAdvance::Continue)
        }
    }
}

/// Builds and installs the source manifest of every record that is still
/// awaiting one, in one journal transaction. `scanned` counts every entry seen.
/// A record whose manifest cannot be built keeps none; the error comes back in
/// its slot and the step that needs the manifest builds it again.
pub async fn persist_source_manifests<J: TransferJournal>(
    records: &mut [TransferJournalRecord],
    journal: &J,
    controls: &FileOperationControls,
    scanned: &std::sync::atomic::AtomicUsize,
) -> Result<Vec<Option<RecoverableTransferError>>, RecoverableTransferError> {
    let mut build_errors = Vec::with_capacity(records.len());
    let mut round = Vec::new();
    for record in records.iter_mut() {
        if record.manifest.is_some()
            || !matches!(record.checkpoint, TransferCheckpoint::AwaitingManifest)
        {
            build_errors.push(None);
            continue;
        }
        let mut manifest_controls = controls.clone();
        match build_counted_source_manifest(&record.request.source, &mut manifest_controls, scanned)
            .await
        {
            Ok(manifest) => {
                build_errors.push(None);
                round.push((
                    record,
                    TransferTransition::ManifestAndCheckpoint {
                        manifest,
                        replacement_manifest: None,
                        checkpoint: TransferCheckpoint::AwaitingManifest,
                    },
                ));
            }
            Err(
                error @ RecoverableTransferError::FileOperation(
                    FileError::Cancelled | FileError::ApplicationStopping,
                ),
            ) => return Err(error),
            Err(error) => build_errors.push(Some(error)),
        }
    }
    commit_round(journal, round).await?;
    Ok(build_errors)
}

/// Settles every unfinished record as failed in place before the task closes
/// with an ordinary failure, so the queue can drop the recovery details.
/// Terminal records stay as they are; a forward-only checkpoint cannot fail in
/// place and stops the settlement.
pub async fn settle_failed_recoverable_transfers<J: TransferJournal>(
    records: Vec<TransferJournalRecord>,
    journal: &J,
    diagnostic: &str,
) -> Result<(), RecoverableTransferError> {
    let mut failing = Vec::new();
    for record in records {
        if checkpoint_requires_forward_recovery(&record.checkpoint) {
            return Err(RecoverableTransferError::RecoveryBlocked {
                diagnostic: format!(
                    "record must recover forward instead of failing in place: {diagnostic}"
                ),
            });
        }
        if !is_terminal(&record.checkpoint) {
            failing.push(record);
        }
    }
    let round = failing
        .iter_mut()
        .map(|record| {
            let failure = failure_intent(&record.checkpoint, diagnostic.to_owned());
            (record, failure)
        })
        .collect();
    commit_round(journal, round).await?;
    let options = FileTransferOptions::new(FileOperationControls::running(
        tokio_util::sync::CancellationToken::new(),
    ));
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    run_recoverable_segment(failing, journal, &options, &events).await?;
    drop(events);
    while let Some(event) = received.recv().await {
        if let TransferSegmentEvent::RecordFinished {
            result: Err(error @ RecoverableTransferError::RecoveryBlocked { .. }),
            ..
        } = event
        {
            return Err(error);
        }
    }
    Ok(())
}

struct SegmentRecord {
    index: usize,
    record: TransferJournalRecord,
    options: FileTransferOptions,
    /// Set once the record turned to cancel or failure cleanup: the error it
    /// finishes with when the cleanup reaches its terminal checkpoint.
    settling: Option<RecoverableTransferError>,
}

enum RoundStep {
    Transition(Box<TransferTransition>),
    Finished(Result<RecoverableTransferOutcome, RecoverableTransferError>),
}

async fn run_rounds<J: TransferJournal>(
    mut active: Vec<SegmentRecord>,
    journal: &J,
    events: &UnboundedSender<TransferSegmentEvent>,
    parallelism: usize,
) -> Result<(), RecoverableTransferError> {
    while !active.is_empty() {
        let pending = active
            .iter_mut()
            .enumerate()
            .map(|(position, entry)| round_step(position, entry))
            .collect::<Vec<_>>();
        let mut steps = stream::iter(pending)
            .buffer_unordered(parallelism)
            .collect::<Vec<_>>()
            .await;
        steps.sort_by_key(|(position, _, _)| *position);
        let mut barrier = CommitBarrier::default();
        let mut transitions = Vec::new();
        let mut finished = Vec::new();
        for (position, step, step_barrier) in steps {
            barrier.absorb(step_barrier);
            match step? {
                RoundStep::Transition(transition) => transitions.push((position, *transition)),
                RoundStep::Finished(result) => finished.push((position, result)),
            }
        }

        let renamed = transitions
            .iter()
            .filter_map(|(position, transition)| match transition.checkpoint() {
                TransferCheckpoint::DirectMoveRenamed(renamed) => Some((
                    active[*position].record.request.source.clone(),
                    renamed.prepared.resolved_target.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !renamed.is_empty() {
            let _ = events.send(TransferSegmentEvent::MovesRenamed(renamed));
        }

        barrier.flush().await?;
        let committed = transitions
            .iter()
            .map(|(position, _)| *position)
            .collect::<Vec<_>>();
        let mut by_position = transitions.into_iter().collect::<HashMap<_, _>>();
        let round = active
            .iter_mut()
            .enumerate()
            .filter_map(|(position, entry)| {
                by_position
                    .remove(&position)
                    .map(|transition| (&mut entry.record, transition))
            })
            .collect();
        commit_round(journal, round).await?;

        let durable_moves = committed
            .iter()
            .filter_map(|position| durable_direct_move_commit(&active[*position].record, None))
            .collect::<Vec<_>>();
        if !durable_moves.is_empty() {
            let _ = events.send(TransferSegmentEvent::DirectMovesCommitted(durable_moves));
        }
        // 按原顺序报结果:完成列表是路径迁移和撤销历史的顺序。
        let finished_positions = finished
            .iter()
            .map(|(position, _)| *position)
            .collect::<Vec<_>>();
        for (position, result) in finished {
            let _ = events.send(TransferSegmentEvent::RecordFinished {
                index: active[position].index,
                result,
            });
        }
        for position in finished_positions.into_iter().rev() {
            active.remove(position);
        }
    }
    Ok(())
}

/// One record's step with its own barrier, so steps of a round can run
/// concurrently and merge their barriers afterwards.
async fn round_step(
    position: usize,
    entry: &mut SegmentRecord,
) -> (
    usize,
    Result<RoundStep, RecoverableTransferError>,
    CommitBarrier,
) {
    let mut barrier = CommitBarrier::default();
    let step = next_round_step(entry, &mut barrier).await;
    (position, step, barrier)
}

/// Same failure policy for every record: cancellation and ordinary failures
/// first clean up through an intent checkpoint, everything that cannot be
/// cleaned in place ends the record as blocked, and errors that leave no
/// trustworthy state stop the whole segment.
async fn next_round_step(
    entry: &mut SegmentRecord,
    barrier: &mut CommitBarrier,
) -> Result<RoundStep, RecoverableTransferError> {
    if entry.settling.is_some() && is_terminal(&entry.record.checkpoint) {
        let error = entry.settling.take().expect("settling error is present");
        return Ok(RoundStep::Finished(Err(error)));
    }
    let step = step_recoverable_transfer(&entry.record, &entry.options, barrier).await;
    let error = match step {
        Ok(TransferStep::Transition(transition)) => {
            return Ok(RoundStep::Transition(transition));
        }
        Ok(TransferStep::Complete(outcome)) => return Ok(RoundStep::Finished(Ok(outcome))),
        Err(error) => error,
    };
    if stops_segment(&error) {
        return Err(error);
    }
    if let Some(original) = entry.settling.take() {
        return Ok(RoundStep::Finished(Err(
            RecoverableTransferError::RecoveryBlocked {
                diagnostic: format!("{original}; recovery cleanup could not finish: {error}"),
            },
        )));
    }
    let blocked = |error: RecoverableTransferError| {
        Ok(RoundStep::Finished(Err(
            RecoverableTransferError::RecoveryBlocked {
                diagnostic: error.to_string(),
            },
        )))
    };
    match error {
        error @ RecoverableTransferError::FileOperation(FileError::Cancelled)
            if matches!(entry.record.checkpoint, TransferCheckpoint::Canceled { .. }) =>
        {
            Ok(RoundStep::Finished(Err(error)))
        }
        error @ (RecoverableTransferError::RecoveryBlocked { .. }
        | RecoverableTransferError::RecordedFailure { .. }) => Ok(RoundStep::Finished(Err(error))),
        error @ RecoverableTransferError::InvalidCheckpoint { .. } => blocked(error),
        error if checkpoint_requires_forward_recovery(&entry.record.checkpoint) => blocked(error),
        error @ RecoverableTransferError::FileOperation(FileError::Cancelled) => {
            let previous = Box::new(entry.record.checkpoint.clone());
            entry.settling = Some(error);
            Ok(RoundStep::Transition(Box::new(
                TransferTransition::Checkpoint(TransferCheckpoint::CancelIntent(previous)),
            )))
        }
        error => {
            let failure = failure_intent(&entry.record.checkpoint, error.to_string());
            entry.settling = Some(error);
            Ok(RoundStep::Transition(Box::new(failure)))
        }
    }
}

fn failure_intent(previous: &TransferCheckpoint, diagnostic: String) -> TransferTransition {
    TransferTransition::Checkpoint(TransferCheckpoint::FailureIntent(TransferFailureIntent {
        previous: Box::new(previous.clone()),
        diagnostic,
    }))
}

fn stops_segment(error: &RecoverableTransferError) -> bool {
    matches!(
        error,
        RecoverableTransferError::Journal { .. }
            | RecoverableTransferError::RecoveryRequired { .. }
            | RecoverableTransferError::FileOperation(FileError::ApplicationStopping)
    )
}

fn is_terminal(checkpoint: &TransferCheckpoint) -> bool {
    matches!(
        checkpoint,
        TransferCheckpoint::Completed(_)
            | TransferCheckpoint::Canceled { .. }
            | TransferCheckpoint::Failed { .. }
            | TransferCheckpoint::Skipped
    )
}

/// Commits one transition per record in one transaction and moves the
/// in-memory records to the committed state.
async fn commit_round<J: TransferJournal>(
    journal: &J,
    round: Vec<(&mut TransferJournalRecord, TransferTransition)>,
) -> Result<(), RecoverableTransferError> {
    if round.is_empty() {
        return Ok(());
    }
    let mutations = round
        .iter()
        .map(|(record, transition)| TransferJournalMutation {
            task_id: record.task_id,
            key: record.key.clone(),
            expected_revision: record.revision,
            transition: transition.clone(),
        })
        .collect::<Vec<_>>();
    let revisions = journal.commit(mutations).await.map_err(journal_error)?;
    if revisions.len() != round.len() {
        return Err(RecoverableTransferError::Journal {
            message: format!(
                "journal returned {} revisions for {} transitions",
                revisions.len(),
                round.len()
            ),
        });
    }
    for ((record, transition), revision) in round.into_iter().zip(revisions) {
        validate_next_revision(record.revision, revision)?;
        record.revision = revision;
        apply_transition(record, transition);
    }
    Ok(())
}

/// The in-memory side of a committed transition. Merge children live inside
/// their parent's checkpoint and are advanced with this too.
pub(super) fn apply_transition(record: &mut TransferJournalRecord, transition: TransferTransition) {
    match transition {
        TransferTransition::Checkpoint(checkpoint)
        | TransferTransition::MergeCompletionAndCheckpoint { checkpoint, .. } => {
            record.checkpoint = checkpoint;
        }
        TransferTransition::ManifestAndCheckpoint {
            manifest,
            replacement_manifest,
            checkpoint,
        } => {
            record.manifest = Some(manifest);
            record.replacement_manifest = replacement_manifest;
            record.checkpoint = checkpoint;
        }
    }
}

/// Merge children keep their own key, source and target; the parent revision
/// is the commit that made the nested checkpoint durable.
pub fn durable_direct_move_commit(
    record: &TransferJournalRecord,
    parent_revision: Option<u64>,
) -> Option<DurableDirectMoveCommit> {
    match &record.checkpoint {
        TransferCheckpoint::DirectMoveRenamed(renamed) => Some(DurableDirectMoveCommit {
            work_key: record.key.clone(),
            source: record.request.source.clone(),
            target: renamed.prepared.resolved_target.clone(),
            checkpoint_revision: parent_revision.unwrap_or(record.revision),
        }),
        TransferCheckpoint::Merging(merge) => merge
            .active_child
            .as_deref()
            .and_then(|child| durable_direct_move_commit(child, Some(record.revision))),
        _ => None,
    }
}

/// The copy strategy depends on the target filesystem; records of a segment
/// usually share one target directory, so probe each directory once.
async fn strategy_engine_for(
    record: &TransferJournalRecord,
    engines: &mut HashMap<PathBuf, Option<Arc<TransferStrategyEngine>>>,
) -> Option<Arc<TransferStrategyEngine>> {
    let target_parent = record
        .request
        .requested_target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| record.request.requested_target.clone());
    if let Some(engine) = engines.get(&target_parent) {
        return engine.clone();
    }
    // 拿不到父目录设备号就不探测,搬运退回历史路径,不阻塞传输。
    let engine = tokio::fs::symlink_metadata(&target_parent)
        .await
        .ok()
        .map(|metadata| {
            let target_device = std::os::unix::fs::MetadataExt::dev(&metadata);
            Arc::new(TransferStrategyEngine::probe(target_device, &target_parent))
        });
    engines.insert(target_parent, engine.clone());
    engine
}
