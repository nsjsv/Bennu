use super::*;

#[test]
fn dirty_root_recovery_precedes_background_watch_budget_patrol() {
    let root = PathBuf::from("/root");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![root.clone()]);
    pending.absorb_request(
        DaemonWorkRequest::WatchBudgetPatrol {
            directories: vec![root.join("directory")],
        },
        now,
    );
    pending.request_dirty_roots(vec![root.clone()], now);

    assert_eq!(
        pending.take_next_work(now + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::DirtyRootRecovery { roots: vec![root] })
    );
    assert!(matches!(
        pending.take_next_work(now + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::WatchBudgetPatrol { .. })
    ));
}

#[test]
fn watch_budget_patrol_waits_for_the_post_completion_interval() {
    let root = PathBuf::from("/root");
    let directory = root.join("directory");
    let queue = DaemonWorkQueue::new(vec![root]);
    let started_at = Instant::now();
    assert!(queue
        .try_enqueue_watch_budget_patrol_at(std::slice::from_ref(&directory), started_at)
        .unwrap());
    assert!(matches!(
        queue.wait_for_next_work(),
        DaemonWorkRequest::WatchBudgetPatrol { .. }
    ));
    queue.finish_work_at(
        &WorkOutcome::Succeeded {
            next_content_refresh: None,
        },
        started_at,
    );

    assert!(!queue
        .try_enqueue_watch_budget_patrol_at(
            std::slice::from_ref(&directory),
            started_at + WATCH_BUDGET_PATROL_INTERVAL - Duration::from_millis(1),
        )
        .unwrap());
    assert!(queue
        .try_enqueue_watch_budget_patrol_at(
            std::slice::from_ref(&directory),
            started_at + WATCH_BUDGET_PATROL_INTERVAL,
        )
        .unwrap());
}

fn failed(message: &str) -> WorkOutcome {
    WorkOutcome::Failed {
        message: message.to_owned(),
    }
}

fn incomplete(message: &str) -> IndexCompleteness {
    IndexCompleteness::Incomplete {
        message: message.to_owned(),
    }
}

/// 不等真实时间：直接在内部队列上按给定时刻取工作，并像 `wait_for_next_work` 一样记下在做什么。
fn take_work_at(queue: &DaemonWorkQueue, now: Instant) -> Option<DaemonWorkRequest> {
    let mut guard = queue.state.lock().unwrap();
    let state = &mut *guard;
    let work_request = state.pending_work.take_next_work(now)?;
    state.in_flight = InFlightWork::of(&work_request, &state.pending_work);
    Some(work_request)
}

#[test]
fn a_failed_full_check_stays_incomplete_until_its_retry_succeeds() {
    let root = PathBuf::from("/root");
    let queue = DaemonWorkQueue::starting_with_full_check(vec![root.clone()], Vec::new());
    let failed_at = Instant::now();
    assert_eq!(
        take_work_at(&queue, failed_at),
        Some(DaemonWorkRequest::StartupCheck)
    );
    assert_eq!(
        queue.finish_work_at(&failed("disk unavailable"), failed_at),
        incomplete("disk unavailable")
    );

    queue
        .enqueue(DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![root.join("note.txt")],
        })
        .unwrap();
    assert!(matches!(
        take_work_at(&queue, failed_at),
        Some(DaemonWorkRequest::ChangedPaths { .. })
    ));
    // 一次小更新成功，盖不掉全量检查的失败。
    assert_eq!(
        queue.finish_work_at(
            &WorkOutcome::Succeeded {
                next_content_refresh: None,
            },
            failed_at
        ),
        incomplete("disk unavailable")
    );

    let retry_at = failed_at + DIRTY_ROOT_RETRY_BASE;
    assert_eq!(
        take_work_at(&queue, retry_at - Duration::from_millis(1)),
        None
    );
    assert_eq!(
        take_work_at(&queue, retry_at),
        Some(DaemonWorkRequest::DirtyRootRecovery {
            roots: vec![root.clone()]
        })
    );
    assert_eq!(
        queue.finish_work_at(&failed("still unavailable"), retry_at),
        incomplete("still unavailable")
    );

    let second_retry_at = retry_at + dirty_root_retry_delay(2);
    assert_eq!(
        take_work_at(&queue, second_retry_at),
        Some(DaemonWorkRequest::DirtyRootRecovery { roots: vec![root] })
    );
    assert_eq!(
        queue.finish_work_at(
            &WorkOutcome::Succeeded {
                next_content_refresh: None,
            },
            second_retry_at
        ),
        IndexCompleteness::Complete
    );
}

#[test]
fn a_failed_local_update_rechecks_only_the_root_it_touched() {
    let first = PathBuf::from("/first");
    let second = PathBuf::from("/second");
    let queue = DaemonWorkQueue::starting_with_full_check(vec![first.clone(), second], Vec::new());
    let now = Instant::now();
    assert_eq!(
        take_work_at(&queue, now),
        Some(DaemonWorkRequest::StartupCheck)
    );
    assert_eq!(
        queue.finish_work_at(
            &WorkOutcome::Succeeded {
                next_content_refresh: None,
            },
            now
        ),
        IndexCompleteness::Complete
    );

    for work_request in [
        DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![first.join("note.txt")],
        },
        DaemonWorkRequest::CoverageRepair {
            scopes: vec![first.join("folder")],
        },
        DaemonWorkRequest::WatchBudgetPatrol {
            directories: vec![first.join("unwatched")],
        },
    ] {
        queue.enqueue(work_request).unwrap();
        assert!(take_work_at(&queue, now).is_some());
        assert_eq!(
            queue.finish_work_at(&failed("write failed"), now),
            incomplete("write failed")
        );
        let state = queue.state.lock().unwrap();
        assert_eq!(
            state.pending_work.dirty_roots.keys().collect::<Vec<_>>(),
            vec![&first]
        );
    }
}

#[test]
fn retrying_now_only_moves_failed_roots_that_are_not_running() {
    let failed_root = PathBuf::from("/failed");
    let running_root = PathBuf::from("/running");
    let overflowed_root = PathBuf::from("/overflowed");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![
        failed_root.clone(),
        running_root.clone(),
        overflowed_root.clone(),
    ]);
    pending.record_failed_roots(vec![running_root.clone()], "failed", now);
    let running_at = now + DIRTY_ROOT_RETRY_BASE;
    assert_eq!(
        pending.take_next_work(running_at),
        Some(DaemonWorkRequest::DirtyRootRecovery {
            roots: vec![running_root]
        })
    );
    pending.record_failed_roots(vec![failed_root.clone()], "failed", running_at);
    pending.request_dirty_roots(vec![overflowed_root], running_at);

    pending.retry_failed_roots_now(running_at);

    assert_eq!(
        pending.take_next_work(running_at),
        Some(DaemonWorkRequest::DirtyRootRecovery {
            roots: vec![failed_root]
        })
    );
    assert_eq!(pending.take_next_work(running_at), None);
}

#[test]
fn cancelled_work_records_no_failure() {
    let root = PathBuf::from("/root");
    let queue = DaemonWorkQueue::starting_with_full_check(vec![root], Vec::new());
    let now = Instant::now();
    assert!(take_work_at(&queue, now).is_some());

    assert_eq!(
        queue.finish_work_at(&WorkOutcome::Cancelled, now),
        IndexCompleteness::Complete
    );
    let state = queue.state.lock().unwrap();
    assert!(state.pending_work.dirty_roots.is_empty());
}

#[test]
fn content_refresh_waits_for_its_alarm_and_yields_to_dirty_roots() {
    let root = PathBuf::from("/root");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![root.clone()]);
    pending.content_refresh_at = Some(now + Duration::from_secs(1));
    pending.absorb_request(
        DaemonWorkRequest::WatchBudgetPatrol {
            directories: vec![root.join("directory")],
        },
        now,
    );

    assert_eq!(pending.next_deadline(), Some(now + Duration::from_secs(1)));
    assert!(matches!(
        pending.take_next_work(now),
        Some(DaemonWorkRequest::WatchBudgetPatrol { .. })
    ));
    assert_eq!(pending.take_next_work(now), None);

    let due = now + Duration::from_secs(1);
    pending.request_dirty_roots(vec![root.clone()], now);
    assert_eq!(
        pending.take_next_work(due + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::DirtyRootRecovery { roots: vec![root] })
    );
    assert_eq!(
        pending.take_next_work(due + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::ContentRefresh)
    );
    assert_eq!(pending.content_refresh_at, None);
    assert_eq!(pending.take_next_work(due + DIRTY_ROOT_QUIET_WINDOW), None);
}

#[test]
fn every_successful_work_replaces_the_content_refresh_alarm() {
    let root = PathBuf::from("/root");
    let queue = DaemonWorkQueue::new(vec![root.clone()]);
    let now = Instant::now();
    queue
        .enqueue(DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![root.join("a")],
        })
        .unwrap();
    queue.wait_for_next_work();
    queue.finish_work_at(
        &WorkOutcome::Succeeded {
            next_content_refresh: Some(now),
        },
        now,
    );
    assert_eq!(
        queue.wait_for_next_work(),
        DaemonWorkRequest::ContentRefresh
    );

    queue.finish_work_at(
        &WorkOutcome::Failed {
            message: "refresh failed".to_owned(),
        },
        now,
    );
    let state = queue.state.lock().unwrap();
    assert_eq!(
        state.pending_work.content_refresh_at, None,
        "a failed refresh leaves re-arming to the next successful work"
    );
}
