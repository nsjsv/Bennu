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
    attach_store(&mut queue, store.clone());
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Move {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable move should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let stored_task_id = running.stored_id.unwrap();
    let (output, _messages) = tokio::sync::mpsc::channel(64);

    let completion = run_queued_transfers(
        transfers.clone(),
        running.controls,
        running.stored_id.unwrap(),
        task_id,
        &output,
        running.store,
        QueuedTransferMode::Move,
        FileOperationVerification::BasicMetadata,
    )
    .await;

    let FileOperationCompletion::Failed {
        error,
        partial_outcome,
    } = &completion
    else {
        panic!("expected plain failure completion");
    };
    assert!(
        error.contains("could not read directory"),
        "unexpected diagnostic: {error}"
    );
    assert!(partial_outcome.completed_path_migrations().is_empty());

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
        Some(file_operations::FileOperationTerminalStatus::Failed)
    );
    assert_persisted(&mut queue);
    assert_eq!(
        store.read_task(stored_task_id).unwrap().unwrap().status,
        bennu_state::StoredTaskStatus::Failed
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

/// 统计 `app_ui::file_operations` 下名为 `event_name` 的日志事件：
/// 只在测试线程上临时安装计数订阅器，正式代码里不放测试计数器。
struct FileOperationEventCounter {
    event_name: &'static str,
    count: Arc<std::sync::atomic::AtomicUsize>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for FileOperationEventCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct EventName(Option<String>);
        impl tracing::field::Visit for EventName {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "event" {
                    self.0 = Some(value.to_owned());
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }

        if event.metadata().target() != "app_ui::file_operations" {
            return;
        }
        let mut name = EventName(None);
        event.record(&mut name);
        if name.0.as_deref() == Some(self.event_name) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// 进度清单只为提前算总量：建不出来时记一条警告、这一项进度显示为未知，
/// 真正的错误在推进到这一项时照常报给用户。
#[tokio::test]
async fn an_unbuildable_progress_manifest_is_logged_and_the_real_error_still_surfaces() {
    use tracing_subscriber::layer::SubscriberExt;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let locked_child = source.join("locked");
    std::fs::create_dir_all(&locked_child).unwrap();
    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o000)).unwrap();
    // root 不受目录权限限制，读得到就测不出这条路径。
    if std::fs::read_dir(&locked_child).is_ok() {
        std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let transfers = vec![QueuedTransfer::new(
        source.clone(),
        directory.path().join("target"),
    )];
    let store = TaskQueueStore::new(directory.path().join("state.sqlite")).unwrap();
    let mut queue = FileOperationQueue::new();
    attach_store(&mut queue, store);
    let FileOperationEnqueueOutcome::Queued { task_id } =
        queue.enqueue(QueuedFileOperation::Copy {
            transfers: transfers.clone(),
            verification: FileOperationVerification::BasicMetadata,
        })
    else {
        panic!("recoverable copy should enqueue");
    };
    assert_persisted(&mut queue);
    let running = queue.active_subscription().unwrap();
    let (output, mut events) = tokio::sync::mpsc::channel(64);
    let unavailable_manifests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(FileOperationEventCounter {
        event_name: "progress_manifest_unavailable",
        count: Arc::clone(&unavailable_manifests),
    });

    let completion = {
        // 当前线程运行时：整个传输都在这个线程上跑，线程内订阅器看得到全部事件。
        let _subscriber = tracing::subscriber::set_default(subscriber);
        run_queued_transfers(
            transfers,
            running.controls,
            running.stored_id.unwrap(),
            task_id,
            &output,
            running.store,
            QueuedTransferMode::Copy,
            FileOperationVerification::BasicMetadata,
        )
        .await
    };
    drop(output);
    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        unavailable_manifests.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let FileOperationCompletion::Failed { error, .. } = &completion else {
        panic!("expected plain failure completion, got {completion:?}");
    };
    assert!(
        error.contains("could not read directory"),
        "unexpected diagnostic: {error}"
    );
    while let Some(event) = events.recv().await {
        assert!(!matches!(
            event,
            FileOperationEvent::Progressed {
                update: FileOperationProgressUpdate::Bytes { .. },
                ..
            }
        ));
    }
}
