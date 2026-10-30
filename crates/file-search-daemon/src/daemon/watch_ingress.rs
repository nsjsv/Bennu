use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind};

use crate::error::{SearchError, SearchResult};
use crate::SearchIndexConfig;

use super::bounded_paths::{estimated_path_bytes, BoundedPathSet};
use super::memory::release_allocator_idle_pages;
use super::watch_coverage::{RecommendedWatchCoverage, WatchCoverageRefresh};
use super::work_queue::WATCH_BUDGET_PATROL_INTERVAL;
use super::{DaemonLifecycleSnapshot, DaemonWorkQueue, DaemonWorkRequest};

const WATCH_EVENT_COALESCE_WINDOW: Duration = Duration::from_millis(250);
const WATCH_EVENT_COALESCE_DEADLINE: Duration = Duration::from_secs(1);
const WATCH_COVERAGE_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const WATCH_EVENT_CHANNEL_CAPACITY: usize = 32;
pub(super) const MAX_EVENT_PATHS: usize = 64;
const MAX_EVENT_PATH_BYTES: usize = 65_536;
pub(super) const MAX_COALESCED_PATHS: usize = 2_048;
const MAX_COALESCED_PATH_BYTES: usize = 1_048_576;

pub(super) struct WatchOverflowState {
    roots: Vec<PathBuf>,
    epoch: AtomicU64,
    all_roots_dirty: AtomicBool,
    dirty_roots: Mutex<BTreeSet<PathBuf>>,
}

impl WatchOverflowState {
    fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            epoch: AtomicU64::new(0),
            all_roots_dirty: AtomicBool::new(false),
            dirty_roots: Mutex::new(BTreeSet::new()),
        }
    }

    fn record_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            self.record_all_roots();
            return;
        }
        let Ok(mut dirty_roots) = self.dirty_roots.try_lock() else {
            self.record_all_roots();
            return;
        };
        for path in paths {
            let Some(root) = self
                .roots
                .iter()
                .filter(|root| path == root.as_path() || path.starts_with(root))
                .max_by_key(|root| root.components().count())
            else {
                self.all_roots_dirty.store(true, Ordering::Release);
                continue;
            };
            dirty_roots.insert(root.clone());
        }
        self.advance_epoch();
    }

    fn record_all_roots(&self) {
        self.all_roots_dirty.store(true, Ordering::Release);
        self.advance_epoch();
    }

    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    fn take_dirty_roots(&self) -> Vec<PathBuf> {
        if self.all_roots_dirty.swap(false, Ordering::AcqRel) {
            if let Ok(mut dirty_roots) = self.dirty_roots.lock() {
                dirty_roots.clear();
            }
            return self.roots.clone();
        }
        self.dirty_roots
            .lock()
            .expect("watch overflow roots mutex poisoned")
            .split_off(&PathBuf::new())
            .into_iter()
            .collect()
    }

    fn advance_epoch(&self) {
        let _ = self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                Some(epoch.saturating_add(1))
            });
    }
}

pub(super) struct DaemonWatchIngressBootstrap {
    coverage: RecommendedWatchCoverage,
    event_receiver: Receiver<notify::Result<Event>>,
    overflow_state: Arc<WatchOverflowState>,
    directory_snapshot_epoch: Arc<AtomicU64>,
    initial_refresh: WatchCoverageRefresh,
}

pub(super) struct DaemonWatchIngress {
    stop_sender: Sender<()>,
    dispatch_thread: thread::JoinHandle<()>,
}

#[derive(Debug)]
pub(super) struct PendingWatchPathBatch {
    changed_paths: BoundedPathSet,
}

struct WatchBudgetPatrolSchedule {
    next_submission: Instant,
    deferred_directories: Option<Vec<PathBuf>>,
}

impl WatchBudgetPatrolSchedule {
    fn new(now: Instant) -> Self {
        Self {
            next_submission: now + WATCH_BUDGET_PATROL_INTERVAL,
            deferred_directories: None,
        }
    }

    fn submit_due(
        &mut self,
        coverage: &mut RecommendedWatchCoverage,
        work_queue: &DaemonWorkQueue,
        now: Instant,
    ) -> SearchResult<()> {
        if self.deferred_directories.is_none() && now < self.next_submission {
            return Ok(());
        }
        if self.deferred_directories.is_none() {
            let directories = coverage.next_watch_budget_patrol()?;
            if directories.is_empty() {
                self.next_submission = now + WATCH_BUDGET_PATROL_INTERVAL;
                return Ok(());
            }
            self.deferred_directories = Some(directories);
        }

        let directories = self
            .deferred_directories
            .as_deref()
            .expect("watch budget patrol directories missing");
        if work_queue.try_enqueue_watch_budget_patrol(directories)? {
            self.deferred_directories = None;
            self.next_submission = now + WATCH_BUDGET_PATROL_INTERVAL;
        }
        Ok(())
    }
}

impl Default for PendingWatchPathBatch {
    fn default() -> Self {
        Self {
            changed_paths: BoundedPathSet::new(MAX_COALESCED_PATHS, MAX_COALESCED_PATH_BYTES),
        }
    }
}

impl PendingWatchPathBatch {
    pub(super) fn absorb_event(&mut self, event: Event) -> Result<(), Vec<PathBuf>> {
        let changed_paths = changed_paths_from_watch_event(event)?;
        for changed_path in &changed_paths {
            if self.changed_paths.insert(changed_path.clone()).is_err() {
                self.changed_paths.clear();
                return Err(changed_paths);
            }
        }
        Ok(())
    }

    pub(super) fn into_changed_paths(mut self) -> Vec<PathBuf> {
        self.changed_paths.take_paths()
    }
}

impl DaemonWatchIngressBootstrap {
    pub(super) fn establish(
        config: SearchIndexConfig,
        database_path: PathBuf,
        directory_snapshot_epoch: Arc<AtomicU64>,
    ) -> SearchResult<Self> {
        let (event_sender, event_receiver) = mpsc::sync_channel(WATCH_EVENT_CHANNEL_CAPACITY);
        let overflow_state = Arc::new(WatchOverflowState::new(
            config.available_roots().cloned().collect(),
        ));
        let mut coverage = RecommendedWatchCoverage::create(
            config,
            database_path,
            event_sender,
            Arc::clone(&overflow_state),
        )?;
        let initial_refresh = coverage.initialize()?;
        release_allocator_idle_pages();

        Ok(Self {
            coverage,
            event_receiver,
            overflow_state,
            directory_snapshot_epoch,
            initial_refresh,
        })
    }

    pub(super) fn spawn(
        self,
        work_queue: Arc<DaemonWorkQueue>,
        lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    ) -> SearchResult<DaemonWatchIngress> {
        let (stop_sender, stop_receiver) = mpsc::channel();
        let Self {
            coverage,
            event_receiver,
            overflow_state,
            directory_snapshot_epoch,
            initial_refresh,
        } = self;
        let dispatch_thread = thread::Builder::new()
            .name("file-search-watch-ingress".to_owned())
            .spawn(move || {
                WatchIngressLoop {
                    coverage,
                    initial_refresh,
                    event_receiver,
                    overflow_state,
                    directory_snapshot_epoch,
                    stop_receiver,
                    work_queue,
                    lifecycle_snapshot,
                }
                .run()
            })
            .map_err(|error| {
                SearchError::WorkerFailed(format!("could not spawn watch ingress: {error}"))
            })?;

        Ok(DaemonWatchIngress {
            stop_sender,
            dispatch_thread,
        })
    }
}

impl DaemonWatchIngress {
    pub(super) fn shutdown(self) {
        let _ = self.stop_sender.send(());
        let _ = self.dispatch_thread.join();
    }
}

struct WatchIngressLoop {
    coverage: RecommendedWatchCoverage,
    initial_refresh: WatchCoverageRefresh,
    event_receiver: Receiver<notify::Result<Event>>,
    overflow_state: Arc<WatchOverflowState>,
    directory_snapshot_epoch: Arc<AtomicU64>,
    stop_receiver: Receiver<()>,
    work_queue: Arc<DaemonWorkQueue>,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
}

impl WatchIngressLoop {
    fn run(self) {
        let Self {
            mut coverage,
            initial_refresh,
            event_receiver,
            overflow_state,
            directory_snapshot_epoch,
            stop_receiver,
            work_queue,
            lifecycle_snapshot,
        } = self;
        if publish_watch_coverage_refresh(
            &work_queue,
            &lifecycle_snapshot,
            Vec::new(),
            initial_refresh,
        )
        .is_err()
        {
            return;
        }
        let mut next_coverage_retry = Instant::now() + WATCH_COVERAGE_RETRY_INTERVAL;
        let mut budget_patrol_schedule = WatchBudgetPatrolSchedule::new(Instant::now());
        let mut observed_overflow_epoch = overflow_state.epoch();
        let mut observed_snapshot_epoch = directory_snapshot_epoch.load(Ordering::Acquire);

        loop {
            if ingress_should_stop(&stop_receiver) {
                break;
            }
            if let Err(error) =
                budget_patrol_schedule.submit_due(&mut coverage, &work_queue, Instant::now())
            {
                coverage.mark_backend_unavailable(error.to_string());
                record_watch_channel_failure(&mut coverage, &lifecycle_snapshot);
                break;
            }

            let current_snapshot_epoch = directory_snapshot_epoch.load(Ordering::Acquire);
            if current_snapshot_epoch != observed_snapshot_epoch {
                observed_snapshot_epoch = current_snapshot_epoch;
                match coverage.restore_snapshot_directories() {
                    Ok(refresh) => {
                        release_allocator_idle_pages();
                        if publish_watch_coverage_refresh(
                            &work_queue,
                            &lifecycle_snapshot,
                            Vec::new(),
                            refresh,
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        coverage.mark_backend_unavailable(error.to_string());
                        record_watch_channel_failure(&mut coverage, &lifecycle_snapshot);
                    }
                }
            }

            let current_overflow_epoch = overflow_state.epoch();
            if current_overflow_epoch != observed_overflow_epoch {
                observed_overflow_epoch = current_overflow_epoch;
                let dirty_roots = overflow_state.take_dirty_roots();
                if !dirty_roots.is_empty()
                    && work_queue
                        .enqueue(DaemonWorkRequest::DirtyRootRecovery { roots: dirty_roots })
                        .is_err()
                {
                    break;
                }
                continue;
            }

            let mut pending_watch_batch = PendingWatchPathBatch::default();
            match event_receiver.recv_timeout(WATCH_EVENT_COALESCE_WINDOW) {
                Ok(Ok(event)) => {
                    if let Err(paths) = pending_watch_batch.absorb_event(event) {
                        overflow_state.record_paths(&paths);
                        continue;
                    }
                }
                Ok(Err(error)) => {
                    overflow_state.record_paths(&error.paths);
                    if handle_watch_error(&mut coverage, &error, &work_queue, &lifecycle_snapshot)
                        .is_err()
                    {
                        break;
                    }
                    tracing::warn!(
                        target: "file_search::watch",
                        event = "watch_ingress_error",
                        error = ?error.kind,
                        path_count = error.paths.len(),
                        "filesystem watch ingress degraded"
                    );
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= next_coverage_retry {
                        if publish_watch_coverage_refresh(
                            &work_queue,
                            &lifecycle_snapshot,
                            Vec::new(),
                            coverage.retry_gaps(),
                        )
                        .is_err()
                        {
                            break;
                        }
                        next_coverage_retry = Instant::now() + WATCH_COVERAGE_RETRY_INTERVAL;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    record_watch_channel_failure(&mut coverage, &lifecycle_snapshot);
                    break;
                }
            }

            let coalesce_deadline = Instant::now() + WATCH_EVENT_COALESCE_DEADLINE;
            let mut coalesced_paths_overflowed = false;
            loop {
                if ingress_should_stop(&stop_receiver) {
                    return;
                }
                let remaining = coalesce_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match event_receiver.recv_timeout(WATCH_EVENT_COALESCE_WINDOW.min(remaining)) {
                    Ok(Ok(event)) => {
                        if let Err(paths) = pending_watch_batch.absorb_event(event) {
                            overflow_state.record_paths(&paths);
                            coalesced_paths_overflowed = true;
                            break;
                        }
                    }
                    Ok(Err(error)) => {
                        overflow_state.record_paths(&error.paths);
                        if handle_watch_error(
                            &mut coverage,
                            &error,
                            &work_queue,
                            &lifecycle_snapshot,
                        )
                        .is_err()
                        {
                            return;
                        }
                        tracing::warn!(
                            target: "file_search::watch",
                            event = "watch_ingress_error",
                            error = ?error.kind,
                            path_count = error.paths.len(),
                            "filesystem watch ingress degraded"
                        );
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        record_watch_channel_failure(&mut coverage, &lifecycle_snapshot);
                        return;
                    }
                }
            }

            if coalesced_paths_overflowed || overflow_state.epoch() != observed_overflow_epoch {
                continue;
            }

            let changed_paths = pending_watch_batch.into_changed_paths();
            if changed_paths.is_empty() {
                continue;
            }
            let refresh = coverage.refresh_changed_paths(&changed_paths);
            if publish_watch_coverage_refresh(
                &work_queue,
                &lifecycle_snapshot,
                changed_paths,
                refresh,
            )
            .is_err()
            {
                break;
            }

            if Instant::now() >= next_coverage_retry {
                if publish_watch_coverage_refresh(
                    &work_queue,
                    &lifecycle_snapshot,
                    Vec::new(),
                    coverage.retry_gaps(),
                )
                .is_err()
                {
                    break;
                }
                next_coverage_retry = Instant::now() + WATCH_COVERAGE_RETRY_INTERVAL;
            }
        }
    }
}

fn handle_watch_error(
    coverage: &mut RecommendedWatchCoverage,
    error: &notify::Error,
    work_queue: &DaemonWorkQueue,
    lifecycle_snapshot: &Mutex<DaemonLifecycleSnapshot>,
) -> SearchResult<()> {
    coverage.record_notify_error(error);
    publish_watch_coverage_refresh(
        work_queue,
        lifecycle_snapshot,
        Vec::new(),
        WatchCoverageRefresh {
            repair_paths: Vec::new(),
            patrol_paths: error.paths.clone(),
            health: coverage.health(),
        },
    )
}

fn publish_watch_coverage_refresh(
    work_queue: &DaemonWorkQueue,
    lifecycle_snapshot: &Mutex<DaemonLifecycleSnapshot>,
    changed_paths: Vec<PathBuf>,
    refresh: WatchCoverageRefresh,
) -> SearchResult<()> {
    let mut local_paths = changed_paths;
    local_paths.extend(refresh.repair_paths);
    if !local_paths.is_empty() {
        work_queue.enqueue(DaemonWorkRequest::ChangedPaths {
            changed_paths: local_paths,
        })?;
    }
    if !refresh.patrol_paths.is_empty() {
        work_queue.enqueue(DaemonWorkRequest::CoverageRepair {
            scopes: refresh.patrol_paths,
        })?;
    }

    lifecycle_snapshot
        .lock()
        .expect("search daemon lifecycle mutex poisoned")
        .update_watch_coverage(refresh.health);
    Ok(())
}

fn record_watch_channel_failure(
    coverage: &mut RecommendedWatchCoverage,
    lifecycle_snapshot: &Mutex<DaemonLifecycleSnapshot>,
) {
    coverage.mark_backend_unavailable("watch event channel disconnected");
    lifecycle_snapshot
        .lock()
        .expect("search daemon lifecycle mutex poisoned")
        .update_watch_coverage(coverage.health());
}

fn ingress_should_stop(stop_receiver: &Receiver<()>) -> bool {
    matches!(
        stop_receiver.try_recv(),
        Ok(()) | Err(TryRecvError::Disconnected)
    )
}

pub(super) fn changed_paths_from_watch_event(event: Event) -> Result<Vec<PathBuf>, Vec<PathBuf>> {
    if matches!(event.kind, EventKind::Access(_)) {
        return Ok(Vec::new());
    }
    if event.paths.is_empty() {
        return Err(Vec::new());
    }

    let mut changed_paths = BoundedPathSet::new(MAX_EVENT_PATHS, MAX_EVENT_PATH_BYTES);
    let original_paths = event.paths;
    for path in &original_paths {
        if changed_paths.insert(path.clone()).is_err() {
            return Err(original_paths);
        }
    }
    Ok(changed_paths.take_paths())
}

pub(super) fn deliver_watch_event(
    event_sender: &SyncSender<notify::Result<Event>>,
    overflow_state: &WatchOverflowState,
    event_result: notify::Result<Event>,
) {
    if let Ok(event) = &event_result {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
    }
    let paths = event_paths(&event_result);
    let path_bytes = paths
        .iter()
        .map(|path| estimated_path_bytes(path))
        .try_fold(0_usize, usize::checked_add);
    if paths.is_empty()
        || paths.len() > MAX_EVENT_PATHS
        || path_bytes.is_none_or(|bytes| bytes > MAX_EVENT_PATH_BYTES)
    {
        overflow_state.record_paths(paths);
        return;
    }

    match event_sender.try_send(event_result) {
        Ok(()) => {}
        Err(TrySendError::Full(event_result)) => {
            overflow_state.record_paths(event_paths(&event_result));
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn event_paths(event_result: &notify::Result<Event>) -> &[PathBuf] {
    match event_result {
        Ok(event) => &event.paths,
        Err(error) => &error.paths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_tracks_only_the_affected_root() {
        let first = PathBuf::from("/first");
        let second = PathBuf::from("/second");
        let state = WatchOverflowState::new(vec![first.clone(), second]);

        state.record_paths(&[first.join("changed.txt")]);

        assert_eq!(state.take_dirty_roots(), vec![first]);
    }

    #[test]
    fn coalesced_capacity_failure_reports_paths_for_dirty_root_recovery() {
        let mut batch = PendingWatchPathBatch {
            changed_paths: BoundedPathSet::new(1, usize::MAX),
        };
        batch
            .absorb_event(Event::new(EventKind::Any).add_path(PathBuf::from("/root/first")))
            .unwrap();
        let overflowed = batch
            .absorb_event(Event::new(EventKind::Any).add_path(PathBuf::from("/root/second")))
            .unwrap_err();

        assert_eq!(overflowed, vec![PathBuf::from("/root/second")]);
        assert!(batch.into_changed_paths().is_empty());
    }
}
