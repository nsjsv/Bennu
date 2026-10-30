use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::SearchResult;

use super::bounded_paths::BoundedPathSet;
use super::daemon_core_stopped;
use super::lifecycle::IndexCompleteness;

pub(super) const DIRTY_ROOT_QUIET_WINDOW: Duration = Duration::from_secs(2);
pub(super) const DIRTY_ROOT_RETRY_BASE: Duration = Duration::from_secs(30);
pub(super) const DIRTY_ROOT_RETRY_MAX: Duration = Duration::from_secs(30 * 60);
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

/// 一件维护工作的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkOutcome {
    Succeeded,
    Failed {
        message: String,
    },
    /// 被重启或关机打断，没有结论。
    Cancelled,
}

#[derive(Debug, Clone)]
pub(super) struct DirtyRootRecoveryState {
    ready_at: Instant,
    retry_attempts: u32,
    running: bool,
    dirtied_while_running: bool,
    /// 这个位置最近一次检查失败的原因；有它就说明索引对这个位置不完整，直到重查成功。
    failure: Option<String>,
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
                        failure: None,
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
        outcome: &WorkOutcome,
        now: Instant,
    ) {
        for root in roots {
            let Some(state) = self.dirty_roots.get_mut(root) else {
                continue;
            };
            state.running = false;
            match outcome {
                WorkOutcome::Succeeded if !state.dirtied_while_running => {
                    self.dirty_roots.remove(root);
                    continue;
                }
                WorkOutcome::Succeeded => state.failure = None,
                WorkOutcome::Failed { message } => state.failure = Some(message.clone()),
                WorkOutcome::Cancelled => continue,
            }
            state.retry_attempts = state.retry_attempts.saturating_add(1);
            state.ready_at = now + dirty_root_retry_delay(state.retry_attempts);
        }
    }

    /// 一件非重查的工作失败：它涉及的位置排进退避重查，并记下原因。
    pub(super) fn record_failed_roots(&mut self, roots: Vec<PathBuf>, message: &str, now: Instant) {
        for root in roots {
            let state = self
                .dirty_roots
                .entry(root)
                .or_insert(DirtyRootRecoveryState {
                    ready_at: now,
                    retry_attempts: 0,
                    running: false,
                    dirtied_while_running: false,
                    failure: None,
                });
            state.retry_attempts = state.retry_attempts.saturating_add(1);
            state.ready_at = now + dirty_root_retry_delay(state.retry_attempts);
            state.failure = Some(message.to_owned());
        }
    }

    pub(super) fn index_completeness(&self) -> IndexCompleteness {
        match self
            .dirty_roots
            .values()
            .find_map(|state| state.failure.as_ref())
        {
            Some(message) => IndexCompleteness::Incomplete {
                message: message.clone(),
            },
            None => IndexCompleteness::Complete,
        }
    }

    fn roots_containing(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for path in paths {
            for root in self.containing_roots(path) {
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        roots
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

/// 30 秒起，每次翻倍，封顶 30 分钟（30 秒 × 2⁶ 已超过上限）。
pub(super) fn dirty_root_retry_delay(retry_attempts: u32) -> Duration {
    let multiplier = 1_u32 << retry_attempts.saturating_sub(1).min(6);
    DIRTY_ROOT_RETRY_BASE
        .saturating_mul(multiplier)
        .min(DIRTY_ROOT_RETRY_MAX)
}

/// worker 正在做的那件工作，结束时由 `finish_work` 交回，决定失败要记到哪些位置上。
#[derive(Debug)]
enum InFlightWork {
    FullCheck,
    Local { roots: Vec<PathBuf> },
    WatchBudgetPatrol { roots: Vec<PathBuf> },
    DirtyRootRecovery { roots: Vec<PathBuf> },
}

impl InFlightWork {
    fn of(work_request: &DaemonWorkRequest, pending_work: &PendingDaemonWork) -> Option<Self> {
        Some(match work_request {
            DaemonWorkRequest::StartupCheck => Self::FullCheck,
            DaemonWorkRequest::ChangedPaths {
                changed_paths: paths,
            }
            | DaemonWorkRequest::CoverageRepair { scopes: paths } => Self::Local {
                roots: pending_work.roots_containing(paths),
            },
            DaemonWorkRequest::WatchBudgetPatrol { directories } => Self::WatchBudgetPatrol {
                roots: pending_work.roots_containing(directories),
            },
            DaemonWorkRequest::DirtyRootRecovery { roots } => Self::DirtyRootRecovery {
                roots: roots.clone(),
            },
            DaemonWorkRequest::Shutdown => return None,
        })
    }
}

#[derive(Debug)]
struct DaemonWorkQueueState {
    accepting_new_work: bool,
    in_flight: Option<InFlightWork>,
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
                in_flight: None,
                watch_budget_patrol_not_before: None,
                pending_work: PendingDaemonWork::new(roots),
            }),
            ready_signal: Condvar::new(),
        }
    }

    /// 每轮维护的队列：全量检查排第一，新纳入的前沿目录紧随其后。
    pub(super) fn starting_with_full_check(
        roots: Vec<PathBuf>,
        newly_included_frontiers: Vec<PathBuf>,
    ) -> Self {
        let mut queue = Self::new(roots);
        let pending_work = &mut queue
            .state
            .get_mut()
            .expect("search daemon work queue mutex poisoned")
            .pending_work;
        let now = Instant::now();
        pending_work.absorb_request(DaemonWorkRequest::StartupCheck, now);
        if !newly_included_frontiers.is_empty() {
            pending_work.absorb_request(
                DaemonWorkRequest::ChangedPaths {
                    changed_paths: newly_included_frontiers,
                },
                now,
            );
        }
        queue
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
        let patrol_running = matches!(
            state.in_flight,
            Some(InFlightWork::WatchBudgetPatrol { .. })
        );
        if patrol_running
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

    /// 交回 `wait_for_next_work` 取走的那件工作的结论，返回此后索引的完整度。
    pub(super) fn finish_work(&self, outcome: &WorkOutcome) -> IndexCompleteness {
        self.finish_work_at(outcome, Instant::now())
    }

    fn finish_work_at(&self, outcome: &WorkOutcome, now: Instant) -> IndexCompleteness {
        let mut guard = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        let state = &mut *guard;
        let in_flight = state
            .in_flight
            .take()
            .expect("finished search daemon work was never taken");
        let failure = match outcome {
            WorkOutcome::Failed { message } => Some(message.as_str()),
            WorkOutcome::Succeeded | WorkOutcome::Cancelled => None,
        };
        let pending_work = &mut state.pending_work;
        match in_flight {
            InFlightWork::FullCheck => {
                if let Some(message) = failure {
                    let roots = pending_work.roots.clone();
                    pending_work.record_failed_roots(roots, message, now);
                }
            }
            InFlightWork::Local { roots } => {
                if let Some(message) = failure {
                    pending_work.record_failed_roots(roots, message, now);
                }
            }
            InFlightWork::WatchBudgetPatrol { roots } => {
                if let Some(message) = failure {
                    pending_work.record_failed_roots(roots, message, now);
                }
                state.watch_budget_patrol_not_before = Some(now + WATCH_BUDGET_PATROL_INTERVAL);
            }
            InFlightWork::DirtyRootRecovery { roots } => {
                pending_work.finish_dirty_root_recovery(&roots, outcome, now);
            }
        }
        self.ready_signal.notify_one();
        state.pending_work.index_completeness()
    }

    pub(super) fn wait_for_next_work(&self) -> DaemonWorkRequest {
        let mut state = self
            .state
            .lock()
            .expect("search daemon work queue mutex poisoned");
        loop {
            if let Some(work_request) = state.pending_work.take_next_work(Instant::now()) {
                state.in_flight = InFlightWork::of(&work_request, &state.pending_work);
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
        queue.finish_work_at(&WorkOutcome::Succeeded, started_at);

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
            queue.finish_work_at(&WorkOutcome::Succeeded, failed_at),
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
            queue.finish_work_at(&WorkOutcome::Succeeded, second_retry_at),
            IndexCompleteness::Complete
        );
    }

    #[test]
    fn a_failed_local_update_rechecks_only_the_root_it_touched() {
        let first = PathBuf::from("/first");
        let second = PathBuf::from("/second");
        let queue =
            DaemonWorkQueue::starting_with_full_check(vec![first.clone(), second], Vec::new());
        let now = Instant::now();
        assert_eq!(
            take_work_at(&queue, now),
            Some(DaemonWorkRequest::StartupCheck)
        );
        assert_eq!(
            queue.finish_work_at(&WorkOutcome::Succeeded, now),
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
}
