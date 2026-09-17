use super::*;
use crate::operation_queue::FileOperationStatus;

/// 监督认领的不变量:其他实例死亡后遗留的非终态任务(runner 租约空闲),
/// 由幸存实例的监督 tick 认领,并从 journal 断点(直移 rename 已发生、
/// TargetCommitted 未落盘)继续推到终态,而不是永远停在"处理中"。
#[tokio::test]
async fn supervision_adopts_a_foreign_nonterminal_task_with_a_free_lease() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"adopt-me").await.unwrap();

    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut owner = FileOperationQueue::new();
    owner.set_store(store.clone());
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    let FileOperationEnqueueOutcome::Queued { task_id } = owner.enqueue(
        QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        },
    ) else {
        panic!("recoverable move should enqueue");
    };
    let stored_task_id = owner.tasks()[0].stored_id.unwrap();

    // 模拟死亡驱动者留下的中间态:rename 已落盘,journal 停在 durable renamed。
    let running = owner.active_subscription().unwrap();
    let record = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap()
        .remove(0);
    let journal = task_queue_transfer_journal(store.clone(), running.controls.clone());
    let DirectMoveIntentBoundary::Intent(record) = run_recoverable_transfer_to_direct_move_intent(
        record,
        &journal,
        FileTransferOptions::new(running.controls.clone()),
    )
    .await
    .unwrap()
    else {
        panic!("fresh same-filesystem move should prepare a direct move intent");
    };
    let batch = run_direct_move_batch_to_durable_renamed(
        vec![record],
        &journal,
        &FileTransferOptions::new(running.controls.clone()),
        &mut |_| {},
    )
    .await
    .unwrap();
    assert!(matches!(batch.as_slice(), [DirectMoveBatchRecord::Renamed(_)]));
    assert!(tokio::fs::symlink_metadata(&source).await.is_err());
    drop(running);
    drop(journal);
    drop(owner);

    // 幸存实例:内存为空,监督 tick 扫 store 认领并推进到 Running。
    let mut survivor = FileOperationQueue::new();
    survivor.set_store(store.clone());
    assert!(survivor.supervise(std::time::Instant::now()).is_none());
    assert_eq!(survivor.tasks().len(), 1);
    assert_eq!(survivor.tasks()[0].status, FileOperationStatus::Running);
    assert_eq!(survivor.tasks()[0].stored_id, Some(stored_task_id));
    assert_eq!(survivor.tasks()[0].driver_generation, 0);

    let launch = survivor.poll_driver_launch().unwrap();
    assert_eq!(launch.id, stored_task_id);
    assert_eq!(launch.generation, 0);
    let (mut output, _messages) = iced::futures::channel::mpsc::channel(32);
    let completion = run_queued_transfers(
        transfers,
        launch.controls,
        launch.stored_id.unwrap(),
        stored_task_id,
        &mut output,
        launch.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;
    drop(output);
    assert!(matches!(completion, FileOperationCompletion::Succeeded(_)));
    assert_eq!(tokio::fs::read(&target).await.unwrap(), b"adopt-me");
    // 终态由 UI 的 Finished 处理落账(queue.finish);认领任务以存储 id 结账。
    assert_eq!(
        survivor.finish(
            stored_task_id,
            FileOperationFinish::Succeeded,
        ),
        (Some(crate::operation_queue::FileOperationTerminalStatus::Completed), None)
    );
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
    // 认领的任务以存储 id 作为内存 id;所有者的本地 task_id 与幸存者无关。
    let _ = task_id;
}
