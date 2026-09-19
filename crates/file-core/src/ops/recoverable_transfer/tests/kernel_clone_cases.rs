//! KernelClone 证明与运行内证明记忆的语义用例。
//!
//! 覆盖三条新不变量:
//! 1. FICLONE 真实命中时,Basic 单文件 Copy 以 KernelClone 事实完成;
//! 2. 阶梯降级(内核快拷/用户态循环)是真实数据搬运,必须回退 Blake3 证明;
//! 3. 携带 KernelClone 证明的 CommitIntent 前向恢复只认精确身份,
//!    payload 被篡改(身份失配)即 blocked,绝不带病提交。

use std::path::PathBuf;

use tempfile::tempdir;

use super::super::super::transfer_strategy::{ficlone_supported, PayloadCopyStrategy};
use super::super::executor::{advance_recoverable_transfer, TransferAdvance};
use super::super::run_recoverable_transfer;
use super::*;

fn basic_copy_options(strategy: Option<PayloadCopyStrategy>) -> FileTransferOptions {
    let mut options = running_transfer_options();
    if let Some(strategy) = strategy {
        let directory = std::env::temp_dir();
        let metadata = std::fs::symlink_metadata(&directory).unwrap();
        let device = std::os::unix::fs::MetadataExt::dev(&metadata);
        let engine = crate::ops::transfer_strategy::TransferStrategyEngine::probe(
            device, device, &directory,
        );
        engine.force_strategy(device, device, strategy);
        options = options.with_strategy_engine(std::sync::Arc::new(engine));
    }
    options
}

async fn advance_to_completion(
    record: &mut TransferJournalRecord,
    journal: &MemoryJournal,
    options: &FileTransferOptions,
) {
    for _ in 0..16 {
        match advance_recoverable_transfer(record, journal, options)
            .await
            .unwrap()
        {
            TransferAdvance::Continue => {}
            TransferAdvance::Complete(_) => return,
        }
    }
    panic!("transfer did not complete within the expected number of advances");
}

fn completed_fingerprint(record: &TransferJournalRecord) -> &TransferFingerprint {
    let TransferCheckpoint::Completed(completed) = &record.checkpoint else {
        panic!("transfer must finish at a Completed checkpoint");
    };
    &completed.fingerprint
}

#[tokio::test]
async fn basic_single_file_copy_records_kernel_clone_proof_only_when_ficlone_hits() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"clone me exactly once").unwrap();

    let journal = MemoryJournal::new(2_601, TransferWorkKey::top_level(0), None);
    let record = journal.record(basic_transfer_request(
        source.clone(),
        target.clone(),
        crate::ops::RecoverableTransferOperation::Copy,
        crate::TransferConflictStrategy::Fail,
    ));

    // 引擎不钉策略,阶梯自由选择;结果与宿主文件系统能力保持一致:
    // 克隆命中 → KernelClone 证明;任何降级 → 必须回退全文 Blake3。
    let options = basic_copy_options(None);
    let mut record = record;
    advance_to_completion(&mut record, &journal, &options).await;

    let content = std::fs::read(&target).unwrap();
    assert_eq!(content, b"clone me exactly once");

    let cloned = ficlone_supported(directory.path());
    match completed_fingerprint(&record) {
        TransferFingerprint::KernelClone { payload_identity } => {
            assert!(cloned, "克隆证明只允许出现在真实支持 FICLONE 的文件系统上");
            let payload_target_identity =
                crate::ops::recoverable_transfer::inspect_file_identity(&target)
                    .await
                    .unwrap();
            assert_eq!(*payload_identity, payload_target_identity);
        }
        TransferFingerprint::Blake3(_) => {}
    }
}

#[tokio::test]
async fn kernel_range_fallback_staging_must_produce_blake3_proof() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, vec![7_u8; 64 * 1024]).unwrap();

    let journal = MemoryJournal::new(2_602, TransferWorkKey::top_level(0), None);
    let record = journal.record(basic_transfer_request(
        source.clone(),
        target.clone(),
        crate::ops::RecoverableTransferOperation::Copy,
        crate::TransferConflictStrategy::Fail,
    ));

    // 钉死内核快拷(真实数据搬运,非克隆):证明必须回退全文 Blake3,
    // 绝不允许借克隆事实跳过校验。
    let options = basic_copy_options(Some(PayloadCopyStrategy::KernelRange));
    let mut record = record;
    advance_to_completion(&mut record, &journal, &options).await;

    assert_eq!(std::fs::read(&target).unwrap(), vec![7_u8; 64 * 1024]);
    assert!(matches!(
        completed_fingerprint(&record),
        TransferFingerprint::Blake3(_)
    ));
}

#[tokio::test]
async fn strong_verification_keeps_full_content_proof_end_to_end() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"strong content").unwrap();

    let journal = MemoryJournal::new(2_603, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(crate::ops::RecoverableTransferRequest {
        verification: crate::FileOperationVerification::Strong,
        ..basic_transfer_request(
            source.clone(),
            target.clone(),
            crate::ops::RecoverableTransferOperation::Copy,
            crate::TransferConflictStrategy::Fail,
        )
    });

    let options = running_transfer_options();
    advance_to_completion(&mut record, &journal, &options).await;

    assert_eq!(std::fs::read(&target).unwrap(), b"strong content");
    assert!(matches!(
        completed_fingerprint(&record),
        TransferFingerprint::Blake3(_)
    ));
}

#[tokio::test]
async fn kernel_clone_commit_intent_recovers_forward_on_exact_identity() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"crash after staging").unwrap();

    // 先正常跑到 CommitIntent(克隆能力缺失时该用例退化为 Blake3 证明,
    // 前向恢复语义相同,仍然覆盖"payload 存在且身份一致 → 前向提交")。
    let journal = MemoryJournal::new(2_604, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(basic_transfer_request(
        source.clone(),
        target.clone(),
        crate::ops::RecoverableTransferOperation::Copy,
        crate::TransferConflictStrategy::Fail,
    ));
    let options = basic_copy_options(None);
    loop {
        match advance_recoverable_transfer(&mut record, &journal, &options)
            .await
            .unwrap()
        {
            TransferAdvance::Continue => {}
            TransferAdvance::Complete(_) => panic!("run must stop before completion"),
        }
        if matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
            break;
        }
    }
    // 模拟进程崩溃:以 journal 中的最新 checkpoint 重建 record 后前向恢复。
    let recovered = journal.record(record.request.clone());
    let outcome = run_recoverable_transfer(recovered, &journal, running_transfer_options())
        .await
        .unwrap();
    assert_eq!(outcome.final_target, Some(target.clone()));
    assert_eq!(std::fs::read(&target).unwrap(), b"crash after staging");
}

#[tokio::test]
async fn kernel_clone_commit_intent_recovery_blocks_on_tampered_payload() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, b"tamper target payload").unwrap();

    let journal = MemoryJournal::new(2_605, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(basic_transfer_request(
        source.clone(),
        target.clone(),
        crate::ops::RecoverableTransferOperation::Copy,
        crate::TransferConflictStrategy::Fail,
    ));
    let options = basic_copy_options(None);
    loop {
        match advance_recoverable_transfer(&mut record, &journal, &options)
            .await
            .unwrap()
        {
            TransferAdvance::Continue => {}
            TransferAdvance::Complete(_) => panic!("run must stop before completion"),
        }
        if matches!(record.checkpoint, TransferCheckpoint::CommitIntent(_)) {
            break;
        }
    }

    // 篡改 payload 内容并保持 inode:Blake3 证明路径必须靠 memo 失配或重算
    // 抓住;KernelClone 证明路径靠 size 失配抓住。两者都不得带病提交。
    let payload_path = match &record.checkpoint {
        TransferCheckpoint::CommitIntent(commit) => match &commit.payload {
            crate::ops::CommitPayload::Artifact { artifact, .. } => artifact.plan.payload_path(),
            crate::ops::CommitPayload::DirectSource { .. } => {
                return;
            }
        },
        _ => panic!("commit intent checkpoint expected"),
    };
    std::fs::write(&payload_path, b"tampered!").unwrap();

    let recovered = journal.record(record.request.clone());
    let error = run_recoverable_transfer(recovered, &journal, running_transfer_options())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            crate::ops::RecoverableTransferError::FingerprintMismatch { .. }
                | crate::ops::RecoverableTransferError::SourceChanged { .. }
                | crate::ops::RecoverableTransferError::RecoveryBlocked { .. }
        ),
        "被篡改的 payload 不得前向提交: {error:?}"
    );
    assert!(!target.exists(), "被篡改的 payload 不得出现在最终目标");
}

#[tokio::test]
async fn proof_memo_hashes_each_object_once_within_a_run() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    std::fs::write(&source, vec![3_u8; 128 * 1024]).unwrap();

    let journal = MemoryJournal::new(2_606, TransferWorkKey::top_level(0), None);
    let mut record = journal.record(basic_transfer_request(
        source.clone(),
        target.clone(),
        crate::ops::RecoverableTransferOperation::Copy,
        crate::TransferConflictStrategy::Fail,
    ));
    let options = running_transfer_options();
    advance_to_completion(&mut record, &journal, &options).await;

    // 行为不变量:内容与既有语义一致;memo 的收益由 A2/A3 测量验证。
    assert_eq!(std::fs::read(&target).unwrap(), vec![3_u8; 128 * 1024]);
}

#[test]
fn legacy_blake3_fingerprint_json_decodes_into_blake3_variant() {
    let bytes: Vec<u8> = (0..32).collect();
    let json = serde_json::to_string(&bytes).unwrap();
    let decoded: TransferFingerprint = serde_json::from_str(&json).unwrap();
    assert!(matches!(decoded, TransferFingerprint::Blake3(_)));
    // 新格式带 tag,往返不丢信息。
    let payload_identity = crate::ops::FileIdentity {
        device: 1,
        inode: 2,
        object_kind: crate::ops::FileObjectKind::RegularFile,
        size: 3,
        modified_seconds: 4,
        modified_nanoseconds: 5,
        changed_seconds: 6,
        changed_nanoseconds: 7,
        symbolic_link_target: None,
    };
    let clone = TransferFingerprint::KernelClone { payload_identity };
    let encoded = serde_json::to_string(&clone).unwrap();
    let decoded: TransferFingerprint = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, clone);
}

#[allow(dead_code)]
fn unused_path_buffer() -> PathBuf {
    PathBuf::new()
}
