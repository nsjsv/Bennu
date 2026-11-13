//! A segment advances many records in rounds, one journal transaction per
//! round. A crash can only land between a round's filesystem effects and its
//! commit; every record must then recover from its last committed checkpoint.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

struct RecordState {
    revision: u64,
    checkpoint: TransferCheckpoint,
    manifest: Option<SourceManifest>,
    replacement_manifest: Option<SourceManifest>,
}

struct MultiRecordMemoryJournal {
    task_id: u64,
    states: Mutex<BTreeMap<u64, RecordState>>,
    commits: AtomicUsize,
    /// The commit with this number fails as if the process died just before it.
    crash_at_commit: Mutex<Option<usize>>,
}

impl MultiRecordMemoryJournal {
    fn new(task_id: u64, records: usize) -> Self {
        Self {
            task_id,
            states: Mutex::new(
                (0..records as u64)
                    .map(|index| {
                        (
                            index,
                            RecordState {
                                revision: 0,
                                checkpoint: TransferCheckpoint::AwaitingManifest,
                                manifest: None,
                                replacement_manifest: None,
                            },
                        )
                    })
                    .collect(),
            ),
            commits: AtomicUsize::new(0),
            crash_at_commit: Mutex::new(None),
        }
    }

    fn record(&self, index: u64, request: RecoverableTransferRequest) -> TransferJournalRecord {
        let states = self.states.lock().unwrap();
        let state = &states[&index];
        TransferJournalRecord {
            task_id: self.task_id,
            key: TransferWorkKey::top_level(index),
            request,
            checkpoint: state.checkpoint.clone(),
            revision: state.revision,
            manifest: state.manifest.clone(),
            replacement_manifest: state.replacement_manifest.clone(),
        }
    }
}

impl TransferJournal for MultiRecordMemoryJournal {
    fn commit(&self, mutations: Vec<TransferJournalMutation>) -> TransferJournalFuture<'_> {
        Box::pin(async move {
            let commit = self.commits.fetch_add(1, Ordering::SeqCst) + 1;
            if *self.crash_at_commit.lock().unwrap() == Some(commit) {
                return Err(TransferJournalError::Storage(format!(
                    "crashed before commit {commit}"
                )));
            }
            let mut states = self.states.lock().unwrap();
            for mutation in &mutations {
                let state = &states[&mutation.key.transfer_index];
                if mutation.task_id != self.task_id || mutation.expected_revision != state.revision
                {
                    return Err(TransferJournalError::StaleRevision);
                }
            }
            let mut revisions = Vec::with_capacity(mutations.len());
            for mutation in mutations {
                let state = states.get_mut(&mutation.key.transfer_index).unwrap();
                match mutation.transition {
                    TransferTransition::ManifestAndCheckpoint {
                        manifest,
                        replacement_manifest,
                        checkpoint,
                    } => {
                        state.manifest = Some(manifest);
                        state.replacement_manifest = replacement_manifest;
                        state.checkpoint = checkpoint;
                    }
                    TransferTransition::Checkpoint(checkpoint)
                    | TransferTransition::MergeCompletionAndCheckpoint { checkpoint, .. } => {
                        state.checkpoint = checkpoint;
                    }
                }
                state.revision += 1;
                revisions.push(state.revision);
            }
            Ok(revisions)
        })
    }
}

struct SegmentFixture {
    _directory: tempfile::TempDir,
    sources: PathBuf,
    targets: PathBuf,
    requests: Vec<RecoverableTransferRequest>,
    journal: MultiRecordMemoryJournal,
}

impl SegmentFixture {
    fn new(count: usize, operation: RecoverableTransferOperation) -> Self {
        let directory = tempdir().unwrap();
        let sources = directory.path().join("sources");
        let targets = directory.path().join("targets");
        std::fs::create_dir(&sources).unwrap();
        std::fs::create_dir(&targets).unwrap();
        let requests = (0..count)
            .map(|index| {
                let name = format!("file-{index}");
                std::fs::write(sources.join(&name), name.as_bytes()).unwrap();
                basic_transfer_request(
                    sources.join(&name),
                    targets.join(&name),
                    operation,
                    TransferConflictStrategy::Fail,
                )
            })
            .collect();
        Self {
            _directory: directory,
            sources,
            targets,
            requests,
            journal: MultiRecordMemoryJournal::new(31, count),
        }
    }

    /// What a restart would load: every record at its last committed state.
    fn reload(&self) -> Vec<TransferJournalRecord> {
        self.requests
            .iter()
            .enumerate()
            .map(|(index, request)| self.journal.record(index as u64, request.clone()))
            .collect()
    }

    async fn run(
        &self,
    ) -> (
        Result<(), RecoverableTransferError>,
        Vec<TransferSegmentEvent>,
    ) {
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let result = run_recoverable_segment(
            self.reload(),
            &self.journal,
            &running_transfer_options(),
            &events,
        )
        .await;
        drop(events);
        let mut collected = Vec::new();
        while let Some(event) = received.recv().await {
            collected.push(event);
        }
        (result, collected)
    }

    fn assert_finished(&self, operation: RecoverableTransferOperation) {
        let mut target_names = std::fs::read_dir(&self.targets)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        target_names.sort();
        let mut expected = (0..self.requests.len())
            .map(|index| format!("file-{index}"))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(target_names, expected, "no duplicates, no leftovers");
        for name in &expected {
            assert_eq!(
                std::fs::read(self.targets.join(name)).unwrap(),
                name.as_bytes()
            );
            assert_eq!(
                self.sources.join(name).exists(),
                operation == RecoverableTransferOperation::Copy
            );
        }
        assert_no_transfer_artifacts(&self.targets);
        assert_no_transfer_artifacts(&self.sources);
        for record in self.reload() {
            assert!(matches!(
                record.checkpoint,
                TransferCheckpoint::Completed(_)
            ));
        }
    }
}

fn finished_results(
    events: Vec<TransferSegmentEvent>,
) -> BTreeMap<usize, Result<RecoverableTransferOutcome, RecoverableTransferError>> {
    events
        .into_iter()
        .filter_map(|event| match event {
            TransferSegmentEvent::RecordFinished { index, result } => Some((index, result)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn copy_segment_commits_once_per_round_whatever_its_size() {
    for count in [1, 12] {
        let fixture = SegmentFixture::new(count, RecoverableTransferOperation::Copy);
        let (result, events) = fixture.run().await;
        result.unwrap();
        let results = finished_results(events);
        assert_eq!(results.len(), count);
        assert!(results.values().all(Result::is_ok));
        fixture.assert_finished(RecoverableTransferOperation::Copy);
        // AwaitingManifest → StageCreationIntent → Staging → CommitIntent →
        // TargetCommitted → Completed.
        assert_eq!(fixture.journal.commits.load(Ordering::SeqCst), 5);
    }
}

#[tokio::test]
async fn move_segment_reports_renames_before_they_are_durable() {
    let fixture = SegmentFixture::new(6, RecoverableTransferOperation::Move);
    let (result, events) = fixture.run().await;
    result.unwrap();
    let renamed_at = events
        .iter()
        .position(
            |event| matches!(event, TransferSegmentEvent::MovesRenamed(moves) if moves.len() == 6),
        )
        .expect("every rename of the round is reported together");
    let committed_at = events
        .iter()
        .position(|event| matches!(event, TransferSegmentEvent::DirectMovesCommitted(commits) if commits.len() == 6))
        .expect("every durable rename is reported together");
    assert!(renamed_at < committed_at);
    fixture.assert_finished(RecoverableTransferOperation::Move);
    // → DirectMoveIntent → DirectMoveRenamed → TargetCommitted → Completed.
    assert_eq!(fixture.journal.commits.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn segment_recovers_from_a_crash_before_any_round_commit() {
    for (operation, rounds) in [
        (RecoverableTransferOperation::Copy, 5),
        (RecoverableTransferOperation::Move, 4),
    ] {
        for crash_at in 1..=rounds {
            let fixture = SegmentFixture::new(5, operation);
            *fixture.journal.crash_at_commit.lock().unwrap() = Some(crash_at);
            let (result, _) = fixture.run().await;
            assert!(
                matches!(result, Err(RecoverableTransferError::Journal { .. })),
                "{operation:?} crash before commit {crash_at}"
            );

            *fixture.journal.crash_at_commit.lock().unwrap() = None;
            let (result, events) = fixture.run().await;
            result.unwrap();
            assert!(finished_results(events).values().all(Result::is_ok));
            fixture.assert_finished(operation);
        }
    }
}

#[tokio::test]
async fn one_failing_record_does_not_stop_its_siblings() {
    let fixture = SegmentFixture::new(4, RecoverableTransferOperation::Copy);
    std::fs::write(fixture.targets.join("file-2"), b"already here").unwrap();

    let (result, events) = fixture.run().await;
    result.unwrap();
    let results = finished_results(events);
    assert!(matches!(
        results[&2],
        Err(RecoverableTransferError::TargetConflict { .. })
    ));
    for index in [0, 1, 3] {
        assert!(results[&index].is_ok());
        let name = format!("file-{index}");
        assert_eq!(
            std::fs::read(fixture.targets.join(&name)).unwrap(),
            name.as_bytes()
        );
    }
    assert_eq!(
        std::fs::read(fixture.targets.join("file-2")).unwrap(),
        b"already here"
    );
    assert!(matches!(
        fixture.reload()[2].checkpoint,
        TransferCheckpoint::Failed { .. }
    ));
    assert_no_transfer_artifacts(&fixture.targets);
}
