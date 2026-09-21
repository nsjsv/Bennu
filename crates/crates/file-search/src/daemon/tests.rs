use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind};
use rusqlite::Connection;
use tempfile::tempdir;

use crate::model::{IndexHealth, IndexPhase, IndexStatus, SearchQuery};
use crate::SearchIndexConfig;

use super::*;

fn snapshot_for(phase: DaemonLifecyclePhase) -> DaemonLifecycleSnapshot {
    DaemonLifecycleSnapshot {
        phase,
        maintenance_backend_failure: None,
        recovery_rebuild_message: None,
        watch_coverage_health: WatchCoverageHealth::Healthy,
        visible_indexed_files: 41,
        capabilities: Vec::new(),
        path_configuration: Default::default(),
    }
}

#[test]
fn semantic_sidecar_failure_keeps_the_last_effective_configuration() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    let effective = VersionedSearchPathPreferences {
        revision: 3,
        preferences: SearchPathPreferences::default(),
    };
    database
        .initialize_search_path_configuration(&effective)
        .unwrap();
    let path_store = SearchPathStore::at(directory.path().join("search-paths.json"));
    path_store
        .replace(&VersionedSearchPathPreferences {
            revision: 4,
            preferences: SearchPathPreferences {
                custom_roots: vec![PathBuf::from("relative")],
                exclusions: Vec::new(),
            },
        })
        .unwrap();
    let base_config = SearchIndexConfig {
        roots: vec![home.clone()],
        ..SearchIndexConfig::default()
    };

    let runtime =
        initialize_path_configuration(&database, &base_config, Some(&home), Some(&path_store))
            .unwrap();

    assert_eq!(runtime.desired.revision, 4);
    assert_eq!(runtime.effective, effective);
    assert!(matches!(
        runtime.phase,
        SearchPathConfigurationPhase::Failed { .. }
    ));
    assert_eq!(
        runtime.policy.unwrap().preferences(),
        &effective.preferences
    );
}

#[test]
fn startup_repairs_an_older_sidecar_from_the_newer_effective_snapshot() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    let effective = VersionedSearchPathPreferences {
        revision: 5,
        preferences: SearchPathPreferences {
            custom_roots: Vec::new(),
            exclusions: vec![home.join("private")],
        },
    };
    database
        .initialize_search_path_configuration(&effective)
        .unwrap();
    let store = SearchPathStore::at(database_path.with_file_name("search-paths.json"));
    store
        .replace(&VersionedSearchPathPreferences {
            revision: 4,
            preferences: SearchPathPreferences::default(),
        })
        .unwrap();

    let runtime = initialize_path_configuration(
        &database,
        &SearchIndexConfig::default(),
        Some(&home),
        Some(&store),
    )
    .unwrap();

    assert_eq!(runtime.desired, effective);
    assert_eq!(runtime.effective, effective);
    assert_eq!(store.load().unwrap(), Some(effective));
}

#[test]
fn retry_repairs_a_damaged_sidecar_without_advancing_the_revision() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let database_path = directory.path().join("search.sqlite");
    let database = SearchDatabase::open(&database_path).unwrap();
    let effective = VersionedSearchPathPreferences {
        revision: 3,
        preferences: SearchPathPreferences::default(),
    };
    database
        .initialize_search_path_configuration(&effective)
        .unwrap();
    drop(database);
    let sidecar = database_path.with_file_name("search-paths.json");
    fs::write(&sidecar, b"not-json").unwrap();
    let core = SearchDaemonCore::new(
        database_path,
        SearchIndexConfig {
            roots: vec![home],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();

    let applied = core
        .configure_search_paths(3, SearchPathPreferences::default())
        .unwrap();

    assert_eq!(applied.revision, 3);
    assert_eq!(
        SearchPathStore::at(sidecar).load().unwrap(),
        Some(effective)
    );
    core.shutdown().unwrap();
}

#[test]
fn failed_transition_retries_the_same_desired_revision() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let archive = directory.path().join("archive");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&archive).unwrap();
    let database_path = directory.path().join("search.sqlite");
    let core = SearchDaemonCore::new(
        database_path.clone(),
        SearchIndexConfig {
            roots: vec![home],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();
    let preferences = SearchPathPreferences {
        custom_roots: vec![archive],
        exclusions: Vec::new(),
    };
    let schema_connection = Connection::open(&database_path).unwrap();
    schema_connection
        .execute_batch("ALTER TABLE files RENAME TO files_unavailable")
        .unwrap();

    assert!(core.configure_search_paths(0, preferences.clone()).is_err());
    assert_eq!(core.current_path_preferences().revision, 1);
    assert!(matches!(
        core.current_status().path_configuration.phase,
        SearchPathConfigurationPhase::Failed { .. }
    ));

    schema_connection
        .execute_batch("ALTER TABLE files_unavailable RENAME TO files")
        .unwrap();
    let applied = core.configure_search_paths(1, preferences).unwrap();

    assert_eq!(applied.revision, 1);
    assert_eq!(
        core.current_status().path_configuration.effective_revision,
        1
    );
    core.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_root_is_reported_unavailable_at_the_status_boundary() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let target = directory.path().join("target");
    let root = directory.path().join("root-link");
    fs::create_dir(&target).unwrap();
    symlink(&target, &root).unwrap();

    let statuses = root_statuses(std::slice::from_ref(&root), &[]);

    assert!(matches!(
        statuses.as_slice(),
        [SearchRootStatus {
            availability: SearchRootAvailability::Unavailable { .. },
            ..
        }]
    ));
}

#[test]
fn missing_root_is_saved_and_a_status_poll_activates_it_after_creation() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let missing = directory.path().join("later");
    fs::create_dir(&home).unwrap();
    let database_path = directory.path().join("search.sqlite");
    let core = SearchDaemonCore::new(
        database_path.clone(),
        SearchIndexConfig {
            roots: vec![home],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();

    core.configure_search_paths(
        0,
        SearchPathPreferences {
            custom_roots: vec![missing.clone()],
            exclusions: Vec::new(),
        },
    )
    .unwrap();
    core.start_index_maintenance().unwrap();
    assert!(matches!(
        core.current_status().path_configuration.roots[0].availability,
        SearchRootAvailability::Unavailable { .. }
    ));

    fs::create_dir(&missing).unwrap();
    fs::write(missing.join("later-root-note.txt"), "later").unwrap();
    assert!(matches!(
        core.current_status().path_configuration.roots[0].availability,
        SearchRootAvailability::Available
    ));
    wait_for_global_hit_count(&database_path, "later-root-note", 1);

    core.shutdown().unwrap();
}

#[test]
fn mount_observation_failure_reports_all_roots_unavailable() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("root");
    fs::create_dir(&root).unwrap();

    let statuses = root_statuses_from_mount_observation(
        std::slice::from_ref(&root),
        &[],
        Err(SearchError::InvalidConfiguration(
            "mountinfo unreadable".to_owned(),
        )),
    );

    assert!(matches!(
        statuses.as_slice(),
        [SearchRootStatus {
            availability: SearchRootAvailability::Unavailable { message },
            ..
        }] if message.contains("could not be verified")
    ));
}

#[test]
fn mount_replacement_hides_retains_recrawls_and_explicit_removal_purges() {
    if std::env::var_os("FILE_MANAGER_RUN_MOUNT_TESTS").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }

    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let mount_root = directory.path().join("mounted");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir(&mount_root).unwrap();
    run_mount_command(&[
        "-t",
        "tmpfs",
        "-o",
        "size=4m,nosuid,nodev,noexec",
        "tmpfs",
        mount_root.to_str().unwrap(),
    ]);
    fs::write(mount_root.join("first-volume-note.txt"), "first").unwrap();

    let database_path = directory.path().join("search.sqlite");
    let core = SearchDaemonCore::new(
        database_path.clone(),
        SearchIndexConfig {
            roots: vec![home],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();
    core.configure_search_paths(
        0,
        SearchPathPreferences {
            custom_roots: vec![mount_root.clone()],
            exclusions: Vec::new(),
        },
    )
    .unwrap();
    core.start_index_maintenance().unwrap();
    wait_for_global_hit_count(&database_path, "first-volume-note", 1);

    run_unmount_command(&mount_root);
    fs::write(
        mount_root.join("underlying-decoy.txt"),
        "must stay outside index",
    )
    .unwrap();
    core.reconcile_root_mounts().unwrap();
    wait_for_global_hit_count(&database_path, "first-volume-note", 0);
    wait_for_global_hit_count(&database_path, "underlying-decoy", 0);
    assert_eq!(
        stored_path_count(&database_path, &mount_root.join("first-volume-note.txt")),
        1
    );
    assert!(matches!(
        core.current_status().path_configuration.roots[0].availability,
        SearchRootAvailability::MountChanged { .. }
    ));

    run_mount_command(&[
        "-t",
        "tmpfs",
        "-o",
        "size=4m,nosuid,nodev,noexec",
        "tmpfs",
        mount_root.to_str().unwrap(),
    ]);
    fs::write(
        mount_root.join("replacement-volume-note.txt"),
        "replacement",
    )
    .unwrap();
    core.reconcile_root_mounts().unwrap();
    wait_for_global_hit_count(&database_path, "replacement-volume-note", 1);
    wait_for_global_hit_count(&database_path, "first-volume-note", 0);
    assert_eq!(
        stored_path_count(&database_path, &mount_root.join("first-volume-note.txt")),
        0
    );

    core.configure_search_paths(1, SearchPathPreferences::default())
        .unwrap();
    wait_for_global_hit_count(&database_path, "replacement-volume-note", 0);
    assert_eq!(
        stored_path_count(
            &database_path,
            &mount_root.join("replacement-volume-note.txt")
        ),
        0
    );
    core.shutdown().unwrap();
    run_unmount_command(&mount_root);
}

fn run_mount_command(arguments: &[&str]) {
    let output = Command::new("mount").args(arguments).output().unwrap();
    assert!(
        output.status.success(),
        "mount failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_unmount_command(path: &Path) {
    let output = Command::new("umount").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "unmount failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stored_path_count(database_path: &Path, path: &Path) -> i64 {
    Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            [crate::path_encoding::storage_bytes(path)],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn removing_an_exclusion_explicitly_recrawls_the_newly_included_frontier() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let private = home.join("private");
    let note = private.join("frontier-note.txt");
    fs::create_dir_all(&private).unwrap();
    fs::write(&note, "needle").unwrap();
    let database_path = directory.path().join("search.sqlite");
    let core = SearchDaemonCore::new(
        database_path.clone(),
        SearchIndexConfig {
            roots: vec![home.clone()],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();
    core.start_index_maintenance().unwrap();
    wait_for_global_hit_count(&database_path, "frontier-note", 1);

    core.configure_search_paths(
        0,
        SearchPathPreferences {
            custom_roots: Vec::new(),
            exclusions: vec![private.clone()],
        },
    )
    .unwrap();
    wait_for_global_hit_count(&database_path, "frontier-note", 0);

    core.configure_search_paths(1, SearchPathPreferences::default())
        .unwrap();
    wait_for_global_hit_count(&database_path, "frontier-note", 1);
    core.shutdown().unwrap();
}

fn wait_for_global_hit_count(database_path: &Path, terms: &str, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let database = SearchDatabase::open_read_only(database_path).unwrap();
        let mut query = SearchQuery::global(1, terms);
        query.limit = 10;
        let actual = database.search(&query).unwrap().hits.len();
        if actual == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "search hit count did not converge: expected {expected}, got {actual}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn public_status_keeps_visible_count_independent_of_phase() {
    let cases = [
        (DaemonLifecyclePhase::Starting, IndexPhase::Starting),
        (
            DaemonLifecyclePhase::Checking {
                checked_entries: 128,
                changed_entries: 3,
            },
            IndexPhase::Checking {
                checked_entries: 128,
                changed_entries: 3,
            },
        ),
        (
            DaemonLifecyclePhase::Crawling {
                scanned_entries: 9,
                current_scope: PathBuf::from("/tmp/root"),
            },
            IndexPhase::Crawling {
                scanned_entries: 9,
                current_scope: PathBuf::from("/tmp/root"),
            },
        ),
        (
            DaemonLifecyclePhase::Applying {
                pending_mutations: 4,
            },
            IndexPhase::Applying {
                pending_mutations: 4,
            },
        ),
        (DaemonLifecyclePhase::Complete, IndexPhase::Complete),
        (
            DaemonLifecyclePhase::Failed {
                message: "database unavailable".to_owned(),
            },
            IndexPhase::Failed {
                message: "database unavailable".to_owned(),
            },
        ),
    ];

    for (phase, expected) in cases {
        let status = snapshot_for(phase).to_index_status();
        assert_eq!(status.phase, expected);
        assert_eq!(status.visible_indexed_files, 41);
    }
}

#[test]
fn watch_degradation_does_not_replace_active_phase() {
    let mut snapshot = snapshot_for(DaemonLifecyclePhase::Checking {
        checked_entries: 7,
        changed_entries: 1,
    });
    snapshot.update_watch_coverage(WatchCoverageHealth::Incomplete {
        gap_count: 1,
        message: "one watch unavailable".to_owned(),
    });

    assert_eq!(
        snapshot.to_index_status(),
        IndexStatus {
            phase: IndexPhase::Checking {
                checked_entries: 7,
                changed_entries: 1,
            },
            visible_indexed_files: 41,
            health: IndexHealth::Degraded {
                message: "one watch unavailable".to_owned(),
            },
            capabilities: Vec::new(),
            path_configuration: Default::default(),
        }
    );
}

#[test]
fn hybrid_watch_budget_patrol_is_a_healthy_maintenance_mode() {
    let mut snapshot = snapshot_for(DaemonLifecyclePhase::Complete);
    snapshot.update_watch_coverage(WatchCoverageHealth::HybridPatrol { root_count: 1 });

    assert_eq!(snapshot.to_index_status().health, IndexHealth::Healthy);
}

#[test]
fn watch_event_paths_are_deduplicated_and_access_events_are_ignored() {
    let duplicate = PathBuf::from("/tmp/repeated");
    let changed = changed_paths_from_watch_event(
        Event::new(EventKind::Any)
            .add_path(duplicate.clone())
            .add_path(duplicate.clone()),
    )
    .unwrap();
    assert_eq!(changed, vec![duplicate]);

    let access = changed_paths_from_watch_event(
        Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path(PathBuf::from("/tmp/ignored")),
    )
    .unwrap();
    assert!(access.is_empty());
}

#[test]
fn ordinary_paths_arriving_during_startup_stay_local() {
    let root = PathBuf::from("/root");
    let changed = root.join("changed.txt");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![root]);
    pending.absorb_request(DaemonWorkRequest::StartupCheck, now);
    pending.absorb_request(
        DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![changed.clone()],
        },
        now,
    );

    assert_eq!(
        pending.take_next_work(now),
        Some(DaemonWorkRequest::StartupCheck)
    );
    assert_eq!(
        pending.take_next_work(now),
        Some(DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![changed],
        })
    );
    assert!(pending.dirty_roots.is_empty());
}

#[test]
fn pending_path_overflow_marks_one_root_dirty_after_the_quiet_window() {
    let first_root = PathBuf::from("/first");
    let second_root = PathBuf::from("/second");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![first_root.clone(), second_root]);
    pending.changed_watch_paths = BoundedPathSet::new(1, usize::MAX);
    pending.absorb_request(
        DaemonWorkRequest::ChangedPaths {
            changed_paths: vec![first_root.join("one"), first_root.join("two")],
        },
        now,
    );

    assert!(pending.take_next_work(now).is_none());
    assert_eq!(
        pending.take_next_work(now + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::DirtyRootRecovery {
            roots: vec![first_root],
        })
    );
}

#[test]
fn repeated_overflow_while_recovering_keeps_one_follow_up() {
    let root = PathBuf::from("/root");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![root.clone()]);
    pending.request_dirty_roots(vec![root.clone()], now);
    assert!(matches!(
        pending.take_next_work(now + DIRTY_ROOT_QUIET_WINDOW),
        Some(DaemonWorkRequest::DirtyRootRecovery { .. })
    ));

    pending.request_dirty_roots(vec![root.clone()], now + DIRTY_ROOT_QUIET_WINDOW);
    let first_finished_at = now + DIRTY_ROOT_QUIET_WINDOW;
    pending.finish_dirty_root_recovery(std::slice::from_ref(&root), true, first_finished_at);

    assert!(pending
        .take_next_work(first_finished_at + DIRTY_ROOT_RETRY_BASE - Duration::from_millis(1))
        .is_none());
    assert_eq!(
        pending.take_next_work(first_finished_at + DIRTY_ROOT_RETRY_BASE),
        Some(DaemonWorkRequest::DirtyRootRecovery { roots: vec![root] })
    );
}

#[test]
fn new_overflow_does_not_shorten_an_existing_recovery_backoff() {
    let root = PathBuf::from("/root");
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![root.clone()]);
    pending.request_dirty_roots(vec![root.clone()], now);
    let first_started_at = now + DIRTY_ROOT_QUIET_WINDOW;
    assert!(matches!(
        pending.take_next_work(first_started_at),
        Some(DaemonWorkRequest::DirtyRootRecovery { .. })
    ));
    pending.request_dirty_roots(vec![root.clone()], first_started_at);
    pending.finish_dirty_root_recovery(std::slice::from_ref(&root), true, first_started_at);

    pending.request_dirty_roots(
        vec![root.clone()],
        first_started_at + Duration::from_secs(1),
    );

    assert!(pending
        .take_next_work(first_started_at + DIRTY_ROOT_QUIET_WINDOW)
        .is_none());
    assert!(matches!(
        pending.take_next_work(first_started_at + DIRTY_ROOT_RETRY_BASE),
        Some(DaemonWorkRequest::DirtyRootRecovery { roots }) if roots == vec![root]
    ));
}

#[test]
fn failed_dirty_recovery_uses_capped_backoff() {
    assert_eq!(dirty_root_retry_delay(1), Duration::from_secs(30));
    assert_eq!(dirty_root_retry_delay(2), Duration::from_secs(60));
    assert_eq!(dirty_root_retry_delay(100), DIRTY_ROOT_RETRY_MAX);
}

#[test]
fn healthy_idle_queue_has_no_periodic_work_even_after_twenty_four_hours() {
    let now = Instant::now();
    let mut pending = PendingDaemonWork::new(vec![PathBuf::from("/root")]);

    assert!(pending
        .take_next_work(now + Duration::from_secs(24 * 60 * 60))
        .is_none());
    assert!(pending.next_deadline().is_none());
}

#[test]
fn watch_budget_patrol_queue_allows_only_one_pending_or_running_batch() {
    let root = PathBuf::from("/root");
    let first = root.join("first");
    let second = root.join("second");
    let queue = DaemonWorkQueue::new(vec![root]);

    assert!(queue
        .try_enqueue_watch_budget_patrol(std::slice::from_ref(&first))
        .unwrap());
    assert!(!queue
        .try_enqueue_watch_budget_patrol(std::slice::from_ref(&second))
        .unwrap());
    assert_eq!(
        queue.wait_for_next_work(),
        DaemonWorkRequest::WatchBudgetPatrol {
            directories: vec![first]
        }
    );
    assert!(!queue
        .try_enqueue_watch_budget_patrol(std::slice::from_ref(&second))
        .unwrap());

    queue.finish_watch_budget_patrol();

    assert!(!queue
        .try_enqueue_watch_budget_patrol(std::slice::from_ref(&second))
        .unwrap());
}

#[test]
fn concurrent_core_shutdown_joins_worker_once() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let core = Arc::new(
        SearchDaemonCore::new(
            database_path,
            SearchIndexConfig {
                roots: Vec::new(),
                ..SearchIndexConfig::default()
            },
        )
        .unwrap(),
    );

    let first = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || core.shutdown())
    };
    let second = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || core.shutdown())
    };

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
}

#[test]
fn daemon_rejects_configuration_outside_the_service_budget_before_opening_database() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    let invalid = SearchIndexConfig {
        roots: vec![PathBuf::from("/tmp"); crate::config::MAX_CONFIGURED_ROOTS + 1],
        ..SearchIndexConfig::default()
    };

    assert!(SearchDaemonCore::new(database_path.clone(), invalid).is_err());
    assert!(!database_path.exists());
}

#[test]
fn corruption_recovery_stays_degraded_until_the_rebuild_cycle_finishes() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    fs::write(&database_path, b"not a sqlite database").unwrap();
    let core = SearchDaemonCore::new(
        database_path,
        SearchIndexConfig {
            roots: Vec::new(),
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();

    assert!(matches!(
        core.current_status().health,
        IndexHealth::Degraded { ref message }
            if message.contains("SQLITE_NOTADB") && message.contains("quarantine")
    ));
    core.lifecycle_snapshot
        .lock()
        .unwrap()
        .finish_watch_cycle(0);
    assert_eq!(core.current_status().health, IndexHealth::Healthy);
    core.shutdown().unwrap();
}

/// 每次启动都执行 FTS optimize 会整库重写倒排索引（GB 级 IO 与内存峰值）；
/// 碎片由 FTS5 automerge 在写入时增量收敛，启动维护不得再做全量合并。
#[test]
fn startup_maintenance_leaves_full_text_compaction_to_automerge() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("search.sqlite");
    drop(SearchDatabase::open(&database_path).unwrap());
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO file_search_fts(file_search_fts, rank) VALUES('automerge', 0)",
            [],
        )
        .unwrap();
    for index in 0..4 {
        connection
            .execute(
                "INSERT INTO file_search_fts(rowid, name, content)
                 VALUES(?1, ?2, 'needle')",
                rusqlite::params![index + 1, format!("file-{index}")],
            )
            .unwrap();
    }
    let segment_count = || {
        connection
            .query_row(
                "SELECT COUNT(DISTINCT segid) FROM file_search_fts_idx",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
    };
    assert!(segment_count() > 1);

    let core = SearchDaemonCore::new(
        database_path,
        SearchIndexConfig {
            roots: Vec::new(),
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();
    assert!(segment_count() > 1);

    core.start_index_maintenance().unwrap();
    let completion_deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(core.current_status().phase, IndexPhase::Complete) {
        assert!(
            Instant::now() < completion_deadline,
            "startup maintenance did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // 维护结束后仍保持多段：启动路径未触发 FTS optimize 整库合并。
    assert!(segment_count() > 1);
    core.shutdown().unwrap();
}
