use std::collections::BTreeSet;

use super::*;
use crate::ops::{MetadataKind, MetadataLossSummary};

/// 真实文件系统上的复制一般不会丢元数据,这里直接构造"暂存时发现的问题",
/// 验证它沿检查点传递、经序列化与重启后仍然存在。
fn loss(paths: &[&str]) -> MetadataLossSummary {
    let mut summary = MetadataLossSummary::default();
    for path in paths {
        summary.record(
            Path::new(path),
            BTreeSet::from([MetadataKind::Permissions, MetadataKind::ExtendedAttributes]),
        );
    }
    summary
}

fn checkpoint_loss(checkpoint: &mut TransferCheckpoint) -> &mut MetadataLossSummary {
    match checkpoint {
        TransferCheckpoint::BackupCreationIntent(backup) => &mut backup.metadata_loss,
        TransferCheckpoint::CommitIntent(commit) => &mut commit.metadata_loss,
        TransferCheckpoint::TargetCommitted(committed) => &mut committed.metadata_loss,
        TransferCheckpoint::Completed(completed) => &mut completed.metadata_loss,
        other => panic!("checkpoint carries no metadata loss: {other:?}"),
    }
}

fn checkpoint_kind(checkpoint: &TransferCheckpoint) -> &'static str {
    match checkpoint {
        TransferCheckpoint::BackupCreationIntent(_) => "backup_creation_intent",
        TransferCheckpoint::CommitIntent(_) => "commit_intent",
        TransferCheckpoint::TargetCommitted(_) => "target_committed",
        TransferCheckpoint::Completed(_) => "completed",
        _ => "other",
    }
}

async fn replace_transfer(root: &Path) -> (MemoryJournal, TransferJournalRecord) {
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    fs::write(&source, b"new").await.unwrap();
    fs::write(&target, b"old").await.unwrap();
    let request = transfer_request(
        source,
        target,
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Replace,
    );
    let journal = MemoryJournal::new(2_001, TransferWorkKey::top_level(0), None);
    let record = journal.record(request);
    (journal, record)
}

async fn advance_until(
    record: &mut TransferJournalRecord,
    journal: &MemoryJournal,
    kind: &str,
) -> TransferCheckpoint {
    for _ in 0..32 {
        if checkpoint_kind(&record.checkpoint) == kind {
            return record.checkpoint.clone();
        }
        assert_eq!(
            advance_recoverable_transfer(record, journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
    }
    panic!("transfer never reached {kind}");
}

async fn run_to_completion(
    record: &mut TransferJournalRecord,
    journal: &MemoryJournal,
) -> RecoverableTransferOutcome {
    for _ in 0..64 {
        if let TransferAdvance::Complete(outcome) =
            advance_recoverable_transfer(record, journal, &running_transfer_options())
                .await
                .unwrap()
        {
            return outcome;
        }
    }
    panic!("transfer never completed");
}

#[tokio::test]
async fn checkpoints_roundtrip_metadata_loss_through_json() {
    for kind in [
        "backup_creation_intent",
        "commit_intent",
        "target_committed",
        "completed",
    ] {
        let directory = tempdir().unwrap();
        let (journal, mut record) = replace_transfer(directory.path()).await;
        let mut checkpoint = advance_until(&mut record, &journal, kind).await;
        *checkpoint_loss(&mut checkpoint) = loss(&["/dst/a.txt", "/dst/b.txt"]);

        let encoded = serde_json::to_vec(&checkpoint).unwrap();
        let decoded: TransferCheckpoint = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, checkpoint, "{kind}");
    }
}

#[tokio::test]
async fn checkpoints_written_before_metadata_loss_existed_decode_as_no_loss() {
    for kind in [
        "backup_creation_intent",
        "commit_intent",
        "target_committed",
        "completed",
    ] {
        let directory = tempdir().unwrap();
        let (journal, mut record) = replace_transfer(directory.path()).await;
        let checkpoint = advance_until(&mut record, &journal, kind).await;

        // 空摘要不序列化,编码结果与旧版本写入的检查点形态一致。
        let encoded = serde_json::to_string(&checkpoint).unwrap();
        assert!(!encoded.contains("metadata_loss"), "{kind}: {encoded}");
        let mut decoded: TransferCheckpoint = serde_json::from_str(&encoded).unwrap();

        assert!(checkpoint_loss(&mut decoded).is_empty(), "{kind}");
    }
}

#[tokio::test]
async fn resumed_transfer_still_reports_metadata_loss_recorded_before_restart() {
    for kind in ["commit_intent", "target_committed"] {
        let directory = tempdir().unwrap();
        let (journal, mut record) = replace_transfer(directory.path()).await;
        let mut checkpoint = advance_until(&mut record, &journal, kind).await;
        let recorded = loss(&["/dst/target.txt"]);
        *checkpoint_loss(&mut checkpoint) = recorded.clone();
        journal
            .commit(TransferJournalMutation::CompareAndSwapCheckpoint {
                task_id: record.task_id,
                key: record.key.clone(),
                expected_revision: record.revision,
                checkpoint,
            })
            .await
            .unwrap();

        // 模拟重启:丢掉内存里的记录,从日志重新读出。
        let mut resumed = journal.record(record.request.clone());
        let outcome = run_to_completion(&mut resumed, &journal).await;

        assert_eq!(outcome.metadata_loss, recorded, "{kind}");
    }
}

#[tokio::test]
async fn merge_root_completion_absorbs_every_child_metadata_loss() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::create_dir(&source).await.unwrap();
    fs::create_dir(&target).await.unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(source.join(name), name.as_bytes()).await.unwrap();
    }
    let request = transfer_request(
        source,
        target,
        RecoverableTransferOperation::Copy,
        TransferConflictStrategy::Merge,
    );
    let journal = MemoryJournal::new(2_002, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(request);

    // 推进到所有子项都已完成、根尚未完成的那一刻。
    for _ in 0..64 {
        if let TransferCheckpoint::Merging(merge) = &record.checkpoint {
            if !merge.child_names.is_empty() && merge.next_child == merge.child_names.len() {
                break;
            }
        }
        assert_eq!(
            advance_recoverable_transfer(&mut record, &journal, &running_transfer_options())
                .await
                .unwrap(),
            TransferAdvance::Continue
        );
    }
    let TransferCheckpoint::Merging(merge) = &mut record.checkpoint else {
        panic!("merge checkpoint expected");
    };
    let child_losses = [
        loss(&["/dst/a.txt"]),
        loss(&["/dst/b.txt", "/dst/b2.txt", "/dst/b3.txt"]),
        loss(&["/dst/c.txt", "/dst/c2.txt", "/dst/c3.txt"]),
    ];
    assert_eq!(merge.completed_children.len(), child_losses.len());
    for (completion, child_loss) in merge.completed_children.iter_mut().zip(&child_losses) {
        let MergeChildOutcome::Committed(completed) = &mut completion.outcome else {
            panic!("committed child expected");
        };
        completed.metadata_loss = child_loss.clone();
    }

    let outcome = run_to_completion(&mut record, &journal).await;

    assert_eq!(outcome.metadata_loss.affected_entries, 7);
    let sample_paths = outcome
        .metadata_loss
        .samples
        .iter()
        .map(|sample| sample.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        sample_paths,
        vec![
            PathBuf::from("/dst/a.txt"),
            PathBuf::from("/dst/b.txt"),
            PathBuf::from("/dst/b2.txt"),
            PathBuf::from("/dst/b3.txt"),
            PathBuf::from("/dst/c.txt"),
        ]
    );
}
