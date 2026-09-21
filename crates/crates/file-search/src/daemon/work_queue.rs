use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::SearchResult;

use super::bounded_paths::BoundedPathSet;
use super::daemon_core_stopped;

pub(super) const DIRTY_ROOT_QUIET_WINDOW: Duration = Duration::from_secs(2);
pub(super) const DIRTY_ROOT_RETRY_BASE: Duration = Duration::from_secs(30);
pub(super) const DIRTY_ROOT_RETRY_MAX: Duration = Duration::from_secs(15 * 60);
pub(super) const WATCH_BUDGET_PATROL_INTERVAL: Duration = Duration::from_secs(5);
const MAX_PENDING_LOCAL_PATHS: usize = 2_048;
const MAX_PENDING_LOCAL_PATH_BYTES: usize = 1_048_576;
const MAX_PENDING_COVERAGE_SCOPES: usize = 256;
const MAX_PENDING_COVERAGE_SCOPE_BYTES: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DaemonWorkRequest {
    StartupCheck,
    ChangedPaths { changed_paths: Vec<PathBuf> },
    CoverageRepair { scopes: Vec<PathBuf> },
    WatchBudgetPatrol { directories: Vec<PathBuf> },
    DirtyRootRecovery { roots: Vec<PathBuf> },
    Shutdown,
}

#[derive(Debug, Clone)]
pub(super) struct DirtyRootRecoveryState {
    ready_at: Instant,
    retry_attempts: u32,
    running: bool,
    dirtied_while_running: bool,
}

#[derive(Debug)]
pub(super) struct PendingDaemonWork {
    roots: Vec<PathBuf>,
    startup_check_requested: bool,
    pub(super) changed_watch_paths: BoundedPathSet,
    coverage_repair_scopes: BoundedPathSet,
    watch_budget_patrol_directories: Option<Vec<PathBuf>>,
    pub(super) dirty_roots: BTreeMap<PathBuf, DirtyRootRecoveryState>,
    shutdown_requested: bool,
}

impl PendingDaemonWork {
    pub(super) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            startup_check_requested: false,
            changed_watch_paths: BoundedPathSet::new(
                MAX_PENDING_LOCAL_PATHS,
                MAX_PENDING_LOCAL_PATH_BYTES,
            ),
            coverage_repair_scopes: BoundedPathSet::new(
                MAX_PENDING_COVERAGE_SCOPES,
                MAX_PENDING_COVERAGE_SCOPE_BYTES,
            ),
            watch_budget_patrol_directories: None,
            dirty_roots: BTreeMap::new(),
            shutdown_requested: false,
        }
    }

    pub(super) fn absorb_request(&mut self, work_request: DaemonWorkRequest, now: Instant) {
        match work_request {
            DaemonWorkRequest::StartupCheck => self.startup_check_requested = true,
            DaemonWorkRequest::ChangedPaths { changed_paths } => {
                self.absorb_changed_paths(changed_paths, now)
            }
            DaemonWorkRequest::CoverageRepair { scopes } => {
                self.absorb_coverage_repairs(scopes, now)
            }
            DaemonWorkRequest::WatchBudgetPatrol { directories } => {
                debug_assert!(self.watch_budget_patrol_directories.is_none());
                self.watch_budget_patrol_directories = Some(directories);
            }
            DaemonWorkRequest::DirtyRootRecovery { roots } => self.request_dirty_roots(roots, now),
            DaemonWorkRequest::Shutdown => self.shutdown_requested = true,
        }
    }

    fn absorb_changed_paths(&mut self, paths: Vec<PathBuf>, now: Instant) {
        self.absorb_bounded_paths(paths, PendingPathClass::Changed, now);
    }

    fn absorb_coverage_repairs(&mut self, scopes: Vec<PathBuf>, now: Instant) {
        self.absorb_bounded_paths(scopes, PendingPathClass::CoverageRepair, now);
    }

    fn absorb_bounded_paths(
        &mut self,
        paths: Vec<PathBuf>,
        path_class: PendingPathClass,
        now: Instant,
    ) {
        for path in paths {
            let insertion = match path_class {
                PendingPathClass::Changed => self.changed_watch_paths.insert(path.clone()),
                PendingPathClass::CoverageRepair => {
                    self.coverage_repair_scopes.insert(path.clone())
                }
            };
            if insertion.is_ok() {
                continue;
            }
            let roots = self.containing_roots(&path);
            for root in &roots {
                self.changed_watch_paths
                    .retain(|pending| !pending.starts_with(root));
                self.coverage_repair_scopes
                    .retain(|pending| !pending.starts_with(root));
            }
            self.request_dirty_roots(roots, now);
        }
    }

    pub(super) fn request_dirty_roots(&mut self, roots: Vec<PathBuf>, now: Instant) {
        for root in roots {
            let Some(state) = self.dirty_roots.get_mut(&root) else {
                self.dirty_roots.insert(
                    root,
                    DirtyRootRecoveryState {
                        ready_at: now + DIRTY_ROOT_QUIET_WINDOW,
                        retry_attempts: 0,
                        running: false,
                        dirtied_while_running: false,
                    },
                );
                continue;
            };
            if state.running {
                state.dirtied_while_running = true;
            } else {
                state.ready_at = state.ready_at.max(now + DIRTY_ROOT_QUIET_WINDOW);
            }
        }
    }

    pub(super) fn take_next_work(&mut self, now: Instant) -> Option<DaemonWorkRequest> {
        if self.shutdown_requested {
            self.shutdown_requested = false;
            return Some(DaemonWorkRequest::Shutdown);
        }
        if self.startup_check_requested {
            self.startup_check_requested = false;
            return Some(DaemonWorkRequest::StartupCheck);
        }

        let changed_paths = self.changed_watch_paths.take_paths();
        if !changed_paths.is_empty() {
            return Some(DaemonWorkRequest::ChangedPaths { changed_paths });
        }

        let scopes = self.coverage_repair_scopes.take_paths();
        if !scopes.is_empty() {
            return Some(DaemonWorkRequest::CoverageRepair { scopes });
        }

        let ready_roots = self
            .dirty_roots
            .iter_mut()
            .filter_map(|(root, state)| {
                (!state.running && state.ready_at <= now).then(|| {
                    state.running = true;
                    state.dirtied_while_running = false;
                    root.clone()
                })
            })
            .collect::<Vec<_>>();
        if !ready_roots.is_empty() {
            return Some(DaemonWorkRequest::DirtyRootRecovery { roots: ready_roots });
        }

        self.watch_budget_patrol_directories
            .take()
            .map(|directories| DaemonWorkRequest::WatchBudgetPatrol { directories })
    }

    pub(super) fn finish_dirty_root_recovery(
        &mut self,
        roots: &[PathBuf],
        succeeded: bool,
        now: Instant,
    ) {
        for root in roots {
            let Some(state) = self.dirty_roots.get_mut(root) else {
                continue;
            };
            if succeeded && !state.dirtied_while_running {
                self.dirty_roots.remove(root);
                continue;
            }
            state.running = false;
            state.retry_attempts = state.retry_attempts.saturating_add(1);
            state.ready_at = now + dirty_root_retry_delay(state.retry_attempts);
        }
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.dirty_roots
            .values()
            .filter(|state| !state.running)
            .map(|state| state.ready_at)
            .min()
    }

    fn containing_roots(&self, path: &Path) -> Vec<PathBuf> {
        let roots = self
            .roots
            .iter()
            .filter(|root| path.starts_with(root))
            .cloned()
            .collect::<Vec<_>>();
        if roots.is_empty() {
            self.roots.clone()
        } else {
            roots
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PendingPathClass {
    Changed,
    CoverageRepair,
}

pub(super) fn dirty_root_retry_delay(retry_attempts: u32) -> Duration {
    let multiplier = 1_u32
        .checked_shl(retry_attempts.saturating_sub(1).min(5))
        .unwrap_or(32);
    DIRTY_ROOT_RETRY_BASE
        .checked_mul(multiplier)
        .unwrap_or(DIRTY_ROOT_RETRY_MAX)
        .min(DIRTY_ROOT_RETRY_MAX)
}

#[derive(Debug)]
struct DaemonWorkQueueState {
    accepting_new_work: bool,
    watch_budget_patrol_running: bool,
    watch_budget_patrol_not_before: Option<Instant>,
    pending_work: PendingDaemonWork,
}

#[derive(Debug)]
pub(super) struct DaemonWorkQueue {
    state: Mutex<DaemonWorkQueueState>,
    ready_signal: Condvar,
}

impl DaemonWorkQueue {
    pub(super) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            state: Mutex::new(DaemonWorkQueueState {
                accepting_new_work: true,
                watch_budget_patrol_running: false,
                watch_budget_patrol_not_before: None,
                pending_work: PendingDaemonWork::new(roots),
            }),
            ready_signal: Condvar::new(),
        }
    }

    pub(super) fn enqueue(&self, work_request: DaemonWorkRequest) -> SearchResult<()> {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        if !state.accepting_new_work {
            return Err(daemon_core_stopped());
        }
        state
            .pending_work
            .absorb_request(work_request, Instant::now());
        self.ready_signal.notify_one();
        Ok(())
    }

    pub(super) fn try_enqueue_watch_budget_patrol(
        &self,
        directories: &[PathBuf],
    ) -> SearchResult<bool> {
        self.try_enqueue_watch_budget_patrol_at(directories, Instant::now())
    }

    fn try_enqueue_watch_budget_patrol_at(
        &self,
        directories: &[PathBuf],
        now: Instant,
    ) -> SearchResult<bool> {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        if !state.accepting_new_work {
            return Err(daemon_core_stopped());
        }
        if state.watch_budget_patrol_running
            || state.pending_work.watch_budget_patrol_directories.is_some()
            || state
                .watch_budget_patrol_not_before
                .is_some_and(|not_before| now < not_before)
        {
            return Ok(false);
        }
        state.pending_work.absorb_request(
            DaemonWorkRequest::WatchBudgetPatrol {
                directories: directories.to_vec(),
            },
            now,
        );
        self.ready_signal.notify_one();
        Ok(true)
    }

    pub(super) fn begin_shutdown(&self) {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        state.accepting_new_work = false;
        state.pending_work.shutdown_requested = true;
        self.ready_signal.notify_one();
    }

    pub(super) fn finish_watch_budget_patrol(&self) {
        self.finish_watch_budget_patrol_at(Instant::now());
    }

    fn finish_watch_budget_patrol_at(&self, now: Instant) {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        state.watch_budget_patrol_running = false;
        state.watch_budget_patrol_not_before = Some(now + WATCH_BUDGET_PATROL_INTERVAL);
        self.ready_signal.notify_one();
    }

    pub(super) fn finish_dirty_root_recovery(&self, roots: &[PathBuf], succeeded: bool) {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        state
            .pending_work
            .finish_dirty_root_recovery(roots, succeeded, Instant::now());
        self.ready_signal.notify_one();
    }

    pub(super) fn wait_for_next_work(&self) -> DaemonWorkRequest {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        loop {
            if let Some(work_request) = state.pending_work.take_next_work(Instant::now()) {
                if matches!(work_request, DaemonWorkRequest::WatchBudgetPatrol { .. }) {
                    state.watch_budget_patrol_running = true;
                }
                return work_request;
            }
            match state.pending_work.next_deadline() {
                Some(deadline) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    let (next_state, _) = self
                        .ready_signal
                        .wait_timeout(state, wait)
                        .expect("search daemon work queue mutex poisoned");
                    state = next_state;
                }
                None => {
                    state = self
                        .ready_signal
                        .wait(state)
                        .expect("search daemon work queue mutex poisoned");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
        queue.finish_watch_budget_patrol_at(started_at);

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
}
