//! 测试用：像正式版一样把任务跑完——任务产生的消息交回 `update`，直到没有新的事可做。
//! 队列驱动者真的执行文件操作。

use iced::futures::StreamExt;
use iced::Task;

use super::FileBrowser;
use crate::model::Message;

pub(crate) async fn drive(browser: &mut FileBrowser, task: Task<Message>) {
    let mut pending = vec![task];
    while let Some(task) = pending.pop() {
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            continue;
        };
        while let Some(action) =
            tokio::time::timeout(std::time::Duration::from_secs(20), stream.next())
                .await
                .expect("a task hung")
        {
            if let iced_runtime::Action::Output(message) = action {
                pending.push(browser.update(message));
            }
        }
    }
}

/// 写进任务库的移动撤销以后还能重做：写库不改任务在队列里的 id，撤销的重放靠它认结束。
#[tokio::test]
async fn an_undone_persisted_move_can_be_redone() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let source = root.join("note.txt");
    let target_folder = root.join("archive");
    std::fs::write(&source, b"hello").unwrap();
    std::fs::create_dir(&target_folder).unwrap();
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.system_focused_window = Some(iced::window::Id::unique());
    file_operations::test_persistence::attach_store(
        &mut browser.operation_queue,
        bennu_state::TaskQueueStore::new(root.join("state.sqlite")).unwrap(),
    );

    let move_task = browser.enqueue_file_operation(file_operations::QueuedFileOperation::Move {
        transfers: vec![file_operations::QueuedTransfer::new(
            source.clone(),
            target_folder.join("note.txt"),
        )],
        verification: Default::default(),
    });
    drive(&mut browser, move_task).await;
    assert!(target_folder.join("note.txt").exists());

    let undo = browser.undo_file_operation();
    drive(&mut browser, undo).await;
    assert!(source.exists());
    assert!(
        browser.operation_history.next_redo_operation().is_some(),
        "the undone move went to the redo stack"
    );

    let redo = browser.redo_file_operation();
    drive(&mut browser, redo).await;
    assert!(target_folder.join("note.txt").exists());
    assert!(!source.exists());
}
