use super::*;

fn paths() -> PendingWatchPaths {
    PendingWatchPaths::new(16, usize::MAX)
}

fn seconds(value: u64) -> Duration {
    Duration::from_secs(value)
}

#[test]
fn a_written_path_waits_until_it_has_been_quiet() {
    let path = PathBuf::from("/root/note.txt");
    let start = Instant::now();
    let mut pending = paths();

    pending
        .record(path.clone(), PathChange::Written, start)
        .unwrap();
    pending
        .record(path.clone(), PathChange::Written, start + seconds(1))
        .unwrap();

    assert!(pending.take_due(start + seconds(2)).is_empty());
    assert_eq!(pending.next_due(), Some(start + seconds(3)));
    assert_eq!(pending.take_due(start + seconds(3)), vec![path]);
    assert_eq!(pending.next_due(), None);
}

#[test]
fn a_path_that_never_goes_quiet_is_handed_over_after_the_longest_wait() {
    let path = PathBuf::from("/root/download.part");
    let start = Instant::now();
    let mut pending = paths();

    for second in 0..12 {
        pending
            .record(path.clone(), PathChange::Written, start + seconds(second))
            .unwrap();
        if second < 10 {
            assert!(pending.take_due(start + seconds(second)).is_empty());
        }
    }

    assert_eq!(pending.take_due(start + seconds(11)), vec![path]);
}

#[test]
fn a_file_created_and_deleted_before_settling_is_handed_over_once_after_it_is_gone() {
    let temporary = PathBuf::from("/root/temp/Xhc61SZJZE");
    let start = Instant::now();
    let mut pending = paths();

    pending
        .record(temporary.clone(), PathChange::Written, start)
        .unwrap();
    assert!(pending.take_due(start).is_empty());
    pending
        .record(
            temporary.clone(),
            PathChange::Removed,
            start + Duration::from_millis(5),
        )
        .unwrap();

    assert_eq!(
        pending.take_due(start + Duration::from_millis(5)),
        vec![temporary]
    );
    assert!(pending.take_due(start + seconds(30)).is_empty());
}

#[test]
fn a_path_that_keeps_changing_backs_off_up_to_a_minute() {
    let log = PathBuf::from("/root/app.log");
    let mut now = Instant::now();
    let mut pending = paths();
    let mut gaps = Vec::new();
    let mut last_handover = None;

    // 每秒追加一行，看相邻两次交出隔了多久。
    for _ in 0..400 {
        pending
            .record(log.clone(), PathChange::Written, now)
            .unwrap();
        if !pending.take_due(now).is_empty() {
            if let Some(previous) = last_handover.replace(now) {
                gaps.push(now.duration_since(previous).as_secs());
            }
        }
        now += seconds(1);
    }

    assert_eq!(&gaps[..5], &[11, 11, 20, 40, 60]);
    assert!(gaps[5..].iter().all(|gap| *gap == 60));
}

#[test]
fn backoff_starts_over_after_two_quiet_minutes() {
    let path = PathBuf::from("/root/note.txt");
    let start = Instant::now();
    let mut pending = paths();
    pending
        .record(path.clone(), PathChange::Removed, start)
        .unwrap();
    assert_eq!(pending.take_due(start), vec![path.clone()]);

    pending
        .record(path.clone(), PathChange::Removed, start + seconds(1))
        .unwrap();
    assert!(pending.take_due(start + seconds(1)).is_empty());
    assert_eq!(pending.take_due(start + seconds(5)), vec![path.clone()]);

    // 5 秒后又交过一次，退避翻倍到 10 秒；2 分钟没再交就复位到 5 秒。
    let later = start + seconds(5) + BACKOFF_MEMORY;
    pending
        .record(path.clone(), PathChange::Removed, later)
        .unwrap();
    assert_eq!(pending.take_due(later), vec![path.clone()]);
    pending
        .record(path.clone(), PathChange::Removed, later + seconds(1))
        .unwrap();
    assert!(pending.take_due(later + seconds(4)).is_empty());
    assert_eq!(pending.take_due(later + seconds(5)), vec![path]);
}

#[test]
fn a_full_queue_hands_everything_over_at_once() {
    let start = Instant::now();
    let mut pending = PendingWatchPaths::new(2, usize::MAX);
    pending
        .record(PathBuf::from("/root/a"), PathChange::Written, start)
        .unwrap();
    pending
        .record(PathBuf::from("/root/b"), PathChange::Written, start)
        .unwrap();

    let rejected = pending
        .record(PathBuf::from("/root/c"), PathChange::Written, start)
        .unwrap_err();
    let mut flushed = pending.take_all(start);
    flushed.sort();

    assert_eq!(rejected, PathBuf::from("/root/c"));
    assert_eq!(
        flushed,
        vec![PathBuf::from("/root/a"), PathBuf::from("/root/b")]
    );
    pending
        .record(rejected, PathChange::Written, start)
        .unwrap();
}

#[test]
fn a_dirty_root_drops_only_its_own_waiting_paths() {
    let start = Instant::now();
    let mut pending = paths();
    pending
        .record(PathBuf::from("/first/a"), PathChange::Written, start)
        .unwrap();
    pending
        .record(PathBuf::from("/second/b"), PathChange::Written, start)
        .unwrap();

    pending.forget_under(&[PathBuf::from("/first")]);

    assert_eq!(
        pending.take_due(start + seconds(2)),
        vec![PathBuf::from("/second/b")]
    );
}

#[test]
fn backoff_memory_is_bounded_and_unremembered_paths_are_not_delayed() {
    let start = Instant::now();
    let mut pending = PendingWatchPaths::new(MAX_REMEMBERED_HANDOVERS + 1, usize::MAX);
    for index in 0..MAX_REMEMBERED_HANDOVERS {
        pending
            .record(
                PathBuf::from(format!("/root/{index}")),
                PathChange::Removed,
                start,
            )
            .unwrap();
    }
    assert_eq!(pending.take_due(start).len(), MAX_REMEMBERED_HANDOVERS);

    let extra = PathBuf::from("/root/extra");
    pending
        .record(extra.clone(), PathChange::Removed, start)
        .unwrap();
    assert_eq!(pending.take_due(start), vec![extra.clone()]);
    pending
        .record(extra.clone(), PathChange::Removed, start + seconds(1))
        .unwrap();
    assert_eq!(
        pending.take_due(start + seconds(1)),
        vec![extra],
        "a path the full memory could not take is handed over without backoff"
    );
    assert_eq!(pending.handovers.len(), MAX_REMEMBERED_HANDOVERS);
}
