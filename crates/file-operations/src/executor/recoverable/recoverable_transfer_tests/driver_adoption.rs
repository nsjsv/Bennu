use super::*;
use crate::FileOperationStatus;

/// 认领的不变量:其他实例死亡后遗留的非终态任务(runner 租约空闲),
/// 由幸存实例在任务库变化时重新扫描认领,并从 journal 断点(直移 rename 已发生、
/// TargetCommitted 未落盘)继续推到终态,而不是永远停在"处理中"。
#[tokio::test]
async fn supervision_adopts_a_foreign_nonterminal_task_with_a_free_lease() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    tokio::fs::write(&source, b"adopt-me").await.unwrap();

    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut owner = FileOperationQueue::new();
    attach_store(&mut owner, store.clone());
    // 两个实例同时在跑:幸存者启动时库里还没有任务,启动恢复不会提前接走它。
    let mut survivor = FileOperationQueue::new();
    attach_store(&mut survivor, store.clone());
    let transfers = vec![QueuedTransfer::new(source.clone(), target.clone())];
    // 认领的任务以存储 id 作为内存 id;所有者的本地 task_id 与幸存者无关。
    let FileOperationEnqueueOutcome::Queued { .. } = owner.enqueue(QueuedFileOperation::Move {
        transfers: transfers.clone(),
        verification: FileOperationVerification::BasicMetadata,
    }) else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut owner);
    let stored_task_id = owner.tasks()[0].stored_id.unwrap();

    // 模拟死亡驱动者留下的中间态:rename 已落盘,journal 停在 durable renamed。
    let running = owner.active_subscription().unwrap();
    run_until_checkpoint(
        &store,
        stored_task_id,
        &running.controls,
        is_direct_move_renamed,
    )
    .await;
    assert!(tokio::fs::symlink_metadata(&source).await.is_err());
    drop(running);
    drop(owner);

    // 幸存实例:内存为空,重新扫描任务库认领并推进到 Running。
    assert!(survivor.adopt_foreign_tasks().is_none());
    assert_persisted(&mut survivor);
    assert_eq!(survivor.tasks().len(), 1);
    assert_eq!(survivor.tasks()[0].status, FileOperationStatus::Running);
    assert_eq!(survivor.tasks()[0].stored_id, Some(stored_task_id));
    assert_eq!(survivor.tasks()[0].driver_generation, 0);

    let launch = survivor.poll_driver_launch().unwrap();
    assert_eq!(launch.id, stored_task_id);
    assert_eq!(launch.generation, 0);
    let (output, _messages) = tokio::sync::mpsc::channel(32);
    let completion = run_queued_transfers(
        transfers,
        launch.controls,
        launch.stored_id.unwrap(),
        stored_task_id,
        &output,
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
        survivor.finish(stored_task_id, FileOperationFinish::Succeeded,),
        Some(crate::FileOperationTerminalStatus::Completed)
    );
    assert_persisted(&mut survivor);
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());
}

/// 锁还被别的进程拿着时不能抢,只登记等待;持锁方一放手等待就醒,
/// 认领走同一条按锁仲裁的恢复路径。过期代数的醒来消息不起作用。
#[tokio::test]
async fn foreign_task_is_adopted_as_soon_as_its_lease_is_released() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    tokio::fs::write(&source, b"wait-for-me").await.unwrap();
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut owner = FileOperationQueue::new();
    attach_store(&mut owner, store.clone());
    let mut survivor = FileOperationQueue::new();
    attach_store(&mut survivor, store.clone());
    let FileOperationEnqueueOutcome::Queued { .. } = owner.enqueue(QueuedFileOperation::Copy {
        transfers: vec![QueuedTransfer::new(source, directory.path().join("target"))],
        verification: FileOperationVerification::BasicMetadata,
    }) else {
        panic!("recoverable copy should enqueue");
    };
    assert_persisted(&mut owner);
    let stored_task_id = owner.tasks()[0].stored_id.unwrap();

    assert!(survivor.adopt_foreign_tasks().is_none());
    assert!(survivor.tasks().is_empty());
    let waits = survivor.foreign_task_waits();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].stored_task_id, stored_task_id);
    // 再扫一次不会重复登记,也不会换代(订阅保持同一个等待)。
    assert!(survivor.adopt_foreign_tasks().is_none());
    assert_eq!(
        survivor.foreign_task_waits()[0].generation,
        waits[0].generation
    );

    let wait = waits[0].clone();
    let waiter = std::thread::spawn(move || wait.wait_for_release());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!waiter.is_finished(), "owner still holds the lease");
    drop(owner);
    waiter.join().unwrap().unwrap();

    assert!(survivor
        .accept_foreign_task_release(stored_task_id, waits[0].generation + 1)
        .is_none());
    assert!(survivor.tasks().is_empty());
    assert!(survivor
        .accept_foreign_task_release(stored_task_id, waits[0].generation)
        .is_none());
    assert_persisted(&mut survivor);
    assert_eq!(survivor.tasks().len(), 1);
    assert_eq!(survivor.tasks()[0].stored_id, Some(stored_task_id));
    assert!(survivor.foreign_task_waits().is_empty());
}
