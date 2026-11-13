use super::*;
use file_core::{DurableDirectMoveCommit, TransferSegmentEvent};

mod progress;
mod store_codec;

use progress::{record_workload, send_transfer_batch_progress, TransferBatchProgress};
pub(super) use progress::{send_archive_progress, send_file_operation_progress};

/// 一段最多多少个文件、多少字节：段内每一步只落盘一次、写一次日志，
/// 段越大单个文件分摊的落盘越少，但文件要等整段走完同一步才一起出现。
const TRANSFER_SEGMENT_FILES: usize = 256;
const TRANSFER_SEGMENT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone)]
struct TaskQueueTransferJournal {
    store: TaskQueueStore,
    controls: FileOperationControls,
}

impl TransferJournal for TaskQueueTransferJournal {
    fn commit(&self, mutations: Vec<TransferJournalMutation>) -> TransferJournalFuture<'_> {
        let store = self.store.clone();
        let controls = self.controls.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let keep_going = || controls.checkpoint_now().is_ok();
                let mut stored = Vec::with_capacity(mutations.len());
                for mutation in &mutations {
                    let encoded = store_codec::encode_mutation(mutation, keep_going)
                        .map_err(store_journal_error)?
                        .ok_or_else(|| interrupted_journal_error(&controls))?;
                    stored.push(encoded);
                }
                store
                    .apply_transfer_mutations_while(&stored, keep_going)
                    .map_err(store_journal_error)?
                    .ok_or_else(|| interrupted_journal_error(&controls))
            })
            .await
            .map_err(|error| TransferJournalError::Storage(error.to_string()))?
        })
    }
}

fn store_journal_error(error: bennu_state::StoreError) -> TransferJournalError {
    match error {
        bennu_state::StoreError::StaleTransferRevision { .. } => {
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
    output: &FileOperationEventSender,
    store: Option<TaskQueueStore>,
    mode: QueuedTransferMode,
    verification: FileOperationVerification,
) -> FileOperationCompletion {
    let Some(store) = store else {
        return FileOperationCompletion::from_result(Err(
            "File operation queue storage is unavailable; transfer was not started".to_owned(),
        ));
    };
    let mut records = match load_recoverable_transfer_records(store.clone(), stored_task_id).await {
        Ok(records) => records,
        Err(RecoverableRecordLoadError::Interrupted(error)) => {
            return FileOperationCompletion::RecoveryInterrupted(
                error,
                FileOperationOutcome::NoHistory,
            );
        }
        Err(RecoverableRecordLoadError::Invalid(error)) => {
            return FileOperationCompletion::RecoveryBlocked {
                error,
                partial_outcome: FileOperationOutcome::NoHistory,
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
            partial_outcome: FileOperationOutcome::NoHistory,
        };
    }

    let history_eligibility = if transfers.iter().all(history_safe_transfer) {
        FileOperationHistoryEligibility::Replayable
    } else {
        FileOperationHistoryEligibility::NotReplayable
    };
    let journal = TaskQueueTransferJournal {
        store,
        controls: controls.clone(),
    };
    let settle = |error, completed| {
        settle_recoverable_transfer_failure(
            mode,
            history_eligibility,
            error,
            completed,
            stored_task_id,
            task_id,
            output,
            &journal,
            &controls,
        )
    };

    if let Err(error) =
        prepare_transfer_manifests(&mut records, &journal, &controls, task_id, output).await
    {
        return settle(error, SettledTransfers::default()).await;
    }
    let mut runner = TransferRunner::new(&records, mode, task_id, output);
    let options = FileTransferOptions::new(controls.clone());
    // 一个文件出错不拦其它文件：能做的都做完，任务最后按出错的那个收尾。
    // 取消和整段停下（日志、落盘、退出）才立刻结算。
    let mut record_failure = None;
    for segment in transfer_segments(records) {
        match runner.run_segment(segment, &journal, &options).await {
            Err(error) => return settle(error, runner.completed).await,
            Ok(Some(error @ RecoverableTransferError::FileOperation(FileError::Cancelled))) => {
                return settle(error, runner.completed).await;
            }
            Ok(Some(error)) => {
                record_failure.get_or_insert(error);
            }
            Ok(None) => {}
        }
    }
    if let Some(error) = record_failure {
        return settle(error, runner.completed).await;
    }

    FileOperationCompletion::Succeeded(settled_transfer_outcome(
        mode,
        history_eligibility,
        runner.completed,
    ))
}

/// 成功和中途停下用同一种结果：停下时它就是已完成的那部分。
fn settled_transfer_outcome(
    mode: QueuedTransferMode,
    history_eligibility: FileOperationHistoryEligibility,
    completed: SettledTransfers,
) -> FileOperationOutcome {
    let SettledTransfers {
        transfers,
        metadata_loss,
    } = completed;
    match mode {
        QueuedTransferMode::Copy => FileOperationOutcome::Copy {
            transfers,
            history_eligibility,
            metadata_loss,
        },
        QueuedTransferMode::Move => FileOperationOutcome::Move {
            transfers,
            history_eligibility,
            metadata_loss,
        },
    }
}

/// 清单要在动手前建好：进度的总数来自它，分段也按它算。建清单时界面显示
/// “准备中 · 已扫描 N 项”。每段的清单用一个事务装进日志。
async fn prepare_transfer_manifests(
    records: &mut [TransferJournalRecord],
    journal: &TaskQueueTransferJournal,
    controls: &FileOperationControls,
    task_id: u64,
    output: &FileOperationEventSender,
) -> Result<(), RecoverableTransferError> {
    let scanned = AtomicUsize::new(0);
    let preparation = async {
        for chunk in records.chunks_mut(TRANSFER_SEGMENT_FILES) {
            let build_errors =
                file_core::persist_source_manifests(chunk, journal, controls, &scanned).await?;
            for (record, error) in chunk.iter().zip(build_errors) {
                let Some(error) = error else { continue };
                // 这份清单只为提前算出进度总量。建不出来时这一项的进度显示为未知；
                // 推进到它时会重新建清单，真正的错误在那里报给用户。
                let log_error =
                    file_core::log_detail::sanitized_application_log_detail(&error.to_string());
                tracing::warn!(
                    target: "app_ui::file_operations",
                    event = "progress_manifest_unavailable",
                    source = %record.request.source.display(),
                    error = %log_error,
                    "transfer progress manifest could not be built; progress is unknown"
                );
            }
        }
        Ok(())
    };
    tokio::pin!(preparation);
    let mut ticker = tokio::time::interval(BYTE_PROGRESS_UI_INTERVAL);
    let mut reported = 0;
    loop {
        tokio::select! {
            result = &mut preparation => return result,
            _ = ticker.tick() => {
                let count = scanned.load(Ordering::Relaxed);
                if count != reported {
                    reported = count;
                    let _ = output
                        .send(FileOperationEvent::Scanned {
                            task_id,
                            scanned_items: count,
                        })
                        .await;
                }
            }
        }
    }
}

/// 顺序切段：累计满 `TRANSFER_SEGMENT_FILES` 个文件或 `TRANSFER_SEGMENT_BYTES`
/// 字节就切；一条记录自己超过上限时单独成段。
fn transfer_segments(
    records: Vec<TransferJournalRecord>,
) -> Vec<Vec<(usize, TransferJournalRecord)>> {
    let mut segments = Vec::new();
    let mut segment = Vec::new();
    let (mut files, mut bytes) = (0_usize, 0_u64);
    for (index, record) in records.into_iter().enumerate() {
        let (record_files, record_bytes) = record_workload(&record);
        if !segment.is_empty()
            && (files + record_files > TRANSFER_SEGMENT_FILES
                || bytes.saturating_add(record_bytes) > TRANSFER_SEGMENT_BYTES)
        {
            segments.push(std::mem::take(&mut segment));
            (files, bytes) = (0, 0);
        }
        files += record_files;
        bytes = bytes.saturating_add(record_bytes);
        segment.push((index, record));
    }
    if !segment.is_empty() {
        segments.push(segment);
    }
    segments
}

/// 跑段并把段里发生的事转给界面：进度、改名、落盘的直接移动、每条记录的结果。
/// `completed` 收下成功的记录，任务在哪里停下都是它的部分结果。
struct TransferRunner<'a> {
    progress: TransferBatchProgress,
    /// 每条进度消息都带全部记录的占位快照：记录成批完成时逐条发会让消息量
    /// 随文件数平方增长，所以和字节进度一样节流，段结束时补发最终状态。
    last_progress_sent_at: Option<Instant>,
    completed: SettledTransfers,
    task_id: u64,
    output: &'a FileOperationEventSender,
}

impl<'a> TransferRunner<'a> {
    fn new(
        records: &[TransferJournalRecord],
        mode: QueuedTransferMode,
        task_id: u64,
        output: &'a FileOperationEventSender,
    ) -> Self {
        Self {
            progress: TransferBatchProgress::new(records, mode),
            last_progress_sent_at: None,
            completed: SettledTransfers::default(),
            task_id,
            output,
        }
    }

    /// 外层错误让整段停下；`Ok` 里是段内记录自己的第一个错误（同段其它
    /// 记录照常做完）。
    async fn run_segment(
        &mut self,
        segment: Vec<(usize, TransferJournalRecord)>,
        journal: &TaskQueueTransferJournal,
        options: &FileTransferOptions,
    ) -> Result<Option<RecoverableTransferError>, RecoverableTransferError> {
        let (indices, records): (Vec<usize>, Vec<_>) = segment.into_iter().unzip();
        // 重启后已经停在“改名已落盘”的移动：界面要再收到一次这个事实。
        let restored_moves = records
            .iter()
            .filter_map(|record| file_core::durable_direct_move_commit(record, None))
            .collect::<Vec<_>>();
        self.send_direct_moves_committed(restored_moves).await;
        self.progress.start_records(&indices);
        self.send_progress().await;

        let (copy_sender, mut copy_progress) = tokio::sync::mpsc::unbounded_channel();
        let (event_sender, mut segment_events) = tokio::sync::mpsc::unbounded_channel();
        let options = options.clone().with_progress_sender(copy_sender);
        let run = file_core::run_recoverable_segment(records, journal, &options, &event_sender);
        tokio::pin!(run);
        let mut first_error = None;
        let result = loop {
            tokio::select! {
                Some(progress) = copy_progress.recv() => {
                    self.progress.observe_copy_progress(&progress);
                }
                Some(event) = segment_events.recv() => {
                    self.handle_segment_event(event, &indices, &mut first_error).await;
                }
                result = &mut run => break result,
            }
            if should_send_byte_progress(self.last_progress_sent_at, Instant::now()) {
                self.send_progress().await;
            }
        };
        // 读空再结算：完成计数和记录结果都不能丢。
        while let Ok(progress) = copy_progress.try_recv() {
            self.progress.observe_copy_progress(&progress);
        }
        while let Ok(event) = segment_events.try_recv() {
            self.handle_segment_event(event, &indices, &mut first_error)
                .await;
        }
        self.send_progress().await;
        result?;
        Ok(first_error)
    }

    async fn handle_segment_event(
        &mut self,
        event: TransferSegmentEvent,
        indices: &[usize],
        first_error: &mut Option<RecoverableTransferError>,
    ) {
        match event {
            // rename 全部完成后立即上报（先于落盘和日志），让列表刷新不等。通道满时
            // 等界面取走：列表增量、路径迁移和搜索刷新都靠这条消息，丢了补不回来。
            TransferSegmentEvent::MovesRenamed(moves) => {
                let _ = self
                    .output
                    .send(FileOperationEvent::MovesRenamed {
                        task_id: self.task_id,
                        moves: moves
                            .into_iter()
                            .map(|(source, target)| CompletedTransfer { source, target })
                            .collect(),
                    })
                    .await;
            }
            TransferSegmentEvent::DirectMovesCommitted(commits) => {
                self.send_direct_moves_committed(commits).await;
            }
            TransferSegmentEvent::RecordFinished { index, result } => match result {
                Ok(outcome) => {
                    accept_recoverable_transfer_outcome(outcome, &mut self.completed);
                    self.progress.complete_record(indices[index]);
                }
                // 被取消的记录说明不了什么；同段里真正出错的那条更该报给用户。
                Err(error) => {
                    if first_error.as_ref().is_none_or(|kept| {
                        matches!(
                            kept,
                            RecoverableTransferError::FileOperation(FileError::Cancelled)
                        )
                    }) {
                        *first_error = Some(error);
                    }
                }
            },
        }
    }

    async fn send_progress(&mut self) {
        send_transfer_batch_progress(
            self.output,
            self.task_id,
            &self.progress,
            self.progress.snapshot(),
        )
        .await;
        self.last_progress_sent_at = Some(Instant::now());
    }

    async fn send_direct_moves_committed(&self, commits: Vec<DurableDirectMoveCommit>) {
        if commits.is_empty() {
            return;
        }
        let _ = self
            .output
            .send(FileOperationEvent::DirectMovesCommitted {
                task_id: self.task_id,
                commits,
            })
            .await;
    }
}

/// 这些错误类的完成语义本身要求保留恢复细节(RecoveryInterrupted/
/// RecoveryBlocked),队列不会以 FinalizeRecovery 收尾,因此不由失败结算关账。
fn failure_preserves_recovery_details(error: &RecoverableTransferError) -> bool {
    matches!(
        error,
        RecoverableTransferError::Journal { .. }
            | RecoverableTransferError::RecoveryRequired { .. }
            | RecoverableTransferError::RecoveryBlocked { .. }
    ) || matches!(
        error,
        RecoverableTransferError::FileOperation(FileError::ApplicationStopping)
    )
}

async fn settle_recoverable_transfer_failure(
    mode: QueuedTransferMode,
    history_eligibility: FileOperationHistoryEligibility,
    error: RecoverableTransferError,
    completed: SettledTransfers,
    stored_task_id: u64,
    task_id: u64,
    output: &FileOperationEventSender,
    journal: &TaskQueueTransferJournal,
    controls: &FileOperationControls,
) -> FileOperationCompletion {
    let cancellation_demanded = matches!(
        &error,
        RecoverableTransferError::FileOperation(FileError::Cancelled)
    ) || matches!(controls.checkpoint_now(), Err(FileError::Cancelled));
    let partial_outcome = settled_transfer_outcome(mode, history_eligibility, completed);
    if !cancellation_demanded && failure_preserves_recovery_details(&error) {
        return recoverable_transfer_failure(error, partial_outcome);
    }

    let records =
        match load_recoverable_transfer_records(journal.store.clone(), stored_task_id).await {
            Ok(records) => records,
            Err(RecoverableRecordLoadError::Interrupted(load_error)) => {
                return FileOperationCompletion::RecoveryInterrupted(load_error, partial_outcome);
            }
            Err(RecoverableRecordLoadError::Invalid(load_error)) => {
                return FileOperationCompletion::RecoveryBlocked {
                    error: load_error,
                    partial_outcome,
                };
            }
        };

    if cancellation_demanded {
        return settle_cancellation_records(
            records,
            mode,
            history_eligibility,
            error,
            task_id,
            output,
            journal,
            controls,
        )
        .await;
    }

    // 普通失败与取消一样必须关账:把所有未终态记录就地结算为失败,队列才
    // 能以 FinalizeRecovery 写入失败状态。任一记录无法结算(日志故障或
    // forward-only 检查点)时降级为 RecoveryInterrupted 保留恢复细节,交给
    // 启动恢复收敛,避免任务卡在数据库中间状态、每次启动重跑重报。
    if let Err(settlement_error) =
        file_core::settle_failed_recoverable_transfers(records, journal, &error.to_string()).await
    {
        return FileOperationCompletion::RecoveryInterrupted(
            settlement_error.to_string(),
            partial_outcome,
        );
    }
    recoverable_transfer_failure(error, partial_outcome)
}

async fn settle_cancellation_records(
    records: Vec<TransferJournalRecord>,
    mode: QueuedTransferMode,
    history_eligibility: FileOperationHistoryEligibility,
    error: RecoverableTransferError,
    task_id: u64,
    output: &FileOperationEventSender,
    journal: &TaskQueueTransferJournal,
    controls: &FileOperationControls,
) -> FileOperationCompletion {
    let mut runner = TransferRunner::new(&records, mode, task_id, output);
    let settlement_options = FileTransferOptions::new(controls.clone());
    let segment = records.into_iter().enumerate().collect();
    let settlement_error = match runner
        .run_segment(segment, journal, &settlement_options)
        .await
    {
        Ok(None)
        | Ok(Some(RecoverableTransferError::FileOperation(FileError::Cancelled)))
        | Err(RecoverableTransferError::FileOperation(FileError::Cancelled)) => None,
        Ok(Some(error)) | Err(error) => Some(error),
    };

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
    // 取消/失败的任务以错误收尾,完成提示只属于成功的任务;已提交的部分
    // 照样迁移路径、进撤销历史。
    recoverable_transfer_failure(
        error,
        settled_transfer_outcome(mode, history_eligibility, runner.completed),
    )
}

fn recoverable_transfer_failure(
    error: RecoverableTransferError,
    partial_outcome: FileOperationOutcome,
) -> FileOperationCompletion {
    let diagnostic = error.to_string();
    match error {
        RecoverableTransferError::FileOperation(file_core::FileError::ApplicationStopping)
        | RecoverableTransferError::Journal { .. }
        | RecoverableTransferError::RecoveryRequired { .. } => {
            FileOperationCompletion::RecoveryInterrupted(diagnostic, partial_outcome)
        }
        RecoverableTransferError::FileOperation(file_core::FileError::Cancelled) => {
            FileOperationCompletion::Canceled(partial_outcome)
        }
        RecoverableTransferError::RecoveryBlocked { .. } => {
            FileOperationCompletion::RecoveryBlocked {
                error: diagnostic,
                partial_outcome,
            }
        }
        _ => FileOperationCompletion::Failed {
            error: diagnostic,
            partial_outcome,
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

/// 已完成的传输:路径迁移与撤销历史用的成对路径,加上内容已提交、但没能
/// 保留下来的元数据(任务成功收尾时变成完成提示)。
#[derive(Debug, Default)]
struct SettledTransfers {
    transfers: Vec<CompletedTransfer>,
    metadata_loss: file_core::MetadataLossSummary,
}

fn accept_recoverable_transfer_outcome(
    outcome: RecoverableTransferOutcome,
    completed: &mut SettledTransfers,
) {
    let RecoverableTransferOutcome {
        source,
        final_target,
        metadata_loss,
    } = outcome;
    completed.metadata_loss.absorb(metadata_loss);
    if let Some(target) = final_target {
        completed
            .transfers
            .push(CompletedTransfer { source, target });
    }
}

fn history_safe_transfer(transfer: &QueuedTransfer) -> bool {
    matches!(
        transfer.conflict_strategy,
        TransferConflictStrategy::Fail
            | TransferConflictStrategy::KeepBoth
            | TransferConflictStrategy::Skip
    )
}

#[cfg(test)]
mod recoverable_transfer_tests;
