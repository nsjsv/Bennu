use super::*;
use crate::operation_progress::{FileOperationProgress, FileOperationProgressUpdate};

fn queued_directory_creation() -> QueuedFileOperation {
    QueuedFileOperation::CreateDirectory {
        parent: PathBuf::from("/tmp"),
    }
}

#[test]
fn byte_progress_uses_whole_task_bytes_without_reaching_terminal_fraction() {
    let mut progress = FileOperationProgress::pending();

    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 100,
        total_bytes: 1_000,
        completed_items: 1,
        total_items: 2,
    });
    let fraction = progress.fraction().unwrap();
    assert!((fraction - 0.1).abs() < f32::EPSILON);
    assert_eq!(progress.items(), Some((1, 2)));

    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 50,
        total_bytes: 1_000,
        completed_items: 1,
        total_items: 2,
    });
    assert_eq!(progress.fraction(), Some(fraction));

    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 1_000,
        total_bytes: 1_000,
        completed_items: 2,
        total_items: 2,
    });
    assert!(progress.fraction().unwrap() < 1.0);

    progress.mark_complete();
    assert_eq!(progress.fraction(), Some(1.0));
    assert_eq!(progress.bytes(), Some((1_000, 1_000)));
    assert_eq!(progress.items(), Some((2, 2)));
}

#[test]
fn item_progress_remains_indeterminate_and_tracks_completed_items() {
    let mut progress = FileOperationProgress::pending();

    progress.update(FileOperationProgressUpdate::IndeterminateItems {
        completed: 1,
        total: 3,
    });

    assert_eq!(progress.fraction(), None);
    assert_eq!(progress.items(), Some((1, 3)));
}

#[test]
fn animation_subscription_is_needed_only_for_active_indeterminate_work() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(queued_directory_creation());
    let task_id = queue.tasks()[0].id;

    assert!(queue.has_active_indeterminate_progress());

    queue.update_progress(
        task_id,
        FileOperationProgressUpdate::Bytes {
            completed_bytes: 1,
            total_bytes: 10,
            completed_items: 0,
            total_items: 1,
        },
        Vec::new(),
    );

    assert!(!queue.has_active_indeterminate_progress());
}

#[test]
fn paused_and_terminal_tasks_ignore_late_progress_updates() {
    let mut queue = FileOperationQueue::new();
    queue.enqueue(queued_directory_creation());
    let task_id = queue.tasks()[0].id;
    let update = |completed_bytes| FileOperationProgressUpdate::Bytes {
        completed_bytes,
        total_bytes: 1_000,
        completed_items: 0,
        total_items: 1,
    };

    queue.update_progress(task_id, update(100), Vec::new());
    assert_eq!(queue.tasks()[0].progress.bytes(), Some((100, 1_000)));

    queue.toggle_pause(task_id);
    queue.update_progress(task_id, update(500), Vec::new());
    assert_eq!(queue.tasks()[0].progress.bytes(), Some((100, 1_000)));

    queue.toggle_pause(task_id);
    queue.update_progress(task_id, update(500), Vec::new());
    assert_eq!(queue.tasks()[0].progress.bytes(), Some((500, 1_000)));

    queue.finish(
        task_id,
        FileOperationFinish::Failed("write failed".to_owned()),
    );
    queue.update_progress(task_id, update(900), Vec::new());
    assert_eq!(queue.tasks()[0].progress.bytes(), Some((500, 1_000)));
    assert_eq!(queue.tasks()[0].progress.fraction(), Some(0.5));
}

#[test]
fn indeterminate_update_does_not_demote_known_byte_progress() {
    let mut progress = FileOperationProgress::pending();
    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 250,
        total_bytes: 1_000,
        completed_items: 0,
        total_items: 1,
    });

    progress.update(FileOperationProgressUpdate::Indeterminate);

    assert_eq!(progress.fraction(), Some(0.25));
    assert_eq!(progress.bytes(), Some((250, 1_000)));
}

#[test]
fn indeterminate_failed_task_has_no_determinate_fraction() {
    let progress = FileOperationProgress::pending();

    assert_eq!(progress.fraction(), None);
}

#[test]
fn failure_and_cancellation_preserve_last_trusted_progress() {
    let mut failed_queue = FileOperationQueue::new();
    failed_queue.enqueue(queued_directory_creation());
    let failed_task_id = failed_queue.tasks()[0].id;
    failed_queue.update_progress(
        failed_task_id,
        FileOperationProgressUpdate::Bytes {
            completed_bytes: 400,
            total_bytes: 1_000,
            completed_items: 1,
            total_items: 3,
        },
        Vec::new(),
    );

    failed_queue.finish(
        failed_task_id,
        FileOperationFinish::Failed("write failed".to_owned()),
    );

    assert_eq!(failed_queue.tasks()[0].progress.fraction(), Some(0.4));
    assert_eq!(failed_queue.tasks()[0].progress.bytes(), Some((400, 1_000)));
    assert_eq!(failed_queue.tasks()[0].progress.items(), Some((1, 3)));

    let mut canceled_queue = FileOperationQueue::new();
    canceled_queue.enqueue(queued_directory_creation());
    let canceled_task_id = canceled_queue.tasks()[0].id;
    canceled_queue.update_progress(
        canceled_task_id,
        FileOperationProgressUpdate::Bytes {
            completed_bytes: 600,
            total_bytes: 1_000,
            completed_items: 2,
            total_items: 3,
        },
        Vec::new(),
    );
    canceled_queue.cancel(canceled_task_id);
    canceled_queue.finish(canceled_task_id, FileOperationFinish::Canceled);

    assert_eq!(canceled_queue.tasks()[0].progress.fraction(), Some(0.6));
    assert_eq!(
        canceled_queue.tasks()[0].progress.bytes(),
        Some((600, 1_000))
    );
    assert_eq!(canceled_queue.tasks()[0].progress.items(), Some((2, 3)));
}
