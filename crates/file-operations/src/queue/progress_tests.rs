use super::*;
use crate::progress::{CurrentFileProgress, FileOperationProgress, FileOperationProgressUpdate};

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
        total_items: Some(2),
    });
    let fraction = progress.fraction().unwrap();
    assert!((fraction - 0.1).abs() < f32::EPSILON);
    assert_eq!(progress.items(), Some((1, Some(2))));

    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 50,
        total_bytes: 1_000,
        completed_items: 1,
        total_items: Some(2),
    });
    assert_eq!(progress.fraction(), Some(fraction));

    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 1_000,
        total_bytes: 1_000,
        completed_items: 2,
        total_items: Some(2),
    });
    assert!(progress.fraction().unwrap() < 1.0);

    progress.mark_complete();
    assert_eq!(progress.fraction(), Some(1.0));
    assert_eq!(progress.bytes(), Some((1_000, 1_000)));
    assert_eq!(progress.items(), Some((2, Some(2))));
}

#[test]
fn item_progress_remains_indeterminate_and_tracks_completed_items() {
    let mut progress = FileOperationProgress::pending();

    progress.update(FileOperationProgressUpdate::IndeterminateItems {
        completed: 1,
        total: 3,
    });

    assert_eq!(progress.fraction(), None);
    assert_eq!(progress.items(), Some((1, Some(3))));
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
            total_items: Some(1),
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
        total_items: Some(1),
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
        total_items: Some(1),
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
            total_items: Some(3),
        },
        Vec::new(),
    );

    failed_queue.finish(
        failed_task_id,
        FileOperationFinish::Failed("write failed".to_owned()),
    );

    assert_eq!(failed_queue.tasks()[0].progress.fraction(), Some(0.4));
    assert_eq!(failed_queue.tasks()[0].progress.bytes(), Some((400, 1_000)));
    assert_eq!(failed_queue.tasks()[0].progress.items(), Some((1, Some(3))));

    let mut canceled_queue = FileOperationQueue::new();
    canceled_queue.enqueue(queued_directory_creation());
    let canceled_task_id = canceled_queue.tasks()[0].id;
    canceled_queue.update_progress(
        canceled_task_id,
        FileOperationProgressUpdate::Bytes {
            completed_bytes: 600,
            total_bytes: 1_000,
            completed_items: 2,
            total_items: Some(3),
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
    assert_eq!(
        canceled_queue.tasks()[0].progress.items(),
        Some((2, Some(3)))
    );
}

fn transfer_update(
    completed_items: usize,
    completed_bytes: u64,
    current_file: Option<CurrentFileProgress>,
) -> FileOperationProgressUpdate {
    FileOperationProgressUpdate::Transfer {
        completed_items,
        total_items: 2_049,
        bytes: Some((completed_bytes, 41)),
        current_file,
    }
}

#[test]
fn transfer_fraction_follows_items_even_when_bytes_stay_at_zero() {
    let mut progress = FileOperationProgress::pending();

    progress.update(transfer_update(508, 0, None));

    let fraction = progress.fraction().unwrap();
    assert!((fraction - 508.0 / 2_049.0).abs() < f32::EPSILON);
    assert_eq!(progress.bytes(), Some((0, 41)));
    assert_eq!(progress.items(), Some((508, Some(2_049))));

    progress.update(transfer_update(2_049, 41, None));
    assert!(progress.fraction().unwrap() < 1.0);
}

#[test]
fn transfer_items_only_grow_and_current_file_follows_the_latest_update() {
    let movie = CurrentFileProgress {
        name: "movie.mkv".to_owned(),
        completed_bytes: 100,
        total_bytes: 400,
    };
    let mut progress = FileOperationProgress::pending();

    progress.update(transfer_update(10, 0, Some(movie.clone())));
    progress.update(transfer_update(5, 0, None));

    assert_eq!(progress.items(), Some((10, Some(2_049))));
    assert_eq!(progress.current_file(), None);

    progress.update(transfer_update(11, 0, Some(movie.clone())));
    assert_eq!(progress.current_file(), Some(&movie));
    assert_eq!(movie.fraction(), Some(0.25));

    progress.mark_complete();
    assert_eq!(progress.current_file(), None);
    assert_eq!(progress.fraction(), Some(1.0));
    assert_eq!(progress.items(), Some((2_049, Some(2_049))));
}

/// 同一任务的进度条只用一种度量：先按项数，之后来的字节更新不改用字节。
#[test]
fn the_first_trusted_measure_keeps_driving_the_fraction() {
    let mut progress = FileOperationProgress::pending();

    progress.update(transfer_update(1_000, 0, None));
    progress.update(FileOperationProgressUpdate::Bytes {
        completed_bytes: 0,
        total_bytes: 41,
        completed_items: 1_000,
        total_items: Some(2_049),
    });

    let fraction = progress.fraction().unwrap();
    assert!((fraction - 1_000.0 / 2_049.0).abs() < f32::EPSILON);
}
