use super::*;
use std::os::unix::fs::PermissionsExt;

/// 普通失败(非取消)也必须把恢复日志关账到终态,队列才能写入失败状态。
/// 回归场景:首条传输的源目录含无权限子目录,准备阶段扫描清单即失败;
/// 修复前日志行停留在 awaiting_manifest,finalize 拒绝写终态,任务在数据
/// 库里永远悬在中间状态、每次启动被恢复重跑并弹出存储错误。
#[tokio::test]
async fn prepare_failure_settles_journal_and_finalizes_failed_task() {
    let directory = tempfile::tempdir().unwrap();
    let first_source = directory.path().join("first-source");
    let locked_child = first_source.join("locked");
    std::fs::create_dir_all(&locked_child).unwrap();
    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o000)).unwrap();
    let second_source = directory.path().join("second-source");
    tokio::fs::write(&second_source, b"second-original")
        .await
        .unwrap();
    let transfers = vec![
        QueuedTransfer::new(first_source.clone(), directory.path().join("first-target")),
        QueuedTransfer::new(
            second_source.clone(),
            directory.path().join("second-target"),
        ),
    ];
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
    let (mut output, _messages) = iced::futures::channel::mpsc::channel(64);

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

    let FileOperationCompletion::Failed {
        error,
        completed_move_transfers,
    } = &completion
    else {
        panic!("expected plain failure completion");
    };
    assert!(
        error.contains("could not read directory"),
        "unexpected diagnostic: {error}"
    );
    assert!(completed_move_transfers.is_empty());

    let records = load_recoverable_transfer_records(store.clone(), stored_task_id)
        .await
        .unwrap();
    assert_eq!(records.len(), transfers.len());
    assert!(records.iter().all(|record| matches!(
        record.checkpoint,
        file_core::TransferCheckpoint::Failed { .. }
    )));
    assert_eq!(
        queue.finish(task_id, FileOperationFinish::Failed(error.clone())),
        (
            Some(crate::operation_queue::FileOperationTerminalStatus::Failed),
            None
        )
    );
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        file_operation_store::StoredTaskStatus::Failed
    );
    assert!(store
        .read_transfer_recovery(stored_task_id)
        .unwrap()
        .journal_entries
        .is_empty());

    assert!(tokio::fs::symlink_metadata(&first_source).await.is_ok());
    assert!(tokio::fs::symlink_metadata(&second_source).await.is_ok());
    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o755)).unwrap();
}
