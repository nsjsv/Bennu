// 驱动者签发不变量：任何让任务进入 Running/Canceling 的队列转换，
// 都在同一次处理里把驱动者交出去，而不是等某个统一出口来轮询。
use std::path::PathBuf;

use iced::futures::StreamExt;

use super::*;
use crate::config;
use crate::operation_history::FileOperationOutcome;
use crate::operation_queue::test_persistence::attach_store;
use crate::operation_queue::FileOperationStatus;

fn create_directory() -> QueuedFileOperation {
    QueuedFileOperation::CreateDirectory {
        parent: PathBuf::from("/workspace"),
    }
}

fn assert_active_tasks_have_drivers(browser: &FileBrowser) {
    for task in browser.operation_queue.tasks() {
        if matches!(
            task.status,
            FileOperationStatus::Running | FileOperationStatus::Canceling
        ) {
            assert!(
                task.driver_running,
                "task {} is {:?} without a driver",
                task.id, task.status
            );
        }
    }
}

#[test]
fn enqueue_launches_the_driver_in_the_same_transition() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.enqueue_file_operation(create_directory()));

    let task = &browser.operation_queue.tasks()[0];
    assert_eq!(task.status, FileOperationStatus::Running);
    assert!(task.driver_running);
}

#[test]
fn finishing_the_active_task_launches_the_next_one() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.enqueue_file_operation(create_directory()));
    drop(browser.enqueue_file_operation(create_directory()));
    let first = browser.operation_queue.tasks()[0].id;
    assert_eq!(
        browser.operation_queue.tasks()[1].status,
        FileOperationStatus::Pending
    );

    drop(browser.update(Message::FileOperationFinished(
        first,
        0,
        FileOperationCompletion::Succeeded(FileOperationOutcome::NoHistory),
    )));

    let second = &browser.operation_queue.tasks()[1];
    assert_eq!(second.status, FileOperationStatus::Running);
    assert!(second.driver_running);
    assert_active_tasks_have_drivers(&browser);
}

#[tokio::test]
async fn persistence_receipt_launches_the_recoverable_task_it_unblocks() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let operation_directory = tempfile::tempdir().expect("create operation directory");
    attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(operation_directory.path().join("state.sqlite"))
            .expect("create operation store"),
    );

    let enqueue = browser.enqueue_file_operation(QueuedFileOperation::Copy {
        transfers: vec![crate::operation_queue::QueuedTransfer::new(
            PathBuf::from("/workspace/source"),
            PathBuf::from("/workspace/target"),
        )],
        verification: file_core::FileOperationVerification::default(),
    });
    // 可恢复任务要等写库回执配上存储 id 才能启动。
    let task = &browser.operation_queue.tasks()[0];
    assert_eq!(task.status, FileOperationStatus::Pending);
    assert!(!task.driver_running);

    let mut stream = iced_runtime::task::into_stream(enqueue).expect("persistence request");
    let Some(iced_runtime::Action::Output(receipt)) = stream.next().await else {
        panic!("enqueue must produce the persistence receipt");
    };
    assert!(matches!(
        receipt,
        Message::FileOperationPersistenceFinished(_)
    ));
    drop(browser.update(receipt));

    let task = &browser.operation_queue.tasks()[0];
    assert_eq!(task.status, FileOperationStatus::Running);
    assert!(task.driver_running);
}

#[test]
fn cancel_and_supervision_keep_every_active_task_driven() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.enqueue_file_operation(create_directory()));
    drop(browser.enqueue_file_operation(create_directory()));
    let running = browser.operation_queue.tasks()[0].id;

    drop(browser.update(Message::OperationSupervisionTick));
    assert_active_tasks_have_drivers(&browser);

    drop(browser.update(Message::FileOperationCancelRequested(running)));
    assert_eq!(
        browser.operation_queue.tasks()[0].status,
        FileOperationStatus::Canceling
    );
    assert_active_tasks_have_drivers(&browser);
}
