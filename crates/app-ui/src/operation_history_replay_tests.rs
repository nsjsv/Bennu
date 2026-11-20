//! 一条记录多项的重放、排队的撤销请求，以及转换、压缩、解压生成的东西的撤销。

use super::tests::{end_replay, entry, moved, request, transfer, FakeQueue};
use super::*;

/// 转换、压缩、解压生成的东西：撤销 = 移到回收站，重做 = 从回收站拿回来（同 Nautilus 撤销压缩、解压）。
#[test]
fn made_files_go_to_the_trash_on_undo_and_come_back_on_redo() {
    let made = [
        (
            FileOperationOutcome::Convert {
                failures: Vec::new(),
                outputs: vec![PathBuf::from("/music/a.m4a"), PathBuf::from("/music/b.m4a")],
            },
            vec![PathBuf::from("/music/a.m4a"), PathBuf::from("/music/b.m4a")],
        ),
        (
            FileOperationOutcome::CreatedArchive {
                archive: PathBuf::from("/docs/report.zip"),
            },
            vec![PathBuf::from("/docs/report.zip")],
        ),
        (
            FileOperationOutcome::Extracted {
                placed: PathBuf::from("/docs/photos"),
            },
            vec![PathBuf::from("/docs/photos")],
        ),
    ];
    for (outcome, paths) in made {
        let mut history = FileOperationHistory::new();
        let mut queue = FakeQueue::default();
        history.record(TaskEnd::Completed(&outcome));

        request(&mut history, &mut queue, HistoryDirection::Undo);
        let (task_id, undo) = queue.last();
        assert!(
            matches!(&undo, QueuedFileOperation::Trash { paths: trashed } if *trashed == paths),
            "{undo:?}"
        );

        let entries = paths
            .iter()
            .map(|path| {
                TrashRestoreEntry::from_historical_paths(
                    PathBuf::from("/trash/files").join(path.file_name().unwrap()),
                    PathBuf::from("/trash/info").join(path.file_name().unwrap()),
                    path.clone(),
                )
            })
            .collect::<Vec<_>>();
        end_replay(
            &mut history,
            &mut queue,
            task_id,
            TaskEnd::Completed(&FileOperationOutcome::Trash {
                entries: entries.clone(),
                untrashable: Vec::new(),
            }),
        );

        let redo = history
            .next_redo_operation()
            .expect("redo brings them back");
        assert!(
            matches!(&redo, QueuedFileOperation::Restore { entries: restored } if *restored == entries),
            "{redo:?}"
        );
    }
}

/// 一个都没转成的转换没有可撤销的东西。
#[test]
fn a_conversion_that_made_nothing_leaves_no_undo() {
    let mut history = FileOperationHistory::new();
    history.record(TaskEnd::Completed(&FileOperationOutcome::Convert {
        failures: vec!["a.mp4: broken".to_owned()],
        outputs: Vec::new(),
    }));
    assert!(history.next_undo_operation().is_none());
}

fn renamed(from: &str, to: &str) -> FileOperationHistoryItem {
    FileOperationHistoryItem::Rename {
        from: PathBuf::from(from),
        to: PathBuf::from(to),
    }
}

fn completed_rename(from: &str, to: &str) -> FileOperationOutcome {
    FileOperationOutcome::Rename {
        from: PathBuf::from(from),
        to: PathBuf::from(to),
    }
}

/// 一个动作是一条记录：撤销从最后一步倒着来，一次只放一个操作；整条撤完以后，
/// 一次重做按原来的顺序从第一步来。
#[test]
fn an_entry_is_undone_last_step_first_and_redone_first_step_first() {
    let mut history = FileOperationHistory::new();
    let mut queue = FakeQueue::default();
    history.undo_stack.push(HistoryEntry {
        items: vec![
            renamed("/a/one", "/a/two"),
            FileOperationHistoryItem::Move {
                transfers: vec![transfer("/a/two", "/b/two")],
            },
        ],
    });

    request(&mut history, &mut queue, HistoryDirection::Undo);
    assert_eq!(queue.operations.len(), 1, "one operation at a time");
    let (move_back, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Move { transfers, .. } if transfers[0].source == Path::new("/b/two")),
        "{operation:?}"
    );
    end_replay(
        &mut history,
        &mut queue,
        move_back,
        TaskEnd::Completed(&moved(vec![transfer("/b/two", "/a/two")])),
    );
    let (rename_back, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Rename { path, .. } if path == Path::new("/a/two")),
        "{operation:?}"
    );
    end_replay(
        &mut history,
        &mut queue,
        rename_back,
        TaskEnd::Completed(&completed_rename("/a/two", "/a/one")),
    );
    assert!(history.undo_stack.is_empty());
    assert_eq!(history.redo_stack.len(), 1, "the whole action is one redo");

    request(&mut history, &mut queue, HistoryDirection::Redo);
    let (rename_again, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Rename { path, .. } if path == Path::new("/a/one")),
        "{operation:?}"
    );
    end_replay(
        &mut history,
        &mut queue,
        rename_again,
        TaskEnd::Completed(&completed_rename("/a/one", "/a/two")),
    );
    let (move_again, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Move { transfers, .. } if transfers[0].source == Path::new("/a/two")),
        "{operation:?}"
    );
    end_replay(
        &mut history,
        &mut queue,
        move_again,
        TaskEnd::Completed(&moved(vec![transfer("/a/two", "/b/two")])),
    );
    assert_eq!(history.undo_stack.len(), 1);
    assert_eq!(history.undo_stack[0].items.len(), 2);
}

/// 撤销到一半停下：撤好的成为一条重做，剩下的留成一条，再按一次接着撤；
/// 停下以后排着的请求作废。
#[test]
fn a_stopped_entry_keeps_the_rest_for_the_next_undo() {
    let mut history = FileOperationHistory::new();
    let mut queue = FakeQueue::default();
    history
        .undo_stack
        .push(entry(renamed("/other/x", "/other/y")));
    history.undo_stack.push(HistoryEntry {
        items: vec![
            renamed("/a/one", "/a/two"),
            renamed("/a/two", "/a/three"),
            renamed("/a/three", "/a/four"),
        ],
    });

    request(&mut history, &mut queue, HistoryDirection::Undo);
    request(&mut history, &mut queue, HistoryDirection::Undo);
    let (first, _) = queue.last();
    end_replay(
        &mut history,
        &mut queue,
        first,
        TaskEnd::Completed(&completed_rename("/a/four", "/a/three")),
    );
    let (second, _) = queue.last();
    end_replay(
        &mut history,
        &mut queue,
        second,
        TaskEnd::Stopped(&FileOperationOutcome::NoHistory),
    );

    assert_eq!(queue.operations.len(), 2, "the queued press was dropped");
    assert_eq!(history.redo_stack.len(), 1);
    assert_eq!(
        history.redo_stack[0].items,
        vec![renamed("/a/three", "/a/four")]
    );
    assert_eq!(history.undo_stack.len(), 2);
    assert_eq!(
        history.undo_stack[1].items,
        vec![renamed("/a/one", "/a/two"), renamed("/a/two", "/a/three")]
    );
    let Some(QueuedFileOperation::Rename { path, .. }) = history.next_undo_operation() else {
        panic!("expected the stopped rename to be next");
    };
    assert_eq!(path, PathBuf::from("/a/three"));
}

/// 连按两次 Ctrl+Z：第二次等第一条撤完再开始，两条的操作不会交错。
#[test]
fn presses_during_a_replay_wait_their_turn() {
    let mut history = FileOperationHistory::new();
    let mut queue = FakeQueue::default();
    history.undo_stack.push(entry(renamed("/a/old", "/a/new")));
    history.undo_stack.push(HistoryEntry {
        items: vec![renamed("/b/1", "/b/2"), renamed("/b/2", "/b/3")],
    });

    request(&mut history, &mut queue, HistoryDirection::Undo);
    request(&mut history, &mut queue, HistoryDirection::Undo);
    assert_eq!(queue.operations.len(), 1);

    let (first, _) = queue.last();
    end_replay(
        &mut history,
        &mut queue,
        first,
        TaskEnd::Completed(&completed_rename("/b/3", "/b/2")),
    );
    let (second, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Rename { path, .. } if path == Path::new("/b/2"))
    );
    end_replay(
        &mut history,
        &mut queue,
        second,
        TaskEnd::Completed(&completed_rename("/b/2", "/b/1")),
    );
    let (_, operation) = queue.last();
    assert!(
        matches!(&operation, QueuedFileOperation::Rename { path, .. } if path == Path::new("/a/new")),
        "the second press starts only after the first entry is done: {operation:?}"
    );
}

/// 队列拒收：这条原样回到撤销栈，原因交给界面报出来。
#[test]
fn a_rejected_replay_puts_the_entry_back() {
    let mut history = FileOperationHistory::new();
    history
        .undo_stack
        .push(entry(FileOperationHistoryItem::Move {
            transfers: vec![transfer("/a", "/b")],
        }));

    let rejection = history.request(HistoryDirection::Undo, &mut |_| {
        FileOperationEnqueueOutcome::Rejected {
            error: "storage is unavailable".to_owned(),
        }
    });

    assert_eq!(rejection.as_deref(), Some("storage is unavailable"));
    assert_eq!(history.undo_depth(), 1);
    assert!(history.replay.is_none());
}
