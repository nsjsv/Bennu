use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::{SearchExcludeRules, SearchIndexConfig};
use crate::database::{SearchDatabase, MAX_KNOWN_ENTRY_PAGE_ENTRIES};
use crate::error::SearchResult;
use crate::filesystem::{FilesystemObservation, LocalFilesystemBoundary};
use crate::model::SearchFileKind;

use super::bounded_paths::{estimated_path_bytes, BoundedPathSet};
use super::watch_budget_patrol::WatchBudgetPatrol;
use super::watch_ingress::{deliver_watch_event, WatchOverflowState};

const TARGET_REGISTERED_DIRECTORIES: usize = 96_000;
const RESERVED_SYSTEM_WATCHES: usize = 16_384;
const MAX_REGISTERED_PATH_BYTES: usize = 32_000_000;
const MAX_COVERAGE_GAPS: usize = 256;
const MAX_COVERAGE_GAP_BYTES: usize = 262_144;
const MAX_COVERAGE_MESSAGE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WatchCoverageHealth {
    Healthy,
    HybridPatrol { root_count: usize },
    Incomplete { gap_count: usize, message: String },
    BackendUnavailable { message: String },
}

pub(super) struct WatchCoverageRefresh {
    pub(super) repair_paths: Vec<PathBuf>,
    pub(super) patrol_paths: Vec<PathBuf>,
    pub(super) health: WatchCoverageHealth,
}

trait DirectoryWatchBackend {
    fn watch_directory(&mut self, path: &Path) -> notify::Result<()>;
    fn unwatch_directory(&mut self, path: &Path) -> notify::Result<()>;
}

impl DirectoryWatchBackend for RecommendedWatcher {
    fn watch_directory(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::watch(self, path, RecursiveMode::NonRecursive)
    }

    fn unwatch_directory(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::unwatch(self, path)
    }
}

pub(super) struct RecommendedWatchCoverage {
    inner: DirectoryWatchCoverage<RecommendedWatcher>,
    database_path: PathBuf,
    budget_patrol: WatchBudgetPatrol,
}

impl RecommendedWatchCoverage {
    pub(super) fn create(
        config: SearchIndexConfig,
        database_path: PathBuf,
        event_sender: SyncSender<notify::Result<Event>>,
        overflow_state: Arc<WatchOverflowState>,
    ) -> SearchResult<Self> {
        let watcher = notify::recommended_watcher(move |event_result| {
            deliver_watch_event(&event_sender, &overflow_state, event_result);
        })?;
        let budget_patrol = WatchBudgetPatrol::open(&database_path)?;
        Ok(Self {
            inner: DirectoryWatchCoverage::new(watcher, config),
            database_path,
            budget_patrol,
        })
    }

    pub(super) fn initialize(&mut self) -> SearchResult<WatchCoverageRefresh> {
        let refresh = self.restore_snapshot_directories()?;
        tracing::info!(
            target: "file_search::watch",
            event = "watch_coverage_initialized",
            registered_directories = self.inner.registered_directories.len(),
            registered_path_bytes = self.inner.registered_directories.estimated_bytes(),
            hybrid_roots = self.inner.watch_budget_overflow_roots.len(),
            health = ?refresh.health,
            "filesystem watch coverage initialized"
        );
        Ok(refresh)
    }

    pub(super) fn restore_snapshot_directories(&mut self) -> SearchResult<WatchCoverageRefresh> {
        let previous_gaps = self.inner.current_gaps();
        let previous_gap_messages = self.inner.current_gap_messages();
        self.inner.begin_snapshot_restore();
        self.inner.register_roots();
        let database = match SearchDatabase::open_read_only(&self.database_path) {
            Ok(database) => database,
            Err(error) => {
                self.inner.restore_previous_gaps(&previous_gap_messages);
                return Err(error);
            }
        };
        let mut after_path = None;
        loop {
            let page = match database
                .directory_snapshot_paths_page(after_path.as_deref(), MAX_KNOWN_ENTRY_PAGE_ENTRIES)
            {
                Ok(page) => page,
                Err(error) => {
                    self.inner.restore_previous_gaps(&previous_gap_messages);
                    return Err(error);
                }
            };
            if page.is_empty() {
                break;
            }
            after_path = page.last().cloned();
            self.inner.register_snapshot_directories(page);
        }
        Ok(self.inner.finish_refresh(previous_gaps))
    }

    pub(super) fn next_watch_budget_patrol(&mut self) -> SearchResult<Vec<PathBuf>> {
        self.budget_patrol.next_directories(
            &self.inner.watch_budget_overflow_roots,
            &self.inner.registered_directories,
        )
    }

    pub(super) fn refresh_changed_paths(
        &mut self,
        changed_paths: &[PathBuf],
    ) -> WatchCoverageRefresh {
        self.inner.refresh_changed_paths(changed_paths)
    }

    pub(super) fn retry_gaps(&mut self) -> WatchCoverageRefresh {
        self.inner.retry_gaps()
    }

    pub(super) fn record_notify_error(&mut self, error: &notify::Error) {
        self.inner.record_notify_error(error);
    }

    pub(super) fn mark_backend_unavailable(&mut self, message: impl Into<String>) {
        self.inner.backend_failure = Some(bounded_message(message.into()));
    }

    pub(super) fn health(&self) -> WatchCoverageHealth {
        self.inner.health()
    }
}

struct BoundedCoverageGaps {
    entries: BTreeMap<PathBuf, String>,
    estimated_bytes: usize,
    max_entries: usize,
    max_estimated_bytes: usize,
}

impl BoundedCoverageGaps {
    fn new(max_entries: usize, max_estimated_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            estimated_bytes: 0,
            max_entries,
            max_estimated_bytes,
        }
    }

    fn insert(&mut self, path: PathBuf, message: String) -> Result<(), ()> {
        let message = bounded_message(message);
        if let Some(previous_message) = self.entries.get_mut(&path) {
            let next_bytes = self
                .estimated_bytes
                .saturating_sub(previous_message.len())
                .saturating_add(message.len());
            if next_bytes > self.max_estimated_bytes {
                return Err(());
            }
            self.estimated_bytes = next_bytes;
            *previous_message = message;
            return Ok(());
        }

        let entry_bytes = estimated_path_bytes(&path).saturating_add(message.len());
        let next_bytes = self.estimated_bytes.saturating_add(entry_bytes);
        if self.entries.len() >= self.max_entries || next_bytes > self.max_estimated_bytes {
            return Err(());
        }
        self.entries.insert(path, message);
        self.estimated_bytes = next_bytes;
        Ok(())
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.estimated_bytes = 0;
    }

    fn remove(&mut self, path: &Path) -> Option<String> {
        let message = self.entries.remove(path)?;
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_sub(estimated_path_bytes(path).saturating_add(message.len()));
        Some(message)
    }

    fn keys(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.keys()
    }

    #[cfg(test)]
    fn contains_key(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    fn retain(&mut self, mut retain_entry: impl FnMut(&Path) -> bool) {
        let removed_paths = self
            .entries
            .keys()
            .filter(|path| !retain_entry(path))
            .cloned()
            .collect::<Vec<_>>();
        for removed_path in removed_paths {
            self.remove(&removed_path);
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchRegistration {
    Ready,
    Failed,
    CapacityReached,
}

struct DirectoryWatchCoverage<Backend> {
    backend: Backend,
    config: SearchIndexConfig,
    rules: SearchExcludeRules,
    registered_directories: BoundedPathSet,
    coverage_gaps: BoundedCoverageGaps,
    watch_budget_overflow_roots: BTreeMap<PathBuf, PathBuf>,
    gap_budget_exceeded_roots: BTreeSet<PathBuf>,
    backend_failure: Option<String>,
}

impl<Backend: DirectoryWatchBackend> DirectoryWatchCoverage<Backend> {
    fn new(backend: Backend, config: SearchIndexConfig) -> Self {
        Self {
            rules: SearchExcludeRules::from_index_config(&config)
                .expect("search index configuration must be validated before watch setup"),
            backend,
            config,
            registered_directories: BoundedPathSet::new(
                recommended_watch_limit(),
                MAX_REGISTERED_PATH_BYTES,
            ),
            coverage_gaps: BoundedCoverageGaps::new(MAX_COVERAGE_GAPS, MAX_COVERAGE_GAP_BYTES),
            watch_budget_overflow_roots: BTreeMap::new(),
            gap_budget_exceeded_roots: BTreeSet::new(),
            backend_failure: None,
        }
    }

    fn begin_snapshot_restore(&mut self) {
        self.watch_budget_overflow_roots.clear();
        self.gap_budget_exceeded_roots.clear();
        self.coverage_gaps.clear();
    }

    fn restore_previous_gaps(&mut self, previous_gaps: &BTreeMap<PathBuf, String>) {
        for (gap, message) in previous_gaps {
            if !self.coverage_gaps.entries.contains_key(gap) {
                self.record_gap(gap.clone(), message.clone());
            }
        }
    }

    fn register_roots(&mut self) {
        self.backend_failure = None;
        let roots = self.config.available_roots().cloned().collect::<Vec<_>>();
        for root in roots {
            self.refresh_path(&root);
        }
    }

    fn register_snapshot_directories(&mut self, directories: Vec<PathBuf>) {
        for directory in directories {
            if self.config.path_is_available(&directory) {
                self.register_directory(&directory);
            }
        }
    }

    fn refresh_changed_paths(&mut self, changed_paths: &[PathBuf]) -> WatchCoverageRefresh {
        let previous_gaps = self.current_gaps();
        for changed_path in changed_paths {
            self.refresh_path(changed_path);
        }
        self.finish_refresh(previous_gaps)
    }

    fn retry_gaps(&mut self) -> WatchCoverageRefresh {
        let previous_gaps = self.current_gaps();
        let retry_paths = previous_gaps.iter().cloned().collect::<Vec<_>>();
        for retry_path in retry_paths {
            self.refresh_path(&retry_path);
        }
        self.finish_refresh(previous_gaps)
    }

    fn current_gaps(&self) -> BTreeSet<PathBuf> {
        self.coverage_gaps.keys().cloned().collect()
    }

    fn current_gap_messages(&self) -> BTreeMap<PathBuf, String> {
        self.coverage_gaps.entries.clone()
    }

    fn finish_refresh(&self, previous_gaps: BTreeSet<PathBuf>) -> WatchCoverageRefresh {
        let current_gaps = self.current_gaps();
        let repair_paths = previous_gaps.difference(&current_gaps).cloned().collect();
        let patrol_paths = current_gaps.into_iter().collect::<Vec<_>>();
        WatchCoverageRefresh {
            repair_paths,
            patrol_paths,
            health: self.health(),
        }
    }

    fn refresh_path(&mut self, path: &Path) {
        let Some(root) = self.config.owning_root(path).map(Path::to_path_buf) else {
            return;
        };
        if !self.config.path_is_available(path) {
            self.prune_scope(&root);
            return;
        }

        let boundary = match LocalFilesystemBoundary::observe(&root, &self.rules) {
            Ok(FilesystemObservation::Complete(boundary)) => boundary,
            Ok(FilesystemObservation::Inaccessible { scope }) => {
                self.record_gap(scope, "search root is inaccessible");
                return;
            }
            Ok(FilesystemObservation::Missing { scope }) => {
                self.prune_scope(&scope);
                self.record_gap(scope, "search root is missing");
                return;
            }
            Ok(FilesystemObservation::PolicyExcluded { scope })
            | Ok(FilesystemObservation::DelegatedToNestedRoot { scope }) => {
                self.prune_scope(&scope);
                return;
            }
            Err(error) => {
                self.record_gap(root, error.to_string());
                return;
            }
        };

        match boundary.inspect_path(path) {
            Ok(FilesystemObservation::Complete(entry)) => {
                self.coverage_gaps.remove(path);
                if entry.kind() == SearchFileKind::Directory {
                    self.register_directory(path);
                } else {
                    self.prune_scope(path);
                }
            }
            Ok(FilesystemObservation::Inaccessible { scope }) => {
                self.record_gap(scope, "directory is inaccessible");
            }
            Ok(FilesystemObservation::Missing { scope })
            | Ok(FilesystemObservation::PolicyExcluded { scope })
            | Ok(FilesystemObservation::DelegatedToNestedRoot { scope }) => {
                self.prune_scope(&scope);
            }
            Err(error) => self.record_gap(path.to_path_buf(), error.to_string()),
        }
    }

    fn register_directory(&mut self, directory: &Path) -> WatchRegistration {
        if self.registered_directories.contains(directory) {
            self.coverage_gaps.remove(directory);
            return WatchRegistration::Ready;
        }
        if self
            .registered_directories
            .insert(directory.to_path_buf())
            .is_err()
        {
            self.record_watch_budget_overflow(directory);
            return WatchRegistration::CapacityReached;
        }
        match self.backend.watch_directory(directory) {
            Ok(()) => {
                self.coverage_gaps.remove(directory);
                WatchRegistration::Ready
            }
            Err(error) => {
                self.registered_directories.remove(directory);
                self.record_gap(directory.to_path_buf(), error.to_string());
                WatchRegistration::Failed
            }
        }
    }

    fn prune_scope(&mut self, scope: &Path) {
        let mut registered_descendants = self
            .registered_directories
            .iter()
            .filter(|directory| **directory == *scope || directory.starts_with(scope))
            .map(|directory| directory.to_path_buf())
            .collect::<Vec<_>>();
        registered_descendants.sort_unstable();
        for registered_directory in registered_descendants.into_iter().rev() {
            let _ = self.backend.unwatch_directory(&registered_directory);
            self.registered_directories.remove(&registered_directory);
        }
        self.coverage_gaps
            .retain(|gap| gap != scope && !gap.starts_with(scope));
    }

    fn record_gap(&mut self, scope: PathBuf, message: impl Into<String>) {
        if self
            .coverage_gaps
            .insert(scope.clone(), message.into())
            .is_err()
        {
            self.record_gap_budget_overflow(&scope);
        }
    }

    fn record_notify_error(&mut self, error: &notify::Error) {
        if error.paths.is_empty() {
            self.backend_failure = Some(bounded_message(error.to_string()));
            return;
        }
        for path in &error.paths {
            self.record_gap(path.clone(), error.to_string());
        }
    }

    fn health(&self) -> WatchCoverageHealth {
        if let Some(message) = &self.backend_failure {
            return WatchCoverageHealth::BackendUnavailable {
                message: message.clone(),
            };
        }
        if !self.gap_budget_exceeded_roots.is_empty() {
            return WatchCoverageHealth::Incomplete {
                gap_count: self
                    .coverage_gaps
                    .len()
                    .saturating_add(self.gap_budget_exceeded_roots.len()),
                message: format!(
                    "search watch failures exceeded bounded diagnostics for {} roots",
                    self.gap_budget_exceeded_roots.len()
                ),
            };
        }
        if !self.coverage_gaps.is_empty() {
            return WatchCoverageHealth::Incomplete {
                gap_count: self.coverage_gaps.len(),
                message: format!(
                    "{} search directory watches are unavailable; retrying with targeted patrol",
                    self.coverage_gaps.len()
                ),
            };
        }
        if !self.watch_budget_overflow_roots.is_empty() {
            return WatchCoverageHealth::HybridPatrol {
                root_count: self.watch_budget_overflow_roots.len(),
            };
        }
        WatchCoverageHealth::Healthy
    }

    fn record_watch_budget_overflow(&mut self, path: &Path) {
        let Some(root) = self.containing_root(path) else {
            return;
        };
        self.watch_budget_overflow_roots
            .entry(root)
            .or_insert_with(|| path.to_path_buf());
    }

    fn record_gap_budget_overflow(&mut self, path: &Path) {
        if let Some(root) = self.containing_root(path) {
            self.gap_budget_exceeded_roots.insert(root);
        } else {
            self.gap_budget_exceeded_roots
                .extend(self.config.available_roots().cloned());
        }
    }

    fn containing_root(&self, path: &Path) -> Option<PathBuf> {
        self.config
            .path_is_available(path)
            .then(|| self.config.owning_root(path).map(Path::to_path_buf))
            .flatten()
    }
}

fn recommended_watch_limit() -> usize {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(watch_limit_for)
        .unwrap_or(TARGET_REGISTERED_DIRECTORIES)
}

fn watch_limit_for(system_limit: usize) -> usize {
    system_limit
        .saturating_sub(RESERVED_SYSTEM_WATCHES)
        .clamp(1, TARGET_REGISTERED_DIRECTORIES)
}

fn bounded_message(mut message: String) -> String {
    if message.len() <= MAX_COVERAGE_MESSAGE_BYTES {
        return message;
    }
    let mut boundary = MAX_COVERAGE_MESSAGE_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;

    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct RecordingBackend {
        watched: BTreeSet<PathBuf>,
        failing_paths: BTreeSet<PathBuf>,
        unwatched: Vec<PathBuf>,
    }

    impl DirectoryWatchBackend for RecordingBackend {
        fn watch_directory(&mut self, path: &Path) -> notify::Result<()> {
            if self.failing_paths.contains(path) {
                return Err(notify::Error::io(io::Error::from(
                    io::ErrorKind::PermissionDenied,
                )));
            }
            self.watched.insert(path.to_path_buf());
            Ok(())
        }

        fn unwatch_directory(&mut self, path: &Path) -> notify::Result<()> {
            self.unwatched.push(path.to_path_buf());
            self.watched.remove(path);
            Ok(())
        }
    }

    fn config_for(root: &Path) -> SearchIndexConfig {
        SearchIndexConfig {
            roots: vec![root.to_path_buf()],
            excluded_paths: Vec::new(),
            unavailable_roots: Vec::new(),
            content_indexing_enabled: true,
            max_extract_bytes: 1024,
        }
    }

    #[test]
    fn startup_registers_roots_and_persisted_directories_without_walking() {
        let directory = tempdir().unwrap();
        let visible = directory.path().join("visible");
        let nested = visible.join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        let mut coverage =
            DirectoryWatchCoverage::new(RecordingBackend::default(), config_for(directory.path()));
        coverage.register_roots();
        coverage.register_snapshot_directories(vec![visible.clone(), nested.clone()]);

        assert!(coverage.registered_directories.contains(directory.path()));
        assert!(coverage.registered_directories.contains(&visible));
        assert!(coverage.registered_directories.contains(&nested));
    }

    #[test]
    fn pruning_unwatches_nested_directories_before_their_parent() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("child");
        let nested = child.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let mut coverage =
            DirectoryWatchCoverage::new(RecordingBackend::default(), config_for(directory.path()));

        coverage.register_roots();
        coverage.register_snapshot_directories(vec![child.clone(), nested.clone()]);
        coverage.prune_scope(directory.path());

        assert_eq!(
            coverage.backend.unwatched,
            vec![nested, child, directory.path().to_path_buf()]
        );
    }

    #[test]
    fn interrupted_snapshot_restore_keeps_previous_gaps_retryable() {
        let directory = tempdir().unwrap();
        let missing = directory.path().join("missing");
        let mut coverage =
            DirectoryWatchCoverage::new(RecordingBackend::default(), config_for(directory.path()));
        coverage.record_gap(missing.clone(), "backend unavailable");
        let previous_gaps = coverage.current_gap_messages();

        coverage.begin_snapshot_restore();
        coverage.restore_previous_gaps(&previous_gaps);

        assert_eq!(coverage.current_gaps(), BTreeSet::from([missing.clone()]));
        assert_eq!(
            coverage.coverage_gaps.entries[&missing],
            "backend unavailable"
        );
    }

    #[test]
    fn one_directory_watch_failure_does_not_discard_sibling_coverage() {
        let directory = tempdir().unwrap();
        let denied = directory.path().join("denied");
        let healthy = directory.path().join("healthy");
        std::fs::create_dir_all(&denied).unwrap();
        std::fs::create_dir_all(&healthy).unwrap();
        let mut backend = RecordingBackend::default();
        backend.failing_paths.insert(denied.clone());

        let mut coverage = DirectoryWatchCoverage::new(backend, config_for(directory.path()));
        coverage.register_roots();
        coverage.register_snapshot_directories(vec![denied.clone(), healthy.clone()]);

        assert!(coverage.registered_directories.contains(&healthy));
        assert!(!coverage.registered_directories.contains(&denied));
        assert!(coverage.coverage_gaps.contains_key(&denied));
    }

    #[test]
    fn successful_retry_clears_gap_and_requests_a_repair_scan() {
        let directory = tempdir().unwrap();
        let denied = directory.path().join("denied");
        std::fs::create_dir_all(&denied).unwrap();
        let mut backend = RecordingBackend::default();
        backend.failing_paths.insert(denied.clone());
        let mut coverage = DirectoryWatchCoverage::new(backend, config_for(directory.path()));
        coverage.register_snapshot_directories(vec![denied.clone()]);

        coverage.backend.failing_paths.remove(&denied);
        let refresh = coverage.retry_gaps();

        assert_eq!(refresh.health, WatchCoverageHealth::Healthy);
        assert_eq!(refresh.repair_paths, vec![denied.clone()]);
        assert!(coverage.registered_directories.contains(&denied));
    }

    #[test]
    fn watch_failure_budget_stays_degraded_and_is_not_hybrid_patrol() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut backend = RecordingBackend::default();
        backend
            .failing_paths
            .extend([first.clone(), second.clone()]);
        let mut coverage = DirectoryWatchCoverage::new(backend, config_for(directory.path()));
        coverage.coverage_gaps = BoundedCoverageGaps::new(1, usize::MAX);

        coverage.register_snapshot_directories(vec![first.clone(), second.clone()]);

        assert!(coverage.watch_budget_overflow_roots.is_empty());
        assert_eq!(
            coverage.gap_budget_exceeded_roots,
            BTreeSet::from([directory.path().to_path_buf()])
        );
        assert!(matches!(
            coverage.health(),
            WatchCoverageHealth::Incomplete { .. }
        ));

        coverage.backend.failing_paths.clear();
        let previous_gaps = coverage.current_gaps();
        coverage.begin_snapshot_restore();
        coverage.register_roots();
        coverage.register_snapshot_directories(vec![first.clone(), second]);
        let refresh = coverage.finish_refresh(previous_gaps);

        assert_eq!(refresh.health, WatchCoverageHealth::Healthy);
        assert_eq!(refresh.repair_paths, vec![first]);
        assert!(coverage.gap_budget_exceeded_roots.is_empty());
    }

    #[test]
    fn directory_registration_capacity_uses_hybrid_patrol_without_growing_paths() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("child");
        std::fs::create_dir_all(&child).unwrap();
        let mut coverage =
            DirectoryWatchCoverage::new(RecordingBackend::default(), config_for(directory.path()));
        coverage.registered_directories = BoundedPathSet::new(1, usize::MAX);

        coverage.register_roots();
        coverage.register_snapshot_directories(vec![child.clone()]);

        assert_eq!(coverage.registered_directories.len(), 1);
        assert_eq!(
            coverage.watch_budget_overflow_roots,
            BTreeMap::from([(directory.path().to_path_buf(), child)])
        );
        let refresh = coverage.finish_refresh(BTreeSet::new());
        assert!(refresh.patrol_paths.is_empty());
        assert_eq!(
            refresh.health,
            WatchCoverageHealth::HybridPatrol { root_count: 1 }
        );
    }

    #[test]
    fn watch_budget_covers_the_observed_seventy_eight_thousand_directories() {
        assert_eq!(watch_limit_for(524_288), TARGET_REGISTERED_DIRECTORIES);
        assert!(watch_limit_for(524_288) >= 78_710);
        assert_eq!(watch_limit_for(100_000), 100_000 - RESERVED_SYSTEM_WATCHES);
        assert_eq!(watch_limit_for(RESERVED_SYSTEM_WATCHES), 1);
    }
}
