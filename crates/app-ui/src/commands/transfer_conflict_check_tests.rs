use super::*;

#[tokio::test]
async fn only_transfers_without_a_decided_strategy_are_checked_for_conflicts() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("report.pdf");
    let taken = workspace.path().join("report副本.pdf");
    std::fs::write(&source, b"report").unwrap();
    std::fs::write(&taken, b"older copy").unwrap();
    let undecided = QueuedTransfer::new(source.clone(), taken.clone());
    // 原位副本带着 KeepBoth 入场：重名时怎么办已经定了，不该再问。
    let keep_both = QueuedTransfer {
        conflict_strategy: TransferConflictStrategy::KeepBoth,
        ..QueuedTransfer::new(source, taken)
    };

    assert_eq!(check_transfer_conflicts(&[keep_both]).await.len(), 0);
    assert_eq!(check_transfer_conflicts(&[undecided]).await.len(), 1);
}
