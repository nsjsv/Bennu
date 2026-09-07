use super::*;
use file_operation_store::{
    StoredManifestCheckpointBatchUpdate, StoredTransferCheckpointSwap,
    TransferManifestCheckpointUpdate,
};

mod progress;
mod store_codec;

use progress::TransferBatchProgress;
pub(super) use progress::{
    send_archive_creation_progress, send_archive_extraction_progress, send_file_operation_progress,
};

/// Convert only durable checkpoints into UI facts. Merge children retain their
/// child key/source/target, while the parent revision is the durable CAS that
/// made the nested checkpoint visible to recovery.
fn durable_direct_move_commit(
    record: &TransferJournalRecord,
    parent_revision: Option<u64>,
) -> Option<DurableDirectMoveCommit> {
    match &record.checkpoint {
        file_core::TransferCheckpoint::DirectMoveRenamed(renamed) => {
            Some(DurableDirectMoveCommit {
                work_key: record.key.clone(),
                source: record.request.source.clone(),
                target: renamed.prepared.resolved_target.clone(),
                checkpoint_revision: parent_revision.unwrap_or(record.revision),
            })
        }
        file_core::TransferCheckpoint::Merging(merge) => merge
            .active_child
            .as_deref()
            .and_then(|child| durable_direct_move_commit(child, Some(record.revision))),
        _ => None,
    }
}

#[derive(Clone)]
struct TaskQueueTransferJournal {
    store: TaskQueueStore,
    controls: FileOperationControls,
    durable_commits: std::sync::Arc<std::sync::Mutex<Vec<DurableDirectMoveCommit>>>,
}

impl TransferJournal for TaskQueueTransferJournal {
    fn commit(
        &self,
        mutation: TransferJournalMutation,
    ) -> Pin<Box<dyn Future<Output = Result<u64, TransferJournalError>> + Send + '_>> {
        let store = self.store.clone();
        let controls = self.controls.clone();
        let durable_commits = self.durable_commits.clone();
        let mut durable_commit = match &mutation {
            TransferJournalMutation::CompareAndSwapCheckpoint { checkpoint, .. }
            | TransferJournalMutation::PersistMergeCompletionAndCheckpoint { checkpoint, .. } => {
                match checkpoint {
                    file_core::TransferCheckpoint::Merging(merge) => merge
                        .active_child
                        .as_deref()
                        .and_then(|child| durable_direct_move_commit(child, Some(0))),
                    _ => None,
                }
            }
            _ => None,
        };
        Box::pin(async move {
            let revision = tokio::task::spawn_blocking(move || match mutation {
                TransferJournalMutation::InstallManifestAndCheckpoint {
                    task_id,
                    key,
                    expected_revision,
                    manifest,
                    replacement_manifest,
                    checkpoint,
                } => {
                    let stored_key = store_codec::encode_work_key(&key);
                    let encoding_controls = controls.clone();
                    let manifest_entries = store_codec::encode_manifest_entries_while(
                        key.transfer_index,
                        &manifest,
                        || encoding_controls.checkpoint_now().is_ok(),
                    )
                    .ok_or_else(|| interrupted_journal_error(&controls))?;
                    let replacement_manifest_entries = match replacement_manifest.as_ref() {
                        Some(manifest) => store_codec::encode_manifest_entries_while(
                            key.transfer_index,
                            manifest,
                            || encoding_controls.checkpoint_now().is_ok(),
                        )
                        .ok_or_else(|| interrupted_journal_error(&controls))?,
                        None => Vec::new(),
                    };
                    let stored_checkpoint =
                        store_codec::encode_checkpoint(&checkpoint).map_err(store_journal_error)?;
                    let transaction_controls = controls.clone();
                    let revision = store
                        .install_transfer_manifests_and_checkpoint_while(
                            task_id,
                            TransferManifestCheckpointUpdate {
                                key: &stored_key,
                                expected_revision,
                                manifest_entries: &manifest_entries,
                                replacement_manifest_entries: &replacement_manifest_entries,
                                checkpoint: &stored_checkpoint,
                            },
                            || transaction_controls.checkpoint_now().is_ok(),
                        )
                        .map_err(store_journal_error)?;
                    revision.ok_or_else(|| interrupted_journal_error(&controls))
                }
                TransferJournalMutation::CompareAndSwapCheckpoint {
                    task_id,
                    key,
                    expected_revision,
                    checkpoint,
                } => store
                    .compare_and_swap_transfer_checkpoint(
                        task_id,
                        &store_codec::encode_work_key(&key),
                        expected_revision,
                        &store_codec::encode_checkpoint(&checkpoint)
                            .map_err(store_journal_error)?,
                    )
                    .map_err(store_journal_error),
                TransferJournalMutation::PersistMergeCompletionAndCheckpoint {
                    task_id,
                    key,
                    expected_revision,
                    completion,
                    checkpoint,
                } => store
                    .compare_and_swap_transfer_merge_completion(
                        task_id,
                        &store_codec::encode_work_key(&key),
                        expected_revision,
                        &store_codec::encode_merge_completion(&completion)
                            .map_err(store_journal_error)?,
                        &store_codec::encode_checkpoint(&checkpoint)
                            .map_err(store_journal_error)?,
                    )
                    .map_err(store_journal_error),
                TransferJournalMutation::InstallManifestAndCheckpointBatch { .. } => {
                    Err(TransferJournalError::Storage(
                        "batch manifest installs use commit_manifest_batch".to_owned(),
                    ))
                }
            })
            .await
            .map_err(|error| TransferJournalError::Storage(error.to_string()))??;
            if let Some(commit) = durable_commit.as_mut() {
                commit.checkpoint_revision = revision;
                durable_commits.lock().unwrap().push(commit.clone());
            }
            Ok(revision)
        })
    }

    fn commit_checkpoint_batch<'a>(
        &'a self,
        swaps: Vec<file_core::TransferCheckpointSwap>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut stored_swaps = Vec::with_capacity(swaps.len());
                for swap in &swaps {
                    stored_swaps.push(StoredTransferCheckpointSwap {
                        task_id: swap.task_id,
                        key: store_codec::encode_work_key(&swap.key),
                        expected_revision: swap.expected_revision,
                        checkpoint: store_codec::encode_checkpoint(&swap.checkpoint)
                            .map_err(store_journal_error)?,
                    });
                }
                store
                    .compare_and_swap_transfer_checkpoints(&stored_swaps)
                    .map_err(store_journal_error)
            })
            .await
            .map_err(|error| TransferJournalError::Storage(error.to_string()))?
        })
    }

    fn commit_manifest_batch<'a>(
        &'a self,
        updates: Vec<file_core::ManifestCheckpointBatchUpdate>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u64>, TransferJournalError>> + Send + 'a>> {
        let store = self.store.clone();
        let controls = self.controls.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut stored = Vec::with_capacity(updates.len());
                for update in &updates {
                    let encoding_controls = controls.clone();
                    let manifest_entries = store_codec::encode_manifest_entries_while(
                        update.key.transfer_index,
                        &update.manifest,
                        || encoding_controls.checkpoint_now().is_ok(),
                    )
                    .ok_or_else(|| interrupted_journal_error(&controls))?;
                    let replacement_manifest_entries = match update.replacement_manifest.as_ref() {
                        Some(manifest) => store_codec::encode_manifest_entries_while(
                            update.key.transfer_index,
                            manifest,
                            || encoding_controls.checkpoint_now().is_ok(),
                        )
                        .ok_or_else(|| interrupted_journal_error(&controls))?,
                        None => Vec::new(),
                    };
                    stored.push(StoredManifestCheckpointBatchUpdate {
                        task_id: update.task_id,
                        key: store_codec::encode_work_key(&update.key),
                        expected_revision: update.expected_revision,
                        manifest_entries,
                        replacement_manifest_entries,
                        checkpoint: store_codec::encode_checkpoint(&update.checkpoint)
                            .map_err(store_journal_error)?,
                    });
                }
                store
                    .install_transfer_manifests_and_checkpoints_batch(&stored)
                    .map_err(store_journal_error)
            })
            .await
            .map_err(|error| TransferJournalError::Storage(error.to_string()))?
        })
    }
}

fn task_queue_transfer_journal(
    store: TaskQueueStore,
    controls: FileOperationControls,
) -> TaskQueueTransferJournal {
    TaskQueueTransferJournal {
        store,
        controls,
        durable_commits: Default::default(),
    }
}

impl TaskQueueTransferJournal {
    fn take_durable_commits(&self) -> Vec<DurableDirectMoveCommit> {
        std::mem::take(&mut *self.durable_commits.lock().unwrap())
    }
}

fn store_journal_error(error: file_operation_store::StoreError) -> TransferJournalError {
    match error {
        file_operation_store::StoreError::StaleTransferRevision { .. } => {
            TransferJournalError::StaleRevision
        }
        error => TransferJournalError::Storage(error.to_string()),
    }
}

fn interrupted_journal_error(controls: &FileOperationControls) -> TransferJournalError {
    match controls.checkpoint_now() {
        Err(FileError::ApplicationStopping) => TransferJournalError::ApplicationStopping,
        Err(FileError::Cancelled) => TransferJournalError::UserCancelled,
        _ => TransferJournalError::Storage(
            "manifest transaction stopped without an operation control signal".to_owned(),
        ),
    }
}

pub(super) async fn run_queued_transfers(
    transfers: Vec<QueuedTransfer>,
    controls: FileOperationControls,
    stored_task_id: u64,
    task_id: u64,
    output: &mut IcedSender<Message>,
    store: Option<TaskQueueStore>,
    mode: QueuedTransferMode,
    verification: FileOperationVerification,
) -> FileOperationCompletion {
    let Some(store) = store else {
        return FileOperationCompletion::from_result(Err(
            "File operation queue storage is unavailable; transfer was not started".to_owned(),
        ));
    };
    let records = match load_recoverable_transfer_records(store.clone(), stored_task_id).await {
        Ok(records) => records,
        Err(RecoverableRecordLoadError::Interrupted(error)) => {
            return FileOperationCompletion::RecoveryInterrupted(error, Vec::new());
        }
        Err(RecoverableRecordLoadError::Invalid(error)) => {
            return FileOperationCompletion::RecoveryBlocked {
                error,
                completed_move_transfers: Vec::new(),
            };
        }
    };
    if records.len() != transfers.len()
        || records
            .iter()
            .zip(&transfers)
            .enumerate()
            .any(|(transfer_index, (record, transfer))| {
                record.key != file_core::TransferWorkKey::top_level(transfer_index as u64)
                    || record.request.source != transfer.source
                    || record.request.requested_target != transfer.target
                    || record.request.conflict_strategy != transfer.conflict_strategy
                    || record.request.verification != verification
                    || record.request.operation != mode.operation()
            })
    {
        return FileOperationCompletion::RecoveryBlocked {
            error: "Stored transfer journal does not match the queued operation".to_owned(),
            completed_move_transfers: Vec::new(),
        };
    }

    let history_eligibility = if transfers.iter().all(history_safe_transfer) {
        FileOperationHistoryEligibility::Replayable
    } else {
        FileOperationHistoryEligibility::NotReplayable
    };

    let base_options = FileTransferOptions::new(controls.clone());
    let journal = task_queue_transfer_journal(store, controls.clone());

    let prepared =
        {
            let prepared =
                match prepare_queued_transfer_records(records, &journal, &base_options, &controls)
                    .await
                {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        return settle_recoverable_transfer_failure(
                            mode,
                            error,
                            Vec::new(),
                            stored_task_id,
                            task_id,
                            output,
                            &journal,
                            &controls,
                        )
                        .await;
                    }
                };
            prepared
        };
    let progress_records = prepared
        .iter()
        .map(|record| match record {
            DirectMoveIntentBatchRecord::Intent(record)
            | DirectMoveIntentBatchRecord::NotApplicable(record) => record.clone(),
        })
        .collect::<Vec<_>>();
    let mut batch_progress = TransferBatchProgress::new(&progress_records);

    let mut completed = Vec::new();
    let mut intent_records: Vec<(usize, TransferJournalRecord)> = Vec::new();
    for (index, record) in prepared.into_iter().enumerate() {
        match record {
            DirectMoveIntentBatchRecord::Intent(record) => {
                intent_records.push((index, record));
            }
            DirectMoveIntentBatchRecord::NotApplicable(record) => {
                if let Err(error) = settle_intent_segment(
                    &mut intent_records,
                    &controls,
                    &journal,
                    &base_options,
                    task_id,
                    output,
                    &mut batch_progress,
                    &mut completed,
                )
                .await
                {
                    return settle_recoverable_transfer_failure(
                        mode,
                        error,
                        completed,
                        stored_task_id,
                        task_id,
                        output,
                        &journal,
                        &controls,
                    )
                    .await;
                }
                if let Err(error) = run_not_applicable_record(
                    record,
                    index,
                    &controls,
                    &journal,
                    &base_options,
                    task_id,
                    output,
                    &mut batch_progress,
                    &mut completed,
                )
                .await
                {
                    return settle_recoverable_transfer_failure(
                        mode,
                        error,
                        completed,
                        stored_task_id,
                        task_id,
                        output,
                        &journal,
                        &controls,
                    )
                    .await;
                }
            }
        }
    }
    if let Err(error) = settle_intent_segment(
        &mut intent_records,
        &controls,
        &journal,
        &base_options,
        task_id,
        output,
        &mut batch_progress,
        &mut completed,
    )
    .await
    {
        return settle_recoverable_transfer_failure(
            mode,
            error,
            completed,
            stored_task_id,
            task_id,
            output,
            &journal,
            &controls,
        )
        .await;
    }

    match mode {
        QueuedTransferMode::Copy
            if history_eligibility == FileOperationHistoryEligibility::Replayable =>
        {
            FileOperationCompletion::Succeeded(FileOperationOutcome::Copy {
                transfers: completed,
            })
        }
        QueuedTransferMode::Copy => {
            FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory)
        }
        QueuedTransferMode::Move => {
            FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
                transfers: completed,
                history_eligibility,
            })
        }
    }
}

async fn prepare_queued_transfer_records(
    records: Vec<TransferJournalRecord>,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    controls: &FileOperationControls,
) -> Result<Vec<DirectMoveIntentBatchRecord>, RecoverableTransferError> {
    let mut prepared = Vec::with_capacity(records.len());
    let mut direct_segment = Vec::new();

    for record in records {
        if is_direct_move_segment_candidate(&record) {
            direct_segment.push(record);
            continue;
        }
        flush_direct_move_preparation(
            &mut direct_segment,
            &mut prepared,
            journal,
            options,
            controls,
        )
        .await?;
        prepared.push(DirectMoveIntentBatchRecord::NotApplicable(
            persist_manifest_for_progress(record, journal, controls).await?,
        ));
    }
    flush_direct_move_preparation(
        &mut direct_segment,
        &mut prepared,
        journal,
        options,
        controls,
    )
    .await?;
    Ok(prepared)
}

async fn flush_direct_move_preparation(
    direct_segment: &mut Vec<TransferJournalRecord>,
    prepared: &mut Vec<DirectMoveIntentBatchRecord>,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    controls: &FileOperationControls,
) -> Result<(), RecoverableTransferError> {
    if direct_segment.is_empty() {
        return Ok(());
    }
    let segment =
        prepare_direct_move_intent_segment(std::mem::take(direct_segment), journal, options)
            .await?;
    for record in segment {
        match record {
            DirectMoveIntentBatchRecord::NotApplicable(record) => {
                prepared.push(DirectMoveIntentBatchRecord::NotApplicable(
                    persist_manifest_for_progress(record, journal, controls).await?,
                ));
            }
            record => prepared.push(record),
        }
    }
    Ok(())
}

async fn persist_manifest_for_progress(
    mut record: TransferJournalRecord,
    journal: &TaskQueueTransferJournal,
    controls: &FileOperationControls,
) -> Result<TransferJournalRecord, RecoverableTransferError> {
    if record.manifest.is_some()
        || !matches!(
            record.checkpoint,
            file_core::TransferCheckpoint::AwaitingManifest
        )
    {
        return Ok(record);
    }
    let mut manifest_controls = controls.clone();
    if manifest_controls.wait_until_running().await.is_err() {
        return Ok(record);
    }
    match persist_recoverable_source_manifest_with_controls(
        &mut record,
        journal,
        &mut manifest_controls,
    )
    .await
    {
        Ok(()) => Ok(record),
        Err(error @ RecoverableTransferError::Journal { .. })
        | Err(error @ RecoverableTransferError::RecoveryRequired { .. }) => Err(error),
        Err(_) => Ok(record),
    }
}

async fn settle_intent_segment(
    intent_records: &mut Vec<(usize, TransferJournalRecord)>,
    controls: &FileOperationControls,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    task_id: u64,
    output: &mut IcedSender<Message>,
    batch_progress: &mut TransferBatchProgress,
    completed: &mut Vec<CompletedTransfer>,
) -> Result<(), RecoverableTransferError> {
    let records_with_index = std::mem::take(intent_records);
    if records_with_index.is_empty() {
        return Ok(());
    }
    let indices: Vec<usize> = records_with_index.iter().map(|(index, _)| *index).collect();
    let records: Vec<TransferJournalRecord> = records_with_index
        .into_iter()
        .map(|(_, record)| record)
        .collect();
    let batch = {
        // rename 全部完成后立即上报 UI（同一条 output channel，先于 durable commits），
        // 让列表刷新不等 parent fsync 与 journal 落盘。
        let renamed_output = output.clone();
        let mut on_renamed = |moves: Vec<(PathBuf, PathBuf)>| {
            let _ = renamed_output
                .clone()
                .try_send(Message::FileOperationMovesRenamed {
                    task_id,
                    moves: moves
                        .into_iter()
                        .map(|(source, target)| CompletedTransfer { source, target })
                        .collect(),
                });
        };
        let batch =
            run_direct_move_batch_to_durable_renamed(records, journal, options, &mut on_renamed)
                .await?;
        batch
    };

    let mut durable_direct_moves = Vec::new();
    let mut diverged = Vec::new();
    let mut first_item_error = None;
    for (record, index) in batch.into_iter().zip(indices) {
        match record {
            DirectMoveBatchRecord::Renamed(record) => durable_direct_moves.push((index, record)),
            DirectMoveBatchRecord::Diverged(record) => diverged.push((index, record)),
            DirectMoveBatchRecord::Failed { error, .. } => {
                first_item_error.get_or_insert(error);
                send_file_operation_progress(
                    output,
                    task_id,
                    batch_progress.complete_record(index),
                )
                .await;
            }
        }
    }

    send_durable_direct_move_commits(output, task_id, &durable_direct_moves).await;
    complete_durable_direct_move_segment(
        &mut durable_direct_moves,
        controls,
        journal,
        options,
        task_id,
        output,
        batch_progress,
        completed,
    )
    .await?;

    for (index, record) in diverged {
        let outcome = run_recoverable_record_with_progress(
            record,
            controls.clone(),
            journal,
            options,
            task_id,
            output,
            index,
            batch_progress,
        )
        .await?;
        accept_recoverable_transfer_outcome(outcome, completed);
        send_file_operation_progress(output, task_id, batch_progress.complete_record(index)).await;
    }

    if let Some(error) = first_item_error {
        return Err(error);
    }
    Ok(())
}

async fn run_not_applicable_record(
    record: TransferJournalRecord,
    index: usize,
    controls: &FileOperationControls,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    task_id: u64,
    output: &mut IcedSender<Message>,
    batch_progress: &mut TransferBatchProgress,
    completed: &mut Vec<CompletedTransfer>,
) -> Result<(), RecoverableTransferError> {
    if let Some(commit) = durable_direct_move_commit(&record, None) {
        send_durable_direct_move_commits(output, task_id, &[(index, record.clone())]).await;
        let _ = commit;
    }
    let outcome = run_recoverable_record_with_progress(
        record,
        controls.clone(),
        journal,
        options,
        task_id,
        output,
        index,
        batch_progress,
    )
    .await?;
    send_durable_direct_move_commit_values(output, task_id, journal.take_durable_commits()).await;
    accept_recoverable_transfer_outcome(outcome, completed);
    send_file_operation_progress(output, task_id, batch_progress.complete_record(index)).await;
    Ok(())
}

async fn settle_recoverable_transfer_failure(
    mode: QueuedTransferMode,
    error: RecoverableTransferError,
    completed_move_transfers: Vec<CompletedTransfer>,
    stored_task_id: u64,
    task_id: u64,
    output: &mut IcedSender<Message>,
    journal: &TaskQueueTransferJournal,
    controls: &FileOperationControls,
) -> FileOperationCompletion {
    if !matches!(
        &error,
        RecoverableTransferError::FileOperation(FileError::Cancelled)
    ) && !matches!(controls.checkpoint_now(), Err(FileError::Cancelled))
    {
        return recoverable_transfer_failure(mode, error, completed_move_transfers);
    }

    let records =
        match load_recoverable_transfer_records(journal.store.clone(), stored_task_id).await {
            Ok(records) => records,
            Err(RecoverableRecordLoadError::Interrupted(load_error)) => {
                return FileOperationCompletion::RecoveryInterrupted(
                    load_error,
                    completed_move_transfers,
                );
            }
            Err(RecoverableRecordLoadError::Invalid(load_error)) => {
                return FileOperationCompletion::RecoveryBlocked {
                    error: load_error,
                    completed_move_transfers,
                };
            }
        };

    let mut batch_progress = TransferBatchProgress::new(&records);
    let settlement_options = FileTransferOptions::new(controls.clone());
    let mut settled_completed = Vec::new();
    let mut settlement_error = None;
    for (record_index, record) in records.into_iter().enumerate() {
        if durable_direct_move_commit(&record, None).is_some() {
            send_durable_direct_move_commits(output, task_id, &[(record_index, record.clone())])
                .await;
        }
        let settled = run_recoverable_record_with_progress(
            record,
            controls.clone(),
            journal,
            &settlement_options,
            task_id,
            output,
            record_index,
            &mut batch_progress,
        )
        .await;
        send_durable_direct_move_commit_values(output, task_id, journal.take_durable_commits())
            .await;
        match settled {
            Ok(outcome) => {
                accept_recoverable_transfer_outcome(outcome, &mut settled_completed);
                send_file_operation_progress(
                    output,
                    task_id,
                    batch_progress.complete_record(record_index),
                )
                .await;
            }
            Err(RecoverableTransferError::FileOperation(FileError::Cancelled)) => {}
            Err(record_error) => {
                settlement_error.get_or_insert(record_error);
            }
        }
    }

    let error = match settlement_error {
        Some(settlement_error)
            if matches!(
                &error,
                RecoverableTransferError::FileOperation(FileError::Cancelled)
            ) || matches!(
                &settlement_error,
                RecoverableTransferError::FileOperation(FileError::ApplicationStopping)
                    | RecoverableTransferError::Journal { .. }
                    | RecoverableTransferError::RecoveryRequired { .. }
                    | RecoverableTransferError::RecoveryBlocked { .. }
            ) =>
        {
            settlement_error
        }
        _ => error,
    };
    recoverable_transfer_failure(mode, error, settled_completed)
}

fn recoverable_transfer_failure(
    mode: QueuedTransferMode,
    error: RecoverableTransferError,
    completed_move_transfers: Vec<CompletedTransfer>,
) -> FileOperationCompletion {
    let diagnostic = error.to_string();
    match error {
        RecoverableTransferError::FileOperation(file_core::FileError::ApplicationStopping) => {
            FileOperationCompletion::RecoveryInterrupted(diagnostic, completed_move_transfers)
        }
        RecoverableTransferError::FileOperation(file_core::FileError::Cancelled) => {
            FileOperationCompletion::Canceled(match mode {
                QueuedTransferMode::Copy => Vec::new(),
                QueuedTransferMode::Move => completed_move_transfers,
            })
        }
        RecoverableTransferError::Journal { .. }
        | RecoverableTransferError::RecoveryRequired { .. } => {
            FileOperationCompletion::RecoveryInterrupted(diagnostic, completed_move_transfers)
        }
        RecoverableTransferError::RecoveryBlocked { .. } => {
            FileOperationCompletion::RecoveryBlocked {
                error: diagnostic,
                completed_move_transfers,
            }
        }
        _ => match mode {
            QueuedTransferMode::Copy => FileOperationCompletion::from_result(Err(diagnostic)),
            QueuedTransferMode::Move => FileOperationCompletion::failed_after_completed_moves(
                diagnostic,
                completed_move_transfers,
            ),
        },
    }
}

#[derive(Debug)]

enum RecoverableRecordLoadError {
    Interrupted(String),
    Invalid(String),
}

async fn load_recoverable_transfer_records(
    store: TaskQueueStore,
    task_id: u64,
) -> Result<Vec<TransferJournalRecord>, RecoverableRecordLoadError> {
    let records = tokio::task::spawn_blocking(move || {
        let snapshot = store.read_transfer_recovery(task_id)?;
        store_codec::decode_recovery_snapshot(task_id, snapshot)
    })
    .await
    .map_err(|error| {
        RecoverableRecordLoadError::Interrupted(format!("Transfer recovery worker failed: {error}"))
    })?;
    records.map_err(|error| {
        let message = format!("Transfer recovery journal could not be read: {error}");
        if error.is_invalid_recovery_data() {
            RecoverableRecordLoadError::Invalid(message)
        } else {
            RecoverableRecordLoadError::Interrupted(message)
        }
    })
}

async fn complete_durable_direct_move_segment(
    records: &mut Vec<(usize, TransferJournalRecord)>,
    controls: &FileOperationControls,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    task_id: u64,
    output: &mut IcedSender<Message>,
    batch_progress: &mut TransferBatchProgress,
    completed: &mut Vec<CompletedTransfer>,
) -> Result<(), RecoverableTransferError> {
    let mut first_settlement_error = None;
    for (index, record) in std::mem::take(records) {
        match run_recoverable_record_with_progress(
            record,
            controls.clone(),
            journal,
            options,
            task_id,
            output,
            index,
            batch_progress,
        )
        .await
        {
            Ok(outcome) => {
                accept_recoverable_transfer_outcome(outcome, completed);
                if first_settlement_error.is_none() {
                    send_file_operation_progress(
                        output,
                        task_id,
                        batch_progress.complete_record(index),
                    )
                    .await;
                }
            }
            Err(error) if recovery_interruption_prevents_settlement(&error) => {
                return Err(error);
            }
            Err(error) => {
                first_settlement_error.get_or_insert(error);
            }
        }
    }
    match first_settlement_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn accept_recoverable_transfer_outcome(
    outcome: RecoverableTransferOutcome,
    completed: &mut Vec<CompletedTransfer>,
) {
    if let RecoverableTransferOutcome {
        source,
        final_target: Some(target),
    } = outcome
    {
        completed.push(CompletedTransfer { source, target });
    }
}

fn recovery_interruption_prevents_settlement(error: &RecoverableTransferError) -> bool {
    matches!(
        error,
        RecoverableTransferError::FileOperation(file_core::FileError::ApplicationStopping)
            | RecoverableTransferError::Journal { .. }
            | RecoverableTransferError::RecoveryRequired { .. }
    )
}

async fn run_recoverable_record_with_progress(
    record: TransferJournalRecord,
    _controls: FileOperationControls,
    journal: &TaskQueueTransferJournal,
    options: &FileTransferOptions,
    task_id: u64,
    output: &mut IcedSender<Message>,
    record_index: usize,
    batch_progress: &mut TransferBatchProgress,
) -> Result<RecoverableTransferOutcome, RecoverableTransferError> {
    let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::unbounded_channel();
    let transfer_options = options.clone().with_progress_sender(progress_sender);
    let transfer = run_recoverable_transfer(record, journal, transfer_options);
    tokio::pin!(transfer);
    let mut latest_copy_progress = None;
    let mut last_copy_progress_sent_at = None;

    loop {
        tokio::select! {
            progress = progress_receiver.recv() => {
                if let Some(progress) = progress {
                    latest_copy_progress = Some(progress);
                    let now = Instant::now();
                    if should_send_byte_progress(last_copy_progress_sent_at, now) {
                        if let Some(progress) = latest_copy_progress.take() {
                            send_copy_progress(output, task_id, record_index, progress, batch_progress).await;
                            last_copy_progress_sent_at = Some(now);
                        }
                    }
                }
            }
            transfer_outcome = &mut transfer => {
                latest_copy_progress = progress::drain_latest_copy_progress(
                    &mut progress_receiver,
                    latest_copy_progress,
                );
                if let Some(progress) = latest_copy_progress.take() {
                    send_copy_progress(output, task_id, record_index, progress, batch_progress).await;
                }
                return transfer_outcome;
            }
        }
    }
}

async fn send_durable_direct_move_commits(
    output: &mut IcedSender<Message>,
    task_id: u64,
    records: &[(usize, TransferJournalRecord)],
) {
    let commits = records
        .iter()
        .filter_map(|(_, record)| durable_direct_move_commit(record, None))
        .collect::<Vec<_>>();
    send_durable_direct_move_commit_values(output, task_id, commits).await;
}

async fn send_durable_direct_move_commit_values(
    output: &mut IcedSender<Message>,
    task_id: u64,
    commits: Vec<DurableDirectMoveCommit>,
) {
    if commits.is_empty() {
        return;
    }
    let _ = output
        .send(Message::FileOperationDirectMovesCommitted { task_id, commits })
        .await;
}

fn history_safe_transfer(transfer: &QueuedTransfer) -> bool {
    matches!(
        transfer.conflict_strategy,
        TransferConflictStrategy::Fail
            | TransferConflictStrategy::KeepBoth
            | TransferConflictStrategy::Skip
    )
}

async fn send_copy_progress(
    output: &mut IcedSender<Message>,
    task_id: u64,
    record_index: usize,
    progress: CopyProgress,
    batch_progress: &mut TransferBatchProgress,
) {
    let Some(update) = batch_progress.observe_copy_progress(record_index, &progress) else {
        return;
    };
    send_file_operation_progress(output, task_id, update).await;
}

#[cfg(test)]
mod recoverable_transfer_tests {
    use std::sync::Arc;

    use super::*;
    use crate::operation_queue::{
        FileOperationEnqueueOutcome, FileOperationFinish, FileOperationQueue,
    };
    use iced::futures::StreamExt;

    use file_core::{
        persist_recoverable_source_manifest_with_controls,
        run_recoverable_transfer_to_direct_move_intent, DirectMoveIntentBoundary,
    };

    mod cancellation_terminalization;
    mod cross_filesystem_recovery;
    mod start_latency_harness;

    fn task_queue_transfer_journal_channel(
        store: TaskQueueStore,
        controls: FileOperationControls,
        _records: &[TransferJournalRecord],
    ) -> (
        TaskQueueTransferJournal,
        iced::futures::channel::mpsc::UnboundedReceiver<()>,
    ) {
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        drop(sender);
        (task_queue_transfer_journal(store, controls), receiver)
    }

    #[derive(Clone)]
    struct StopAfterDirectMoveRenamedJournal {
        inner: TaskQueueTransferJournal,
        renamed_committed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TransferJournal for StopAfterDirectMoveRenamedJournal {
        fn commit(
            &self,
            mutation: TransferJournalMutation,
        ) -> Pin<Box<dyn Future<Output = Result<u64, TransferJournalError>> + Send + '_>> {
            let commits_renamed = matches!(
                &mutation,
                TransferJournalMutation::CompareAndSwapCheckpoint {
                    checkpoint: file_core::TransferCheckpoint::DirectMoveRenamed(_),
                    ..
                }
            );
            let commits_after_renamed = matches!(
                &mutation,
                TransferJournalMutation::CompareAndSwapCheckpoint {
                    checkpoint: file_core::TransferCheckpoint::TargetCommitted(_),
                    ..
                }
            ) && self
                .renamed_committed
                .load(std::sync::atomic::Ordering::SeqCst);
            if commits_after_renamed {
                return Box::pin(async {
                    Err(TransferJournalError::Storage(
                        "stop after durable direct move rename".to_owned(),
                    ))
                });
            }
            let inner = self.inner.clone();
            let renamed_committed = self.renamed_committed.clone();
            Box::pin(async move {
                let revision = inner.commit(mutation).await?;
                if commits_renamed {
                    renamed_committed.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(revision)
            })
        }
    }

    #[derive(Clone)]
    struct ReplayDirectMoveRenamedJournal {
        inner: TaskQueueTransferJournal,
    }

    impl TransferJournal for ReplayDirectMoveRenamedJournal {
        fn commit(
            &self,
            mutation: TransferJournalMutation,
        ) -> Pin<Box<dyn Future<Output = Result<u64, TransferJournalError>> + Send + '_>> {
            if matches!(
                &mutation,
                TransferJournalMutation::CompareAndSwapCheckpoint {
                    checkpoint: file_core::TransferCheckpoint::DirectMoveRenamed(_),
                    ..
                }
            ) {
                return Box::pin(async move {
                    let revision = self.inner.commit(mutation.clone()).await?;
                    assert_eq!(
                        self.inner.commit(mutation).await,
                        Err(TransferJournalError::StaleRevision)
                    );
                    Ok(revision)
                });
            }
            self.inner.commit(mutation)
        }
    }

    #[test]
    fn application_stopping_has_recoverable_interruption_completion() {
        assert!(matches!(
            recoverable_transfer_failure(
                QueuedTransferMode::Copy,
                RecoverableTransferError::FileOperation(
                    file_core::FileError::ApplicationStopping,
                ),
                Vec::new(),
            ),
            FileOperationCompletion::RecoveryInterrupted(_, completed)
                if completed.is_empty()
        ));
    }

    #[test]
    fn recoverable_cancellation_has_typed_completion() {
        assert!(matches!(
            recoverable_transfer_failure(
                QueuedTransferMode::Move,
                RecoverableTransferError::FileOperation(file_core::FileError::Cancelled),
                vec![CompletedTransfer {
                    source: PathBuf::from("source"),
                    target: PathBuf::from("target"),
                }],
            ),
            FileOperationCompletion::Canceled(completed_move_transfers)
                if completed_move_transfers.len() == 1
        ));
    }

    #[tokio::test]
    async fn recoverable_copy_uses_one_manifest_byte_denominator_for_the_batch() {
        let directory = tempfile::tempdir().unwrap();
        let first_source = directory.path().join("first-source");
        let second_source = directory.path().join("second-source");
        let first_target = directory.path().join("first-target");
        let second_target = directory.path().join("second-target");
        tokio::fs::write(&first_source, vec![1_u8; 10])
            .await
            .unwrap();
        tokio::fs::write(&second_source, vec![2_u8; 990])
            .await
            .unwrap();
        let transfers = vec![
            QueuedTransfer::new(first_source, first_target),
            QueuedTransfer::new(second_source, second_target),
        ];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Copy {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable copy should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let (mut output, mut messages) = iced::futures::channel::mpsc::channel(32);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Copy,
            FileOperationVerification::BasicMetadata,
        )
        .await;
        drop(output);

        assert!(matches!(
            completion,
            FileOperationCompletion::Succeeded(FileOperationOutcome::Copy { .. })
        ));
        let mut byte_updates = Vec::new();
        while let Some(message) = messages.next().await {
            if let Message::FileOperationProgressed(
                id,
                FileOperationProgressUpdate::Bytes {
                    completed_bytes,
                    total_bytes,
                    completed_items,
                    total_items,
                },
            ) = message
            {
                assert_eq!(id, task_id);
                byte_updates.push((completed_bytes, total_bytes, completed_items, total_items));
            }
        }

        assert!(byte_updates.contains(&(10, 1_000, 1, 2)));
        assert_eq!(byte_updates.last(), Some(&(1_000, 1_000, 2, 2)));
        assert!(byte_updates
            .iter()
            .all(|(_, total_bytes, _, total_items)| *total_bytes == 1_000 && *total_items == 2));
    }

    #[tokio::test]
    async fn mismatched_verification_blocks_recovery_without_filesystem_effects() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        tokio::fs::write(&source, b"content").await.unwrap();
        let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Copy {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable copy should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(8);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Copy,
            FileOperationVerification::Strong,
        )
        .await;
        let FileOperationCompletion::RecoveryBlocked { error, .. } = completion else {
            panic!("verification mismatch must block recovery");
        };
        assert!(error.contains("does not match"));
        assert!(tokio::fs::symlink_metadata(&target).await.is_err());

        assert_eq!(
            queue.finish(task_id, FileOperationFinish::RecoveryBlocked(error)),
            (
                Some(crate::operation_queue::FileOperationTerminalStatus::Failed),
                None
            )
        );
        assert_eq!(
            store.read_task(stored_task_id).unwrap().unwrap().status,
            file_operation_store::StoredTaskStatus::Failed
        );
        assert!(!store
            .read_transfer_recovery(stored_task_id)
            .unwrap()
            .journal_entries
            .is_empty());
    }

    #[tokio::test]
    async fn sqlite_basic_move_emits_durable_target_commit_before_runner_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        tokio::fs::write(&source, b"move-content").await.unwrap();
        let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store);
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let (mut output, messages) = iced::futures::channel::mpsc::channel(32);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;
        drop(output);
        assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
        let messages = messages.collect::<Vec<_>>().await;
        assert!(messages.iter().any(|message| matches!(
            message,
            Message::FileOperationDirectMovesCommitted {
                task_id: committed_task_id,
                commits,
            } if *committed_task_id == task_id
                && matches!(
                    commits.as_slice(),
                    [DurableDirectMoveCommit {
                        work_key,
                        source: committed_source,
                        target: committed_target,
                        checkpoint_revision,
                    }] if work_key == &file_core::TransferWorkKey::top_level(0)
                        && committed_source == &source
                        && committed_target == &target
                        && *checkpoint_revision > 0
                )
        )));
    }

    #[tokio::test]
    async fn sqlite_basic_move_batch_makes_every_target_durable_before_item_completion() {
        let directory = tempfile::tempdir().unwrap();
        let mut transfers = Vec::new();
        for index in 0..3 {
            let source = directory.path().join(format!("source-{index}"));
            let target = directory.path().join(format!("target-{index}"));
            tokio::fs::write(&source, format!("content-{index}"))
                .await
                .unwrap();
            transfers.push(QueuedTransfer::new(source, target));
        }
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store);
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let (mut output, messages) = iced::futures::channel::mpsc::channel(64);

        let completion = run_queued_transfers(
            transfers.clone(),
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;
        drop(output);

        let FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
            transfers: completed,
            ..
        }) = completion
        else {
            panic!("expected move completion");
        };
        assert_eq!(
            completed
                .iter()
                .map(|transfer| (&transfer.source, &transfer.target))
                .collect::<Vec<_>>(),
            transfers
                .iter()
                .map(|transfer| (&transfer.source, &transfer.target))
                .collect::<Vec<_>>()
        );

        let messages = messages.collect::<Vec<_>>().await;
        let direct_commit_batches = messages
            .iter()
            .enumerate()
            .filter_map(|(position, message)| match message {
                Message::FileOperationDirectMovesCommitted {
                    task_id: committed_task_id,
                    commits,
                } => Some((position, *committed_task_id, commits.len())),
                _ => None,
            })
            .collect::<Vec<_>>();
        // rename 事实必须先于 durable 提交消息到达 UI：列表刷新不等 journal 落盘。
        let renamed_moves = messages
            .iter()
            .enumerate()
            .filter_map(|(position, message)| match message {
                Message::FileOperationMovesRenamed { moves, .. } => Some((position, moves.len())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(renamed_moves, vec![(0, transfers.len())]);
        let first_item_completion = messages
            .iter()
            .position(|message| matches!(message, Message::FileOperationProgressed(_, _)))
            .expect("batch should publish item completion progress");
        assert_eq!(direct_commit_batches, vec![(1, task_id, transfers.len())]);
        assert!(direct_commit_batches[0].0 < first_item_completion);
        let direct_commit_batch = messages
            .iter()
            .find_map(|message| match message {
                Message::FileOperationDirectMovesCommitted {
                    task_id: committed_task_id,
                    commits,
                } => Some((*committed_task_id, commits)),
                _ => None,
            })
            .expect("visibility segment should publish one commit batch");
        assert_eq!(direct_commit_batch.0, task_id);
        assert_eq!(direct_commit_batch.1.len(), transfers.len());
        for (index, (transfer, commit)) in transfers
            .iter()
            .zip(direct_commit_batch.1.iter())
            .enumerate()
        {
            assert_eq!(
                commit.work_key,
                file_core::TransferWorkKey::top_level(index as u64)
            );
            assert_eq!(commit.source, transfer.source);
            assert_eq!(commit.target, transfer.target);
            assert!(commit.checkpoint_revision > 0);
        }
    }

    #[tokio::test]
    async fn direct_move_segment_settles_visible_items_before_later_conflict() {
        let directory = tempfile::tempdir().unwrap();
        let first_source = directory.path().join("first-source");
        let second_source = directory.path().join("second-source");
        let third_source = directory.path().join("third-source");
        let first_target = directory.path().join("first-target");
        let second_target = directory.path().join("second-target");
        let third_target = directory.path().join("third-target");
        tokio::fs::write(&first_source, b"first").await.unwrap();
        tokio::fs::write(&second_source, b"second").await.unwrap();
        tokio::fs::write(&third_source, b"third").await.unwrap();
        tokio::fs::write(&second_target, b"occupied").await.unwrap();
        let transfers = vec![
            QueuedTransfer::new(first_source.clone(), first_target.clone()),
            QueuedTransfer::new(second_source.clone(), second_target.clone()),
            QueuedTransfer::new(third_source.clone(), third_target.clone()),
        ];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store);
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;

        let FileOperationCompletion::Failed {
            completed_move_transfers,
            ..
        } = completion
        else {
            panic!("later target conflict should fail the batch");
        };
        assert_eq!(
            completed_move_transfers,
            vec![CompletedTransfer {
                source: first_source.clone(),
                target: first_target.clone(),
            }]
        );
        assert_eq!(tokio::fs::read(&first_target).await.unwrap(), b"first");
        assert!(tokio::fs::symlink_metadata(&first_source).await.is_err());
        assert_eq!(tokio::fs::read(&second_source).await.unwrap(), b"second");
        assert_eq!(tokio::fs::read(&second_target).await.unwrap(), b"occupied");
        assert_eq!(tokio::fs::read(&third_source).await.unwrap(), b"third");
        assert!(tokio::fs::symlink_metadata(&third_target).await.is_err());
    }

    #[tokio::test]
    async fn direct_move_segment_finishes_other_visible_items_after_one_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let mut transfers = Vec::new();
        for index in 0..3 {
            let source = directory.path().join(format!("source-{index}"));
            let target = directory.path().join(format!("target-{index}"));
            tokio::fs::write(&source, format!("content-{index}"))
                .await
                .unwrap();
            transfers.push(QueuedTransfer::new(source, target));
        }
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let mut records = load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap();
        let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
        let mut manifest_controls = running.controls.clone();
        for record in &mut records {
            persist_recoverable_source_manifest_with_controls(
                record,
                &journal,
                &mut manifest_controls,
            )
            .await
            .unwrap();
        }
        let mut batch_progress = TransferBatchProgress::new(&records);
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);
        let mut durable_direct_moves = Vec::new();
        let mut intent_records = Vec::new();
        for (index, record) in records.into_iter().enumerate() {
            let DirectMoveIntentBoundary::Intent(record) =
                run_recoverable_transfer_to_direct_move_intent(
                    record,
                    &journal,
                    FileTransferOptions::new(running.controls.clone()),
                )
                .await
                .unwrap()
            else {
                panic!("all three moves should prepare a direct move intent");
            };
            intent_records.push((index, record));
        }
        let records: Vec<TransferJournalRecord> = intent_records
            .iter()
            .map(|(_, record)| record.clone())
            .collect();
        let batch = run_direct_move_batch_to_durable_renamed(
            records,
            &journal,
            &FileTransferOptions::new(running.controls.clone()),
            &mut |_| {},
        )
        .await
        .unwrap();
        for (record, (index, _)) in batch.into_iter().zip(intent_records) {
            let DirectMoveBatchRecord::Renamed(record) = record else {
                panic!("all three moves should reach durable renamed visibility");
            };
            durable_direct_moves.push((index, record));
        }

        let displaced_target = directory.path().join("displaced-target");
        tokio::fs::rename(&transfers[1].target, &displaced_target)
            .await
            .unwrap();
        tokio::fs::write(&transfers[1].target, b"external")
            .await
            .unwrap();

        let mut completed = Vec::new();
        assert!(matches!(
            complete_durable_direct_move_segment(
                &mut durable_direct_moves,
                &running.controls,
                &journal,
                &FileTransferOptions::new(running.controls.clone()),
                task_id,
                &mut output,
                &mut batch_progress,
                &mut completed,
            )
            .await,
            Err(RecoverableTransferError::RecoveryBlocked { .. })
        ));
        assert_eq!(
            completed,
            vec![
                CompletedTransfer {
                    source: transfers[0].source.clone(),
                    target: transfers[0].target.clone(),
                },
                CompletedTransfer {
                    source: transfers[2].source.clone(),
                    target: transfers[2].target.clone(),
                },
            ]
        );
        assert_eq!(
            tokio::fs::read(&transfers[1].target).await.unwrap(),
            b"external"
        );
        assert_eq!(
            tokio::fs::read(displaced_target).await.unwrap(),
            b"content-1"
        );
        let restored = load_recoverable_transfer_records(store, stored_task_id)
            .await
            .unwrap();
        assert!(matches!(
            restored[0].checkpoint,
            file_core::TransferCheckpoint::Completed(_)
        ));
        assert!(matches!(
            restored[1].checkpoint,
            file_core::TransferCheckpoint::DirectMoveRenamed(_)
        ));
        assert!(matches!(
            restored[2].checkpoint,
            file_core::TransferCheckpoint::Completed(_)
        ));
    }

    #[tokio::test]
    async fn sqlite_merge_move_emits_the_nested_durable_target_commit() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        let source_child = source.join("child");
        let target_child = target.join("child");
        tokio::fs::create_dir(&source).await.unwrap();
        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::write(&source_child, b"nested-move")
            .await
            .unwrap();
        let transfers = vec![QueuedTransfer {
            source: source.clone(),
            target: target.clone(),
            conflict_strategy: TransferConflictStrategy::Merge,
        }];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store);
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable merge move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let (mut output, messages) = iced::futures::channel::mpsc::channel(32);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;
        drop(output);
        assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
        let messages = messages.collect::<Vec<_>>().await;
        assert!(messages.iter().any(|message| matches!(
            message,
            Message::FileOperationDirectMovesCommitted {
                task_id: committed_task_id,
                commits,
            } if *committed_task_id == task_id
                && matches!(
                    commits.as_slice(),
                    [DurableDirectMoveCommit {
                        work_key,
                        source: committed_source,
                        target: committed_target,
                        checkpoint_revision,
                    }] if work_key.transfer_index == 0
                        && work_key.relative_path.as_path() == std::path::Path::new("child")
                        && committed_source == &source_child
                        && committed_target == &target_child
                        && *checkpoint_revision > 0
                )
        )));
        assert!(tokio::fs::symlink_metadata(&source).await.is_err());
        assert_eq!(
            tokio::fs::read(&target_child).await.unwrap(),
            b"nested-move"
        );
    }

    #[tokio::test]
    async fn restored_direct_move_renamed_replays_its_durable_commit() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        tokio::fs::write(&source, b"direct-move").await.unwrap();
        let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let record = load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap()
            .remove(0);
        let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
        let stopping_journal = StopAfterDirectMoveRenamedJournal {
            inner: journal,
            renamed_committed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        assert!(matches!(
            run_recoverable_transfer(
                record,
                &stopping_journal,
                FileTransferOptions::new(running.controls.clone()),
            )
            .await,
            Err(RecoverableTransferError::Journal { .. })
        ));
        let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
        assert_eq!(
            snapshot.journal_entries[0].checkpoint.kind,
            file_operation_store::StoredTransferCheckpointKind::DirectMoveRenamed
        );

        let (mut output, messages) = iced::futures::channel::mpsc::channel(32);
        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;
        drop(output);
        assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
        let commits = messages
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter(|message| {
                matches!(
                    message,
                    Message::FileOperationDirectMovesCommitted {
                        task_id: committed_task_id,
                        commits,
                    } if *committed_task_id == task_id
                        && matches!(
                            commits.as_slice(),
                            [DurableDirectMoveCommit {
                                source: committed_source,
                                target: committed_target,
                                ..
                            }] if committed_source == &source && committed_target == &target
                        )
                )
            })
            .count();
        assert_eq!(commits, 1);
    }

    #[tokio::test]
    async fn duplicate_direct_move_cas_is_rejected_and_original_completes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        tokio::fs::write(&source, b"direct-move").await.unwrap();
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Move {
                transfers: vec![QueuedTransfer::new(source, target.clone())],
                verification: FileOperationVerification::BasicMetadata,
            })
        else {
            panic!("recoverable move should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let record = load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap()
            .remove(0);
        let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
        let replaying_journal = ReplayDirectMoveRenamedJournal { inner: journal };

        run_recoverable_transfer(
            record,
            &replaying_journal,
            FileTransferOptions::new(running.controls.clone()),
        )
        .await
        .unwrap();

        let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
        assert_eq!(
            snapshot.journal_entries[0].checkpoint.kind,
            file_operation_store::StoredTransferCheckpointKind::Completed
        );
        assert_eq!(tokio::fs::read(target).await.unwrap(), b"direct-move");
    }

    #[tokio::test]
    async fn sqlite_replace_runner_persists_backup_manifest_until_terminal_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        tokio::fs::write(&source, b"replacement").await.unwrap();
        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::write(target.join("old.txt"), b"old-target")
            .await
            .unwrap();
        let transfers = vec![QueuedTransfer {
            source,
            target: target.clone(),
            conflict_strategy: TransferConflictStrategy::Replace,
        }];
        let store = TaskQueueStore::new(directory.path().join("replace-state.sqlite")).unwrap();
        let mut queue = FileOperationQueue::new();
        queue.set_store(store.clone());
        let FileOperationEnqueueOutcome::Queued { task_id } =
            queue.enqueue(QueuedFileOperation::Copy {
                transfers: transfers.clone(),
                verification: FileOperationVerification::Strong,
            })
        else {
            panic!("recoverable replace copy should enqueue");
        };
        let running = queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(32);

        let completion = run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &mut output,
            running.store,
            QueuedTransferMode::Copy,
            FileOperationVerification::Strong,
        )
        .await;
        assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"replacement");
        let snapshot = store.read_transfer_recovery(stored_task_id).unwrap();
        assert!(!snapshot.replacement_manifest_entries.is_empty());

        assert_eq!(
            queue.finish(task_id, FileOperationFinish::Succeeded),
            (
                Some(crate::operation_queue::FileOperationTerminalStatus::Completed),
                None
            )
        );
        assert!(store
            .read_transfer_recovery(stored_task_id)
            .unwrap()
            .journal_entries
            .is_empty());
    }

    #[tokio::test]
    async fn sqlite_move_runner_preserves_committed_item_and_history_after_restart() {
        // migrate durable journal test

        let directory = tempfile::tempdir().unwrap();
        let first_source = directory.path().join("first-move-source");
        let second_source = directory.path().join("second-move-source");
        let first_target = directory.path().join("first-move-target");
        let second_target = directory.path().join("second-move-target");
        tokio::fs::write(&first_source, b"first-move")
            .await
            .unwrap();
        tokio::fs::write(&second_source, b"second-move")
            .await
            .unwrap();
        let transfers = vec![
            QueuedTransfer::new(first_source.clone(), first_target.clone()),
            QueuedTransfer::new(second_source.clone(), second_target.clone()),
        ];
        let store = TaskQueueStore::new(directory.path().join("move-state.sqlite")).unwrap();
        let mut original_queue = FileOperationQueue::new();
        original_queue.set_store(store.clone());
        assert!(matches!(
            original_queue.enqueue(QueuedFileOperation::Move {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            }),
            FileOperationEnqueueOutcome::Queued { .. }
        ));
        let task_id = original_queue.tasks()[0].id;
        let running = original_queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let mut records = load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap();
        let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);
        let mut batch_progress = TransferBatchProgress::new(&records);

        run_recoverable_record_with_progress(
            records.remove(0),
            running.controls.clone(),
            &journal,
            &FileTransferOptions::new(running.controls.clone()),
            task_id,
            &mut output,
            0,
            &mut batch_progress,
        )
        .await
        .unwrap();
        assert!(tokio::fs::symlink_metadata(&first_source).await.is_err());
        drop(original_queue);

        let mut restored_queue = FileOperationQueue::new();
        assert!(restored_queue
            .set_store_and_restore(store.clone())
            .is_none());
        let restored_running = restored_queue.active_subscription().unwrap();
        let stored_task_id = restored_running.stored_id.unwrap();
        let completion = run_queued_transfers(
            transfers,
            restored_running.controls,
            restored_running.stored_id.unwrap(),
            task_id,
            &mut output,
            restored_running.store,
            QueuedTransferMode::Move,
            FileOperationVerification::BasicMetadata,
        )
        .await;

        let FileOperationCompletion::Succeeded(FileOperationOutcome::Move {
            transfers,
            history_eligibility: FileOperationHistoryEligibility::Replayable,
        }) = completion
        else {
            panic!("expected resumed move completion");
        };
        assert_eq!(transfers.len(), 2);
        assert_eq!(tokio::fs::read(&first_target).await.unwrap(), b"first-move");
        assert_eq!(
            tokio::fs::read(&second_target).await.unwrap(),
            b"second-move"
        );
        assert!(tokio::fs::symlink_metadata(&second_source).await.is_err());
        assert_eq!(
            restored_queue.finish(stored_task_id, FileOperationFinish::Succeeded),
            (
                Some(crate::operation_queue::FileOperationTerminalStatus::Completed),
                None
            )
        );
        assert!(store
            .read_transfer_recovery(stored_task_id)
            .unwrap()
            .journal_entries
            .is_empty());
    }

    #[tokio::test]
    async fn sqlite_runner_skips_committed_batch_item_and_finishes_remaining_item() {
        // migrate committed batch test

        let directory = tempfile::tempdir().unwrap();
        let first_source = directory.path().join("first-source");
        let second_source = directory.path().join("second-source");
        let first_target = directory.path().join("first-target");
        let second_target = directory.path().join("second-target");
        tokio::fs::write(&first_source, b"first-original")
            .await
            .unwrap();
        tokio::fs::write(&second_source, b"second-original")
            .await
            .unwrap();
        let transfers = vec![
            QueuedTransfer::new(first_source.clone(), first_target.clone()),
            QueuedTransfer::new(second_source.clone(), second_target.clone()),
        ];
        let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
        let mut original_queue = FileOperationQueue::new();
        original_queue.set_store(store.clone());
        assert!(matches!(
            original_queue.enqueue(QueuedFileOperation::Copy {
                transfers: transfers.clone(),
                verification: FileOperationVerification::BasicMetadata,
            }),
            FileOperationEnqueueOutcome::Queued { .. }
        ));
        let task_id = original_queue.tasks()[0].id;
        let running = original_queue.active_subscription().unwrap();
        let stored_task_id = running.stored_id.unwrap();
        let mut records = load_recoverable_transfer_records(store.clone(), stored_task_id)
            .await
            .unwrap();
        let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
        let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);
        let mut batch_progress = TransferBatchProgress::new(&records);

        run_recoverable_record_with_progress(
            records.remove(0),
            running.controls.clone(),
            &journal,
            &FileTransferOptions::new(running.controls.clone()),
            task_id,
            &mut output,
            0,
            &mut batch_progress,
        )
        .await
        .unwrap();
        tokio::fs::write(&first_source, b"first-changed-after-commit")
            .await
            .unwrap();
        drop(original_queue);

        let mut restored_queue = FileOperationQueue::new();
        assert!(restored_queue
            .set_store_and_restore(store.clone())
            .is_none());
        let restored_running = restored_queue.active_subscription().unwrap();
        let stored_task_id = restored_running.stored_id.unwrap();
        let stored_task_id = restored_running.stored_id.unwrap();
        let completion = run_queued_transfers(
            transfers,
            restored_running.controls,
            restored_running.stored_id.unwrap(),
            task_id,
            &mut output,
            restored_running.store,
            QueuedTransferMode::Copy,
            FileOperationVerification::BasicMetadata,
        )
        .await;

        let FileOperationCompletion::Succeeded(FileOperationOutcome::Copy { transfers }) =
            completion
        else {
            panic!("expected resumed copy completion");
        };
        assert_eq!(transfers.len(), 2);
        assert_eq!(
            tokio::fs::read(&first_target).await.unwrap(),
            b"first-original"
        );
        assert_eq!(
            tokio::fs::read(&second_target).await.unwrap(),
            b"second-original"
        );
        assert_eq!(
            restored_queue.finish(stored_task_id, FileOperationFinish::Succeeded),
            (
                Some(crate::operation_queue::FileOperationTerminalStatus::Completed),
                None
            )
        );
        assert_eq!(
            store.read_task(stored_task_id).unwrap().unwrap().status,
            file_operation_store::StoredTaskStatus::Completed
        );
        assert!(store
            .read_transfer_recovery(stored_task_id)
            .unwrap()
            .journal_entries
            .is_empty());
    }
}
