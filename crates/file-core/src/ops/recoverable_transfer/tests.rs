use std::path::{Path, PathBuf};

use tempfile::tempdir;
use tokio::fs;

use super::*;

fn owner() -> ArtifactOwner {
    ArtifactOwner {
        task_id: 10,
        transfer_index: 2,
        work_index: 3,
    }
}

#[tokio::test]
async fn identity_does_not_follow_symbolic_links() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target");
        let link = directory.path().join("link");
        fs::write(&target, b"content").await.unwrap();
        symlink("target", &link).unwrap();

        let target_identity = inspect_file_identity(&target).await.unwrap();
        let link_identity = inspect_file_identity(&link).await.unwrap();

        assert_eq!(target_identity.object_kind, FileObjectKind::RegularFile);
        assert_eq!(link_identity.object_kind, FileObjectKind::SymbolicLink);
        assert_eq!(
            link_identity.symbolic_link_target,
            Some(PathBuf::from("target"))
        );
        assert!(!link_identity.same_object(&target_identity));
    }
}

#[tokio::test]
async fn source_manifest_covers_nested_tree_without_following_links() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let root = directory.path().join("source");
        fs::create_dir(&root).await.unwrap();
        fs::create_dir(root.join("nested")).await.unwrap();
        fs::write(root.join("nested/file"), b"content")
            .await
            .unwrap();
        symlink("nested", root.join("link")).unwrap();

        let manifest = build_source_manifest(&root).await.unwrap();
        let paths = manifest
            .entries
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                PathBuf::new(),
                PathBuf::from("link"),
                PathBuf::from("nested"),
                PathBuf::from("nested/file"),
            ]
        );
        assert_eq!(
            manifest.entries[1].identity.object_kind,
            FileObjectKind::SymbolicLink
        );
        verify_source_manifest(&manifest).await.unwrap();
    }
}

#[tokio::test]
async fn source_manifest_detects_content_version_change() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    fs::write(&source, b"before").await.unwrap();
    let manifest = build_source_manifest(&source).await.unwrap();

    fs::write(&source, b"after-longer").await.unwrap();

    assert!(matches!(
        verify_source_manifest(&manifest).await,
        Err(RecoverableTransferError::SourceChanged { path }) if path == source
    ));
}

#[test]
fn no_replace_rename_never_overwrites_existing_target() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"source").unwrap();
    std::fs::write(&target, b"target").unwrap();

    assert!(matches!(
        rename_noreplace(&source, &target),
        Err(NoReplaceRenameError::TargetExists)
    ));
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    assert_eq!(std::fs::read(&target).unwrap(), b"target");
}

#[test]
fn no_replace_rename_moves_to_absent_target() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"source").unwrap();

    rename_noreplace(&source, &target).unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"source");
}

#[tokio::test]
async fn commit_barrier_flushes_registered_and_removed_locations() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("tree");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/file"), b"content").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("nested/file", root.join("link")).unwrap();

    let mut barrier = CommitBarrier::default();
    barrier.require_parent_of(&root.join("nested/file"));
    barrier.require_rename(&root.join("link"), &root.join("gone/child"));
    barrier.flush().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn identity_and_manifest_preserve_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempdir().unwrap();
    let root = directory.path().join("source");
    fs::create_dir(&root).await.unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'f', 0xff]);
    let child = root.join(&name);
    fs::write(&child, b"content").await.unwrap();

    let manifest = build_source_manifest(&root).await.unwrap();

    assert!(manifest
        .entries
        .iter()
        .any(|entry| entry.relative_path.as_os_str() == name));
    assert_eq!(
        inspect_file_identity(&child).await.unwrap().object_kind,
        FileObjectKind::RegularFile
    );
}

#[tokio::test]
async fn merge_checkpoint_does_not_serialize_sibling_cache() {
    let directory = tempdir().unwrap();
    let target_identity = inspect_file_identity(directory.path()).await.unwrap();
    let checkpoint = TransferCheckpoint::Merging(MergeTransfer {
        target_root_identity: target_identity,
        next_child: 41,
        active_child: None,
        child_names: (0..10_000)
            .map(|index| PathBuf::from(format!("child-{index}")))
            .collect(),
        completed_children: Vec::new(),
        completed_prefix_verified: true,
    });

    let encoded = serde_json::to_vec(&checkpoint).unwrap();
    assert!(encoded.len() < 512);
    let TransferCheckpoint::Merging(decoded) = serde_json::from_slice(&encoded).unwrap() else {
        panic!("merge checkpoint expected");
    };
    assert_eq!(decoded.next_child, 41);
    assert!(decoded.child_names.is_empty());
    assert!(decoded.completed_children.is_empty());
    assert!(!decoded.completed_prefix_verified);
}

#[tokio::test]
async fn transfer_checkpoint_does_not_serialize_manifest_cache() {
    let directory = tempdir().unwrap();
    let identity = inspect_file_identity(directory.path()).await.unwrap();
    let record = TransferJournalRecord {
        task_id: 1,
        key: TransferWorkKey::top_level(0),
        request: transfer_request(
            directory.path().join("source"),
            directory.path().join("target"),
            RecoverableTransferOperation::Copy,
            TransferConflictStrategy::Fail,
        ),
        checkpoint: TransferCheckpoint::AwaitingManifest,
        revision: 0,
        manifest: Some(SourceManifest {
            root: directory.path().join("source"),
            entries: (0..10_000)
                .map(|index| SourceManifestEntry {
                    relative_path: PathBuf::from(format!("child-{index}")),
                    identity: identity.clone(),
                })
                .collect(),
        }),
        replacement_manifest: None,
    };

    let encoded = serde_json::to_vec(&record).unwrap();
    assert!(encoded.len() < 1_024);
    let decoded: TransferJournalRecord = serde_json::from_slice(&encoded).unwrap();
    assert!(decoded.manifest.is_none());
}

#[test]
fn artifact_token_roundtrips_bytes() {
    let bytes = [0x7a; 16];
    assert_eq!(ArtifactToken::from_bytes(bytes).into_bytes(), bytes);
}

#[test]
fn recovered_name_keeps_original_name_bytes() {
    let path = Path::new("/tmp/report.txt");
    assert_eq!(
        rename::recovered_name_candidate(path, 3),
        PathBuf::from("/tmp/report.txt.recovered3")
    );
}

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::{
    FileOperationControls, FileOperationVerification, FileTransferOptions, TransferConflictStrategy,
};
use tokio_util::sync::CancellationToken;

use super::executor::{
    advance_recoverable_transfer, persist_source_manifests, run_recoverable_segment,
    run_recoverable_transfer, TransferAdvance, TransferSegmentEvent,
};

struct MemoryJournalState {
    revision: u64,
    checkpoint: TransferCheckpoint,
    manifest: Option<SourceManifest>,
    replacement_manifest: Option<SourceManifest>,
}

struct MemoryJournal {
    task_id: u64,
    key: TransferWorkKey,
    state: Mutex<MemoryJournalState>,
    attempts: AtomicUsize,
    fail_on_attempt: Mutex<Option<usize>>,
}

impl MemoryJournal {
    fn new(task_id: u64, key: TransferWorkKey, fail_on_attempt: Option<usize>) -> Self {
        Self {
            task_id,
            key,
            state: Mutex::new(MemoryJournalState {
                revision: 0,
                checkpoint: TransferCheckpoint::AwaitingManifest,
                manifest: None,
                replacement_manifest: None,
            }),
            attempts: AtomicUsize::new(0),
            fail_on_attempt: Mutex::new(fail_on_attempt),
        }
    }

    fn record(&self, request: RecoverableTransferRequest) -> TransferJournalRecord {
        let state = self.state.lock().unwrap();
        TransferJournalRecord {
            task_id: self.task_id,
            key: self.key.clone(),
            request,
            checkpoint: state.checkpoint.clone(),
            revision: state.revision,
            manifest: state.manifest.clone(),
            replacement_manifest: state.replacement_manifest.clone(),
        }
    }

    fn set_failure(&self, fail_on_attempt: Option<usize>) {
        *self.fail_on_attempt.lock().unwrap() = fail_on_attempt;
    }

    fn attempt_count(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl TransferJournal for MemoryJournal {
    fn commit(&self, mutations: Vec<TransferJournalMutation>) -> TransferJournalFuture<'_> {
        Box::pin(async move {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if *self.fail_on_attempt.lock().unwrap() == Some(attempt) {
                return Err(TransferJournalError::Storage(format!(
                    "injected journal failure {attempt}"
                )));
            }

            let mut state = self.state.lock().unwrap();
            let mut revisions = Vec::with_capacity(mutations.len());
            for mutation in mutations {
                if mutation.task_id != self.task_id
                    || mutation.key != self.key
                    || mutation.expected_revision != state.revision
                {
                    return Err(TransferJournalError::StaleRevision);
                }
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

/// 准备阶段装清单的单条记录版本：建不出清单时把那条错误交回来。
async fn persist_recoverable_source_manifest(
    record: &mut TransferJournalRecord,
    journal: &impl TransferJournal,
) -> Result<(), RecoverableTransferError> {
    let controls = FileOperationControls::running(CancellationToken::new());
    let scanned = AtomicUsize::new(0);
    let mut build_errors =
        persist_source_manifests(std::slice::from_mut(record), journal, &controls, &scanned)
            .await?;
    match build_errors.pop().flatten() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn transfer_request(
    source: PathBuf,
    target: PathBuf,
    operation: RecoverableTransferOperation,
    conflict_strategy: TransferConflictStrategy,
) -> RecoverableTransferRequest {
    RecoverableTransferRequest {
        source,
        requested_target: target,
        operation,
        conflict_strategy,
        verification: FileOperationVerification::Strong,
    }
}

fn basic_transfer_request(
    source: PathBuf,
    target: PathBuf,
    operation: RecoverableTransferOperation,
    conflict_strategy: TransferConflictStrategy,
) -> RecoverableTransferRequest {
    RecoverableTransferRequest {
        source,
        requested_target: target,
        operation,
        conflict_strategy,
        verification: FileOperationVerification::BasicMetadata,
    }
}

fn running_transfer_options() -> FileTransferOptions {
    FileTransferOptions::running(CancellationToken::new())
}

fn canceled_transfer_options() -> FileTransferOptions {
    let cancel = CancellationToken::new();
    cancel.cancel();
    FileTransferOptions::running(cancel)
}

#[tokio::test]
async fn persisted_source_manifest_keeps_awaiting_checkpoint_and_is_reused() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"before").await.unwrap();
    let key = TransferWorkKey::top_level(0);
    let journal = MemoryJournal::new(2_001, key, None);
    let mut record = journal.record(transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    ));

    persist_recoverable_source_manifest(&mut record, &journal)
        .await
        .unwrap();

    assert_eq!(record.revision, 1);
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::AwaitingManifest
    ));
    assert!(record.manifest.is_some());
    fs::write(&source, b"after-longer").await.unwrap();

    let error = run_recoverable_transfer(record, &journal, running_transfer_options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RecoverableTransferError::SourceChanged { path } if path == source
    ));
    assert!(fs::symlink_metadata(&target).await.is_err());
}

#[tokio::test]
async fn source_manifest_journal_failure_leaves_record_and_filesystem_unchanged() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"content").await.unwrap();
    let key = TransferWorkKey::top_level(0);
    let journal = MemoryJournal::new(2_002, key, Some(1));
    let mut record = journal.record(transfer_request(
        source.clone(),
        target.clone(),
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Fail,
    ));

    let error = persist_recoverable_source_manifest(&mut record, &journal)
        .await
        .unwrap_err();

    assert!(matches!(error, RecoverableTransferError::Journal { .. }));
    assert_eq!(record.revision, 0);
    assert!(record.manifest.is_none());
    assert!(matches!(
        record.checkpoint,
        TransferCheckpoint::AwaitingManifest
    ));
    assert_eq!(fs::read(&source).await.unwrap(), b"content");
    assert!(fs::symlink_metadata(&target).await.is_err());
    assert_no_transfer_artifacts(directory.path());
}

fn assert_no_transfer_artifacts(parent: &Path) {
    for entry in std::fs::read_dir(parent).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(".file-manager-transfer-"));
        assert!(!name.starts_with(".file-manager-source-retirement-"));
    }
}

#[tokio::test]
async fn prepare_records_content_fingerprints_only_for_strong_verification() {
    let directory = tempdir().unwrap();

    for (task_id, verification) in [
        (2_003, FileOperationVerification::BasicMetadata),
        (2_004, FileOperationVerification::Strong),
    ] {
        let source = directory.path().join(format!("source-{task_id}"));
        let target = directory.path().join(format!("target-{task_id}"));
        fs::write(&source, b"content").await.unwrap();
        let mut request = basic_transfer_request(
            source.clone(),
            target,
            RecoverableTransferOperation::Move,
            TransferConflictStrategy::Fail,
        );
        request.verification = verification;
        let journal = MemoryJournal::new(task_id, TransferWorkKey::top_level(0), None);
        let mut record = journal.record(request);

        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        match (verification, &record.checkpoint) {
            (
                FileOperationVerification::BasicMetadata,
                TransferCheckpoint::DirectMoveIntent(prepared),
            ) => assert!(prepared.source_fingerprint.is_none()),
            (FileOperationVerification::Strong, TransferCheckpoint::CommitIntent(commit)) => {
                assert!(commit.prepared.source_fingerprint.is_some())
            }
            _ => panic!("verification mode selected the wrong pre-side-effect checkpoint"),
        }
        assert!(source.exists());
    }
}

#[tokio::test]
async fn basic_copy_and_same_filesystem_move_build_complete_post_side_effect_proofs() {
    let directory = tempdir().unwrap();

    for (task_id, operation) in [
        (2_005, RecoverableTransferOperation::Copy),
        (2_006, RecoverableTransferOperation::Move),
    ] {
        let source = directory.path().join(format!("source-{task_id}"));
        let target = directory.path().join(format!("target-{task_id}"));
        fs::write(&source, b"content").await.unwrap();
        let request = basic_transfer_request(
            source.clone(),
            target.clone(),
            operation,
            TransferConflictStrategy::Fail,
        );
        let journal = MemoryJournal::new(task_id, TransferWorkKey::top_level(0), None);
        let mut record = journal.record(request);
        let options = running_transfer_options();

        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &options)
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
        match operation {
            RecoverableTransferOperation::Copy => {
                let TransferCheckpoint::StageCreationIntent(prepared) = &record.checkpoint else {
                    panic!("Basic copy must persist staging before its first payload write");
                };
                assert!(prepared.source_fingerprint.is_none());
                assert!(source.exists());

                assert_eq!(
                    advance_recoverable_transfer(&mut record, &journal, &options)
                        .await
                        .unwrap(),
                    TransferAdvance::Continue
                );
                assert!(matches!(record.checkpoint, TransferCheckpoint::Staging(_)));
                assert!(source.exists());

                assert_eq!(
                    advance_recoverable_transfer(&mut record, &journal, &options)
                        .await
                        .unwrap(),
                    TransferAdvance::Continue
                );
                let TransferCheckpoint::CommitIntent(commit) = &record.checkpoint else {
                    panic!("staged Basic copy must persist a complete commit proof");
                };
                let CommitPayload::Artifact { artifact, .. } = &commit.payload else {
                    panic!("Basic copy must commit from owned staging");
                };
                assert_eq!(
                    fingerprint_object(&artifact.plan.payload_path())
                        .await
                        .unwrap(),
                    commit
                        .fingerprint
                        .as_blake3()
                        .expect("basic copy keeps a blake3 proof")
                );
                assert!(source.exists());
                assert!(!target.exists());
            }
            RecoverableTransferOperation::Move => {
                let TransferCheckpoint::DirectMoveIntent(prepared) = &record.checkpoint else {
                    panic!("Basic same-filesystem move must persist a direct move intent");
                };
                assert!(prepared.source_fingerprint.is_none());
                assert!(source.exists());
                assert!(!target.exists());

                assert_eq!(
                    advance_recoverable_transfer(&mut record, &journal, &options)
                        .await
                        .unwrap(),
                    TransferAdvance::Continue
                );
                assert!(matches!(
                    record.checkpoint,
                    TransferCheckpoint::DirectMoveRenamed(_)
                ));
                assert!(!source.exists());
                assert_eq!(fs::read(&target).await.unwrap(), b"content");

                assert_eq!(
                    advance_recoverable_transfer(&mut record, &journal, &options)
                        .await
                        .unwrap(),
                    TransferAdvance::Continue
                );
                let TransferCheckpoint::TargetCommitted(committed) = &record.checkpoint else {
                    panic!("visible Basic move must persist a complete target proof");
                };
                assert_eq!(
                    fingerprint_object(&target).await.unwrap(),
                    committed
                        .fingerprint
                        .as_blake3()
                        .expect("direct move keeps a blake3 proof")
                );
            }
        }
    }
}

mod artifact_cases;
mod conflict_cases;
mod direct_move_cases;
mod invalid_checkpoint_cases;
mod kernel_clone_cases;
mod merge_and_control_cases;
mod metadata_loss_cases;
mod payload_integrity_cases;
mod recovery_cases;
mod replacement_backup_cases;
mod segment_cases;
mod source_retirement_cases;
