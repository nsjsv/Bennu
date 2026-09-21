use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use tokio_util::sync::CancellationToken;

use crate::crawler::IndexMaintenanceProgress;
use crate::database::{SearchDatabase, SearchRootMount};
use crate::error::{SearchError, SearchResult};
use crate::logging::bounded_search_log_detail;
use crate::managed_search_index::ManagedSearchIndex;
use crate::model::{
    ExtractorCapability, IndexHealth, IndexPhase, IndexStatus, SearchPathConfigurationPhase,
    SearchPathConfigurationStatus, SearchRootAvailability, SearchRootStatus,
};
use crate::search_path_store::SearchPathStore;
use crate::writer::IndexWriter;
use crate::{
    SearchIndexConfig, SearchIndexer, SearchPathPolicy, SearchPathPreferences,
    VersionedSearchPathPreferences,
};

mod bounded_paths;
mod memory;
mod root_mount;
mod watch_budget_patrol;
mod watch_coverage;
mod watch_ingress;
mod work_queue;

#[cfg(test)]
use self::bounded_paths::BoundedPathSet;
use self::memory::release_allocator_idle_pages;
use self::root_mount::observe_root_mounts;
use self::watch_coverage::WatchCoverageHealth;
#[cfg(test)]
use self::watch_ingress::changed_paths_from_watch_event;
use self::watch_ingress::{DaemonWatchIngress, DaemonWatchIngressBootstrap};
#[cfg(test)]
use self::work_queue::{
    dirty_root_retry_delay, PendingDaemonWork, DIRTY_ROOT_QUIET_WINDOW, DIRTY_ROOT_RETRY_BASE,
    DIRTY_ROOT_RETRY_MAX,
};
use self::work_queue::{DaemonWorkQueue, DaemonWorkRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonLifecyclePhase {
    Starting,
    Checking {
        checked_entries: u64,
        changed_entries: u64,
    },
    Crawling {
        scanned_entries: u64,
        current_scope: PathBuf,
    },
    Applying {
        pending_mutations: u64,
    },
    Complete,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonLifecycleSnapshot {
    phase: DaemonLifecyclePhase,
    maintenance_backend_failure: Option<String>,
    recovery_rebuild_message: Option<String>,
    watch_coverage_health: WatchCoverageHealth,
    visible_indexed_files: u64,
    capabilities: Vec<ExtractorCapability>,
    path_configuration: SearchPathConfigurationStatus,
}

impl DaemonLifecycleSnapshot {
    fn starting(
        visible_indexed_files: u64,
        recovery_rebuild_message: Option<String>,
        path_configuration: SearchPathConfigurationStatus,
    ) -> Self {
        Self {
            phase: DaemonLifecyclePhase::Starting,
            maintenance_backend_failure: None,
            recovery_rebuild_message,
            watch_coverage_health: WatchCoverageHealth::Healthy,
            visible_indexed_files,
            capabilities: Vec::new(),
            path_configuration,
        }
    }

    fn begin_checking(&mut self) {
        self.phase = DaemonLifecyclePhase::Checking {
            checked_entries: 0,
            changed_entries: 0,
        };
    }

    fn apply_progress(&mut self, progress: IndexMaintenanceProgress) {
        self.phase = match progress {
            IndexMaintenanceProgress::Checking {
                checked_entries,
                changed_entries,
            } => DaemonLifecyclePhase::Checking {
                checked_entries,
                changed_entries,
            },
            IndexMaintenanceProgress::Crawling {
                scanned_entries,
                current_scope,
            } => DaemonLifecyclePhase::Crawling {
                scanned_entries,
                current_scope,
            },
            IndexMaintenanceProgress::Applying { pending_mutations } => {
                DaemonLifecyclePhase::Applying { pending_mutations }
            }
        };
    }

    fn finish_watch_cycle(&mut self, visible_indexed_files: u64) {
        self.phase = DaemonLifecyclePhase::Complete;
        self.visible_indexed_files = visible_indexed_files;
        self.recovery_rebuild_message = None;
        if matches!(
            self.path_configuration.phase,
            SearchPathConfigurationPhase::Applying
        ) && self.path_configuration.desired_revision
            == self.path_configuration.effective_revision
        {
            self.path_configuration.phase = SearchPathConfigurationPhase::Ready;
        }
    }

    fn record_error(&mut self, message: String) {
        self.phase = DaemonLifecyclePhase::Failed {
            message: message.clone(),
        };
        if matches!(
            self.path_configuration.phase,
            SearchPathConfigurationPhase::Applying
        ) {
            self.path_configuration.phase = SearchPathConfigurationPhase::Failed { message };
        }
    }

    fn record_maintenance_failure(&mut self, message: String) {
        self.maintenance_backend_failure = Some(message);
    }

    fn clear_maintenance_failure(&mut self) {
        self.maintenance_backend_failure = None;
    }

    fn update_watch_coverage(&mut self, health: WatchCoverageHealth) {
        self.watch_coverage_health = health;
    }

    fn update_path_configuration(&mut self, status: SearchPathConfigurationStatus) {
        self.path_configuration = status;
    }

    fn to_index_status(&self) -> IndexStatus {
        let phase = match &self.phase {
            DaemonLifecyclePhase::Starting => IndexPhase::Starting,
            DaemonLifecyclePhase::Checking {
                checked_entries,
                changed_entries,
            } => IndexPhase::Checking {
                checked_entries: *checked_entries,
                changed_entries: *changed_entries,
            },
            DaemonLifecyclePhase::Crawling {
                scanned_entries,
                current_scope,
            } => IndexPhase::Crawling {
                scanned_entries: *scanned_entries,
                current_scope: current_scope.clone(),
            },
            DaemonLifecyclePhase::Applying { pending_mutations } => IndexPhase::Applying {
                pending_mutations: *pending_mutations,
            },
            DaemonLifecyclePhase::Complete => IndexPhase::Complete,
            DaemonLifecyclePhase::Failed { message } => IndexPhase::Failed {
                message: message.clone(),
            },
        };
        IndexStatus {
            phase,
            visible_indexed_files: self.visible_indexed_files,
            health: self.index_health(),
            capabilities: self.capabilities.clone(),
            path_configuration: self.path_configuration.clone(),
        }
    }

    fn index_health(&self) -> IndexHealth {
        if let Some(message) = self.maintenance_backend_failure.as_ref() {
            return IndexHealth::Error {
                message: message.clone(),
            };
        }
        match &self.watch_coverage_health {
            WatchCoverageHealth::Healthy | WatchCoverageHealth::HybridPatrol { .. } => {
                match self.recovery_rebuild_message.as_ref() {
                    Some(message) => IndexHealth::Degraded {
                        message: message.clone(),
                    },
                    None => IndexHealth::Healthy,
                }
            }
            WatchCoverageHealth::Incomplete { message, .. } => IndexHealth::Degraded {
                message: message.clone(),
            },
            WatchCoverageHealth::BackendUnavailable { message } => IndexHealth::Error {
                message: message.clone(),
            },
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum DaemonRuntimePhase {
    #[default]
    Created,
    Maintaining,
    Stopped,
}

struct ActiveMaintenance {
    work_queue: Arc<DaemonWorkQueue>,
    crawl_cancellation: CancellationToken,
    worker_join: thread::JoinHandle<SearchResult<()>>,
    watch_ingress: Option<DaemonWatchIngress>,
}

#[derive(Default)]
struct DaemonRuntimeState {
    phase: DaemonRuntimePhase,
    maintenance_requested: bool,
    maintenance: Option<ActiveMaintenance>,
}

struct PathConfigurationRuntime {
    desired: VersionedSearchPathPreferences,
    effective: VersionedSearchPathPreferences,
    policy: Option<SearchPathPolicy>,
    effective_config: SearchIndexConfig,
    mounts: Vec<SearchRootMount>,
    unavailable_roots: Vec<PathBuf>,
    pending_frontiers: Vec<PathBuf>,
    phase: SearchPathConfigurationPhase,
}

pub struct SearchDaemonCore {
    database_path: PathBuf,
    home_directory: Option<PathBuf>,
    base_config: SearchIndexConfig,
    path_store: Option<SearchPathStore>,
    path_configuration: Mutex<PathConfigurationRuntime>,
    writer: Arc<IndexWriter>,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    directory_snapshot_epoch: Arc<AtomicU64>,
    runtime_state: Mutex<DaemonRuntimeState>,
    operation_serialization: Mutex<()>,
}

impl SearchDaemonCore {
    pub fn new(database_path: PathBuf, config: SearchIndexConfig) -> SearchResult<Self> {
        config
            .validate()
            .map_err(SearchError::InvalidConfiguration)?;
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SearchError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let home_directory = config.roots.first().cloned();
        let path_store = home_directory
            .as_ref()
            .map(|_| search_path_store_for_database(&database_path))
            .transpose()?;
        let managed_index_open = ManagedSearchIndex::new(database_path.clone()).open()?;
        let recovery_rebuild_message = managed_index_open.recovery_notice.as_ref().map(|notice| {
            let message = notice.message();
            let log_message = bounded_search_log_detail(&message);
            tracing::warn!(
                target: "file_search::daemon",
                event = "damaged_index_quarantined",
                detail = %log_message,
                "damaged search index quarantined; rebuilding from filesystem"
            );
            message
        });
        let path_configuration = initialize_path_configuration(
            &managed_index_open.database,
            &config,
            home_directory.as_deref(),
            path_store.as_ref(),
        )?;
        let path_status = path_configuration_status(&path_configuration);
        let writer = Arc::new(IndexWriter::spawn(managed_index_open.database));
        let known_count = writer.count()?;
        let lifecycle_snapshot = Arc::new(Mutex::new(DaemonLifecycleSnapshot::starting(
            known_count,
            recovery_rebuild_message,
            path_status,
        )));

        Ok(Self {
            database_path,
            home_directory,
            base_config: config,
            path_store,
            path_configuration: Mutex::new(path_configuration),
            writer,
            lifecycle_snapshot,
            directory_snapshot_epoch: Arc::new(AtomicU64::new(0)),
            runtime_state: Mutex::new(DaemonRuntimeState::default()),
            operation_serialization: Mutex::new(()),
        })
    }

    pub fn start_index_maintenance(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        let (config, pending_frontiers) = {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            match runtime_state.phase {
                DaemonRuntimePhase::Maintaining => return Ok(()),
                DaemonRuntimePhase::Stopped => return Err(daemon_core_stopped()),
                DaemonRuntimePhase::Created => {
                    runtime_state.maintenance_requested = true;
                    let path_configuration = self
                        .path_configuration
                        .lock()
                        .expect("search path configuration mutex poisoned");
                    (
                        path_configuration.effective_config.clone(),
                        path_configuration.pending_frontiers.clone(),
                    )
                }
            }
        };

        let (maintenance, maintenance_failure) = spawn_active_maintenance(
            Arc::clone(&self.writer),
            config,
            pending_frontiers,
            self.database_path.clone(),
            Arc::clone(&self.lifecycle_snapshot),
            Arc::clone(&self.directory_snapshot_epoch),
        )?;
        {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            if matches!(runtime_state.phase, DaemonRuntimePhase::Stopped) {
                drop(runtime_state);
                stop_active_maintenance(maintenance)?;
                return Err(daemon_core_stopped());
            }
            runtime_state.phase = DaemonRuntimePhase::Maintaining;
            runtime_state.maintenance = Some(maintenance);
            self.path_configuration
                .lock()
                .expect("search path configuration mutex poisoned")
                .pending_frontiers
                .clear();
        }

        match maintenance_failure {
            Some(error) => {
                self.record_maintenance_failure(error.to_string());
                Err(error)
            }
            None => {
                self.lifecycle_snapshot
                    .lock()
                    .expect("search daemon lifecycle mutex poisoned")
                    .clear_maintenance_failure();
                Ok(())
            }
        }
    }

    pub fn current_status(&self) -> IndexStatus {
        let _ = self.reconcile_root_mounts();
        let roots = {
            let path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration
                .policy
                .as_ref()
                .map(|policy| {
                    root_statuses(
                        &policy.preferences().custom_roots,
                        &path_configuration.mounts,
                    )
                })
                .unwrap_or_default()
        };
        let mut lifecycle = self
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned");
        lifecycle.path_configuration.roots = roots;
        lifecycle.to_index_status()
    }

    pub fn current_path_preferences(&self) -> VersionedSearchPathPreferences {
        self.path_configuration
            .lock()
            .expect("search path configuration mutex poisoned")
            .desired
            .clone()
    }

    pub fn configure_search_paths(
        &self,
        expected_revision: u64,
        preferences: SearchPathPreferences,
    ) -> SearchResult<VersionedSearchPathPreferences> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        if matches!(
            self.runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned")
                .phase,
            DaemonRuntimePhase::Stopped
        ) {
            return Err(daemon_core_stopped());
        }
        let home_directory = self.home_directory.as_ref().ok_or_else(|| {
            SearchError::InvalidConfiguration(
                "search path configuration requires a Home root".to_owned(),
            )
        })?;
        let policy = SearchPathPolicy::new(home_directory.clone(), preferences)
            .map_err(SearchError::InvalidConfiguration)?;
        let normalized_preferences = policy.preferences().clone();
        let (next, previous_mounts, previous_config, previous_policy) = {
            let path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            if path_configuration.desired.revision != expected_revision {
                return Err(SearchError::InvalidConfiguration(format!(
                    "search path configuration revision conflict: expected {expected_revision}, current {}",
                    path_configuration.desired.revision
                )));
            }
            let retrying_failed_desired = matches!(
                path_configuration.phase,
                SearchPathConfigurationPhase::Failed { .. }
            ) && path_configuration.desired.preferences
                == normalized_preferences;
            let next = if retrying_failed_desired {
                path_configuration.desired.clone()
            } else {
                VersionedSearchPathPreferences {
                    revision: expected_revision.checked_add(1).ok_or_else(|| {
                        SearchError::InvalidConfiguration(
                            "search path configuration revision is exhausted".to_owned(),
                        )
                    })?,
                    preferences: normalized_preferences,
                }
            };
            (
                next,
                path_configuration.mounts.clone(),
                path_configuration.effective_config.clone(),
                path_configuration.policy.clone(),
            )
        };
        self.path_store
            .as_ref()
            .expect("Home-backed daemon must own a search path sidecar")
            .replace(&next)?;

        {
            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.desired = next.clone();
            path_configuration.phase = SearchPathConfigurationPhase::Applying;
            self.update_path_configuration_status(path_configuration_status(&path_configuration));
        }

        let (should_maintain, previous_maintenance) = {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            (
                runtime_state.maintenance_requested,
                runtime_state.maintenance.take(),
            )
        };
        if let Some(maintenance) = previous_maintenance {
            if let Err(error) = stop_active_maintenance(maintenance) {
                self.fail_path_configuration(error.to_string());
                self.restore_maintenance_after_failed_transition(should_maintain, previous_config);
                return Err(error);
            }
        }

        let policy_change = previous_policy
            .as_ref()
            .map(|previous| previous.change_to(&policy))
            .unwrap_or_else(|| policy.change_to(&policy));
        let transition = (|| {
            let observed_mounts = observe_root_mounts(policy.roots())?;
            let invalidated_roots =
                invalidated_roots(policy.roots(), &previous_mounts, &observed_mounts);
            let mounts = retained_root_mounts(policy.roots(), &previous_mounts, &observed_mounts);
            let unavailable_roots =
                unavailable_roots_for_mounts(policy.roots(), &mounts, &observed_mounts);
            let effective_config =
                index_config_for_policy(&self.base_config, &policy, unavailable_roots.clone());
            self.writer.apply_path_configuration_transition(
                next.clone(),
                policy.clone(),
                mounts.clone(),
                invalidated_roots,
                policy_change.affected_scopes.clone(),
            )?;
            Ok::<_, SearchError>((mounts, unavailable_roots, effective_config))
        })();
        let (mounts, unavailable_roots, effective_config) = match transition {
            Ok(transition) => transition,
            Err(error) => {
                self.fail_path_configuration(error.to_string());
                self.restore_maintenance_after_failed_transition(should_maintain, previous_config);
                return Err(error);
            }
        };

        {
            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.effective = next.clone();
            path_configuration.policy = Some(policy);
            path_configuration.effective_config = effective_config.clone();
            path_configuration.mounts = mounts;
            path_configuration.unavailable_roots = unavailable_roots;
            path_configuration.pending_frontiers = policy_change.newly_included_frontiers.clone();
            path_configuration.phase = SearchPathConfigurationPhase::Applying;
            self.update_path_configuration_status(path_configuration_status(&path_configuration));
        }

        if should_maintain {
            match spawn_active_maintenance(
                Arc::clone(&self.writer),
                effective_config,
                policy_change.newly_included_frontiers.clone(),
                self.database_path.clone(),
                Arc::clone(&self.lifecycle_snapshot),
                Arc::clone(&self.directory_snapshot_epoch),
            ) {
                Ok((maintenance, maintenance_failure)) => {
                    let mut runtime_state = self
                        .runtime_state
                        .lock()
                        .expect("search daemon runtime mutex poisoned");
                    runtime_state.phase = DaemonRuntimePhase::Maintaining;
                    runtime_state.maintenance = Some(maintenance);
                    drop(runtime_state);
                    self.path_configuration
                        .lock()
                        .expect("search path configuration mutex poisoned")
                        .pending_frontiers
                        .clear();
                    match maintenance_failure {
                        Some(error) => self.record_maintenance_failure(error.to_string()),
                        None => self
                            .lifecycle_snapshot
                            .lock()
                            .expect("search daemon lifecycle mutex poisoned")
                            .clear_maintenance_failure(),
                    }
                }
                Err(error) => {
                    self.runtime_state
                        .lock()
                        .expect("search daemon runtime mutex poisoned")
                        .phase = DaemonRuntimePhase::Created;
                    self.fail_path_configuration(error.to_string());
                    self.record_maintenance_failure(error.to_string());
                    return Err(error);
                }
            }
        }
        Ok(next)
    }

    fn reconcile_root_mounts(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        let (policy, effective, previous_mounts, previous_unavailable, effective_config) = {
            let path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            let Some(policy) = path_configuration.policy.clone() else {
                return Ok(());
            };
            (
                policy,
                path_configuration.effective.clone(),
                path_configuration.mounts.clone(),
                path_configuration.unavailable_roots.clone(),
                path_configuration.effective_config.clone(),
            )
        };
        let observed_mounts = observe_root_mounts(policy.roots())?;
        let mounts = retained_root_mounts(policy.roots(), &previous_mounts, &observed_mounts);
        let current_unavailable =
            unavailable_roots_for_mounts(policy.roots(), &mounts, &observed_mounts);
        let mount_changes = changed_mount_roots(policy.roots(), &previous_mounts, &observed_mounts);
        let newly_unavailable = current_unavailable
            .iter()
            .filter(|root| !previous_unavailable.contains(root))
            .cloned()
            .collect::<Vec<_>>();
        let availability_changed = current_unavailable != previous_unavailable;
        if mount_changes.is_empty() && !availability_changed {
            return Ok(());
        }
        let mut invalidated = mount_changes;
        for root in newly_unavailable {
            if !invalidated.contains(&root) {
                invalidated.push(root);
            }
        }

        let next_effective_config =
            index_config_for_policy(&self.base_config, &policy, current_unavailable.clone());
        let (should_maintain, maintenance) = {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            (
                runtime_state.maintenance_requested,
                runtime_state.maintenance.take(),
            )
        };
        if let Some(maintenance) = maintenance {
            if let Err(error) = stop_active_maintenance(maintenance) {
                self.fail_path_configuration(error.to_string());
                self.restore_maintenance_after_failed_transition(should_maintain, effective_config);
                return Err(error);
            }
        }
        if let Err(error) = self.writer.apply_path_configuration_transition(
            effective,
            policy.clone(),
            mounts.clone(),
            invalidated,
            Vec::new(),
        ) {
            self.fail_path_configuration(error.to_string());
            self.restore_maintenance_after_failed_transition(should_maintain, effective_config);
            return Err(error);
        }
        {
            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.mounts = mounts;
            path_configuration.unavailable_roots = current_unavailable;
            path_configuration.effective_config = next_effective_config.clone();
            path_configuration.phase = SearchPathConfigurationPhase::Applying;
            self.update_path_configuration_status(path_configuration_status(&path_configuration));
        }
        if should_maintain {
            match spawn_active_maintenance(
                Arc::clone(&self.writer),
                next_effective_config,
                Vec::new(),
                self.database_path.clone(),
                Arc::clone(&self.lifecycle_snapshot),
                Arc::clone(&self.directory_snapshot_epoch),
            ) {
                Ok((maintenance, maintenance_failure)) => {
                    self.runtime_state
                        .lock()
                        .expect("search daemon runtime mutex poisoned")
                        .maintenance = Some(maintenance);
                    if let Some(error) = maintenance_failure {
                        self.record_maintenance_failure(error.to_string());
                    }
                }
                Err(error) => {
                    self.runtime_state
                        .lock()
                        .expect("search daemon runtime mutex poisoned")
                        .phase = DaemonRuntimePhase::Created;
                    self.record_maintenance_failure(error.to_string());
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub fn open_query_reader(&self) -> SearchResult<SearchDatabase> {
        self.reconcile_root_mounts()?;
        SearchDatabase::open_read_only(&self.database_path)
    }

    pub(crate) fn record_maintenance_failure(&self, message: String) {
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .record_maintenance_failure(message);
    }

    pub fn shutdown(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        let maintenance = {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            runtime_state.phase = DaemonRuntimePhase::Stopped;
            runtime_state.maintenance_requested = false;
            runtime_state.maintenance.take()
        };
        let maintenance_outcome = match maintenance {
            Some(maintenance) => stop_active_maintenance(maintenance),
            None => Ok(()),
        };
        self.writer.cancel_index_maintenance();
        let writer_outcome = self.writer.shutdown();
        maintenance_outcome.and(writer_outcome)
    }

    fn update_path_configuration_status(&self, status: SearchPathConfigurationStatus) {
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .update_path_configuration(status);
    }

    fn fail_path_configuration(&self, message: String) {
        let status = {
            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.phase = SearchPathConfigurationPhase::Failed { message };
            path_configuration_status(&path_configuration)
        };
        self.update_path_configuration_status(status);
    }

    fn restore_maintenance_after_failed_transition(
        &self,
        should_maintain: bool,
        config: SearchIndexConfig,
    ) {
        if !should_maintain {
            return;
        }
        match spawn_active_maintenance(
            Arc::clone(&self.writer),
            config,
            Vec::new(),
            self.database_path.clone(),
            Arc::clone(&self.lifecycle_snapshot),
            Arc::clone(&self.directory_snapshot_epoch),
        ) {
            Ok((maintenance, maintenance_failure)) => {
                let mut runtime_state = self
                    .runtime_state
                    .lock()
                    .expect("search daemon runtime mutex poisoned");
                runtime_state.phase = DaemonRuntimePhase::Maintaining;
                runtime_state.maintenance = Some(maintenance);
                drop(runtime_state);
                if let Some(error) = maintenance_failure {
                    self.record_maintenance_failure(error.to_string());
                }
            }
            Err(error) => {
                self.runtime_state
                    .lock()
                    .expect("search daemon runtime mutex poisoned")
                    .phase = DaemonRuntimePhase::Created;
                self.record_maintenance_failure(error.to_string());
            }
        }
    }
}

impl Drop for SearchDaemonCore {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
fn search_path_store_for_database(database_path: &Path) -> SearchResult<SearchPathStore> {
    Ok(SearchPathStore::at(
        database_path.with_file_name("search-paths.json"),
    ))
}

#[cfg(not(test))]
fn search_path_store_for_database(_database_path: &Path) -> SearchResult<SearchPathStore> {
    SearchPathStore::from_environment()
}

fn initialize_path_configuration(
    database: &SearchDatabase,
    base_config: &SearchIndexConfig,
    home_directory: Option<&Path>,
    path_store: Option<&SearchPathStore>,
) -> SearchResult<PathConfigurationRuntime> {
    let Some(home_directory) = home_directory else {
        let initial = VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        };
        let effective = database.initialize_search_path_configuration(&initial)?;
        return Ok(PathConfigurationRuntime {
            desired: effective.clone(),
            effective,
            policy: None,
            effective_config: base_config.clone(),
            mounts: Vec::new(),
            unavailable_roots: Vec::new(),
            pending_frontiers: Vec::new(),
            phase: SearchPathConfigurationPhase::Ready,
        });
    };
    let existing = database.read_search_path_configuration()?;
    let previous_mounts = database.read_search_root_mounts()?;
    let path_store = path_store.expect("Home-backed daemon must own a search path sidecar");
    let desired = match path_store.load() {
        Ok(Some(desired)) => desired,
        Ok(None) => VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        },
        Err(error) => {
            return failed_path_configuration_runtime(
                base_config,
                home_directory,
                None,
                existing,
                previous_mounts,
                error.to_string(),
            );
        }
    };
    let desired = match existing.as_ref() {
        Some(effective) if desired.revision <= effective.revision && desired != *effective => {
            if let Err(error) = path_store.replace(effective) {
                return failed_path_configuration_runtime(
                    base_config,
                    home_directory,
                    Some(effective.clone()),
                    existing,
                    previous_mounts,
                    error.to_string(),
                );
            }
            effective.clone()
        }
        _ => desired,
    };
    let policy =
        match SearchPathPolicy::new(home_directory.to_path_buf(), desired.preferences.clone()) {
            Ok(policy) => policy,
            Err(message) => {
                return failed_path_configuration_runtime(
                    base_config,
                    home_directory,
                    Some(desired),
                    existing,
                    previous_mounts,
                    message,
                );
            }
        };
    let previous_effective = existing.clone().unwrap_or(VersionedSearchPathPreferences {
        revision: 0,
        preferences: SearchPathPreferences::default(),
    });
    let previous_policy = SearchPathPolicy::new(
        home_directory.to_path_buf(),
        previous_effective.preferences.clone(),
    )
    .map_err(SearchError::InvalidConfiguration)?;
    let policy_change = previous_policy.change_to(&policy);
    let observed_mounts = observe_root_mounts(policy.roots())?;
    let invalidated = invalidated_roots(policy.roots(), &previous_mounts, &observed_mounts);
    let mounts = retained_root_mounts(policy.roots(), &previous_mounts, &observed_mounts);
    if existing.as_ref() != Some(&desired) || previous_mounts != mounts || !invalidated.is_empty() {
        database.apply_search_path_transition(
            &desired,
            &policy,
            &mounts,
            &invalidated,
            &policy_change.affected_scopes,
        )?;
    }
    let unavailable = unavailable_roots_for_mounts(policy.roots(), &mounts, &observed_mounts);
    Ok(PathConfigurationRuntime {
        desired: desired.clone(),
        effective: desired,
        effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
        policy: Some(policy),
        mounts,
        unavailable_roots: unavailable,
        pending_frontiers: policy_change.newly_included_frontiers,
        phase: SearchPathConfigurationPhase::Ready,
    })
}

fn failed_path_configuration_runtime(
    base_config: &SearchIndexConfig,
    home_directory: &Path,
    desired: Option<VersionedSearchPathPreferences>,
    effective: Option<VersionedSearchPathPreferences>,
    previous_mounts: Vec<SearchRootMount>,
    message: String,
) -> SearchResult<PathConfigurationRuntime> {
    let Some(effective) = effective else {
        return Err(SearchError::InvalidConfiguration(message));
    };
    let policy = SearchPathPolicy::new(home_directory.to_path_buf(), effective.preferences.clone())
        .map_err(SearchError::InvalidConfiguration)?;
    let observed_mounts = observe_root_mounts(policy.roots())?;
    let mounts = if previous_mounts.is_empty() {
        observed_mounts.clone()
    } else {
        retained_root_mounts(policy.roots(), &previous_mounts, &observed_mounts)
    };
    let unavailable = unavailable_roots_for_mounts(policy.roots(), &mounts, &observed_mounts);
    Ok(PathConfigurationRuntime {
        desired: desired.unwrap_or_else(|| effective.clone()),
        effective,
        effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
        policy: Some(policy),
        mounts,
        unavailable_roots: unavailable,
        pending_frontiers: Vec::new(),
        phase: SearchPathConfigurationPhase::Failed { message },
    })
}

fn index_config_for_policy(
    base_config: &SearchIndexConfig,
    policy: &SearchPathPolicy,
    unavailable_roots: Vec<PathBuf>,
) -> SearchIndexConfig {
    let mut config = base_config.clone();
    config.roots = policy.roots().to_vec();
    config.excluded_paths = policy.preferences().exclusions.clone();
    config.unavailable_roots = unavailable_roots;
    config
}

fn unavailable_roots_for_mounts(
    roots: &[PathBuf],
    expected_mounts: &[SearchRootMount],
    observed_mounts: &[SearchRootMount],
) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| {
            if root_is_unavailable(root) {
                return true;
            }
            let expected = expected_mounts
                .iter()
                .find(|mount| &mount.root_path == *root);
            let observed = observed_mounts
                .iter()
                .find(|mount| &mount.root_path == *root);
            expected.zip(observed).is_none_or(|(expected, observed)| {
                expected.mount_point != observed.mount_point || expected.device != observed.device
            })
        })
        .cloned()
        .collect()
}

fn root_is_unavailable(root: &Path) -> bool {
    std::fs::symlink_metadata(root)
        .map(|metadata| !metadata.file_type().is_dir())
        .unwrap_or(true)
}

fn retained_root_mounts(
    roots: &[PathBuf],
    previous_mounts: &[SearchRootMount],
    observed_mounts: &[SearchRootMount],
) -> Vec<SearchRootMount> {
    roots
        .iter()
        .filter_map(|root| {
            let observed = observed_mounts
                .iter()
                .find(|mount| &mount.root_path == root)?;
            let Some(previous) = previous_mounts
                .iter()
                .find(|mount| &mount.root_path == root)
            else {
                return Some(observed.clone());
            };
            let root_unavailable = root_is_unavailable(root);
            let observed_is_shallower = observed.mount_point.components().count()
                < previous.mount_point.components().count();
            Some(if root_unavailable || observed_is_shallower {
                previous.clone()
            } else {
                observed.clone()
            })
        })
        .collect()
}

fn changed_mount_roots(
    roots: &[PathBuf],
    previous_mounts: &[SearchRootMount],
    current_mounts: &[SearchRootMount],
) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| {
            previous_mounts
                .iter()
                .find(|mount| &mount.root_path == *root)
                .zip(
                    current_mounts
                        .iter()
                        .find(|mount| &mount.root_path == *root),
                )
                .is_some_and(|(previous, current)| previous != current)
        })
        .cloned()
        .collect()
}

fn invalidated_roots(
    roots: &[PathBuf],
    previous_mounts: &[SearchRootMount],
    current_mounts: &[SearchRootMount],
) -> Vec<PathBuf> {
    let mut invalidated = changed_mount_roots(roots, previous_mounts, current_mounts);
    for root in roots.iter().filter(|root| root_is_unavailable(root)) {
        if !invalidated.contains(root) {
            invalidated.push(root.clone());
        }
    }
    invalidated
}

fn path_configuration_status(
    path_configuration: &PathConfigurationRuntime,
) -> SearchPathConfigurationStatus {
    let roots = path_configuration
        .policy
        .as_ref()
        .map(|policy| {
            root_statuses(
                &policy.preferences().custom_roots,
                &path_configuration.mounts,
            )
        })
        .unwrap_or_default();
    SearchPathConfigurationStatus {
        desired_revision: path_configuration.desired.revision,
        effective_revision: path_configuration.effective.revision,
        effective_preferences: path_configuration.effective.preferences.clone(),
        phase: path_configuration.phase.clone(),
        roots,
    }
}

fn root_statuses(roots: &[PathBuf], expected_mounts: &[SearchRootMount]) -> Vec<SearchRootStatus> {
    root_statuses_from_mount_observation(roots, expected_mounts, observe_root_mounts(roots))
}

fn root_statuses_from_mount_observation(
    roots: &[PathBuf],
    expected_mounts: &[SearchRootMount],
    mount_observation: SearchResult<Vec<SearchRootMount>>,
) -> Vec<SearchRootStatus> {
    let observed_mounts = match mount_observation {
        Ok(mounts) => mounts,
        Err(error) => {
            let message = format!("storage identity could not be verified: {error}");
            return roots
                .iter()
                .map(|root| SearchRootStatus {
                    path: root.clone(),
                    availability: SearchRootAvailability::Unavailable {
                        message: message.clone(),
                    },
                })
                .collect();
        }
    };
    roots
        .iter()
        .map(|root| {
            let availability = match std::fs::symlink_metadata(root) {
                Ok(metadata) if !metadata.file_type().is_dir() => {
                    SearchRootAvailability::Unavailable {
                        message: "configured search root is not a no-follow directory".to_owned(),
                    }
                }
                Err(error) => SearchRootAvailability::Unavailable {
                    message: error.to_string(),
                },
                Ok(_) => {
                    let expected = expected_mounts
                        .iter()
                        .find(|mount| mount.root_path == *root);
                    let observed = observed_mounts
                        .iter()
                        .find(|mount| mount.root_path == *root);
                    match expected.zip(observed) {
                        Some((expected, observed))
                            if expected.mount_point != observed.mount_point
                                || expected.device != observed.device =>
                        {
                            SearchRootAvailability::MountChanged {
                                message:
                                    "configured search root now resolves to a different filesystem"
                                        .to_owned(),
                            }
                        }
                        Some(_) => SearchRootAvailability::Available,
                        None => SearchRootAvailability::Unavailable {
                            message: "storage identity has not been confirmed".to_owned(),
                        },
                    }
                }
            };
            SearchRootStatus {
                path: root.clone(),
                availability,
            }
        })
        .collect()
}

fn spawn_active_maintenance(
    writer: Arc<IndexWriter>,
    config: SearchIndexConfig,
    newly_included_frontiers: Vec<PathBuf>,
    database_path: PathBuf,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    directory_snapshot_epoch: Arc<AtomicU64>,
) -> SearchResult<(ActiveMaintenance, Option<SearchError>)> {
    let work_queue = Arc::new(DaemonWorkQueue::new(
        config.available_roots().cloned().collect(),
    ));
    let crawl_cancellation = CancellationToken::new();
    let worker_join = spawn_daemon_worker(
        writer,
        config.clone(),
        Arc::clone(&lifecycle_snapshot),
        Arc::clone(&work_queue),
        Arc::clone(&directory_snapshot_epoch),
        crawl_cancellation.clone(),
    )?;
    let (watch_ingress, maintenance_failure) = match DaemonWatchIngressBootstrap::establish(
        config,
        database_path,
        directory_snapshot_epoch,
    )
    .and_then(|bootstrap| bootstrap.spawn(Arc::clone(&work_queue), lifecycle_snapshot))
    {
        Ok(watch_ingress) => (Some(watch_ingress), None),
        Err(error) => (None, Some(error)),
    };
    if let Err(error) = work_queue.enqueue(DaemonWorkRequest::StartupCheck) {
        crawl_cancellation.cancel();
        work_queue.begin_shutdown();
        let _ = worker_join.join();
        return Err(error);
    }
    if !newly_included_frontiers.is_empty() {
        if let Err(error) = work_queue.enqueue(DaemonWorkRequest::ChangedPaths {
            changed_paths: newly_included_frontiers,
        }) {
            crawl_cancellation.cancel();
            work_queue.begin_shutdown();
            let _ = worker_join.join();
            return Err(error);
        }
    }
    Ok((
        ActiveMaintenance {
            work_queue,
            crawl_cancellation,
            worker_join,
            watch_ingress,
        },
        maintenance_failure,
    ))
}

fn stop_active_maintenance(maintenance: ActiveMaintenance) -> SearchResult<()> {
    if let Some(watch_ingress) = maintenance.watch_ingress {
        watch_ingress.shutdown();
    }
    maintenance.crawl_cancellation.cancel();
    maintenance.work_queue.begin_shutdown();
    maintenance
        .worker_join
        .join()
        .map_err(|_| SearchError::WorkerFailed("search daemon worker thread panicked".to_owned()))?
}

fn spawn_daemon_worker(
    writer: Arc<IndexWriter>,
    config: SearchIndexConfig,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    work_queue: Arc<DaemonWorkQueue>,
    directory_snapshot_epoch: Arc<AtomicU64>,
    crawl_cancellation: CancellationToken,
) -> SearchResult<thread::JoinHandle<SearchResult<()>>> {
    thread::Builder::new()
        .name("file-search-daemon-core".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    SearchError::WorkerFailed(format!(
                        "could not create search daemon runtime: {error}"
                    ))
                })?;
            loop {
                let work_request = work_queue.wait_for_next_work();
                if matches!(work_request, DaemonWorkRequest::Shutdown) {
                    break;
                }
                let dirty_roots = match &work_request {
                    DaemonWorkRequest::DirtyRootRecovery { roots } => Some(roots.clone()),
                    _ => None,
                };
                let watch_budget_patrol =
                    matches!(&work_request, DaemonWorkRequest::WatchBudgetPatrol { .. });
                let succeeded = run_index_maintenance(
                    &runtime,
                    Arc::clone(&writer),
                    &config,
                    Arc::clone(&lifecycle_snapshot),
                    &directory_snapshot_epoch,
                    work_request,
                    &crawl_cancellation,
                );
                if watch_budget_patrol {
                    work_queue.finish_watch_budget_patrol();
                }
                if let Some(roots) = dirty_roots {
                    work_queue.finish_dirty_root_recovery(&roots, succeeded);
                }
            }
            Ok(())
        })
        .map_err(|error| {
            SearchError::WorkerFailed(format!("could not spawn search daemon worker: {error}"))
        })
}

fn run_index_maintenance(
    runtime: &tokio::runtime::Runtime,
    writer: Arc<IndexWriter>,
    config: &SearchIndexConfig,
    lifecycle_snapshot: Arc<Mutex<DaemonLifecycleSnapshot>>,
    directory_snapshot_epoch: &AtomicU64,
    work_request: DaemonWorkRequest,
    crawl_cancellation: &CancellationToken,
) -> bool {
    let work_label = work_request_label(&work_request);
    let high_frequency_maintenance = matches!(
        &work_request,
        DaemonWorkRequest::ChangedPaths { .. } | DaemonWorkRequest::WatchBudgetPatrol { .. }
    );
    lifecycle_snapshot
        .lock()
        .expect("search daemon lifecycle mutex poisoned")
        .begin_checking();

    let maintenance_outcome = runtime.block_on(async {
        let indexer = SearchIndexer::new(Arc::clone(&writer), config.clone());
        let progress_snapshot = Arc::clone(&lifecycle_snapshot);
        let on_progress = move |progress| {
            progress_snapshot
                .lock()
                .expect("search daemon lifecycle mutex poisoned")
                .apply_progress(progress);
        };

        let stats = match work_request {
            DaemonWorkRequest::StartupCheck => {
                indexer
                    .rebuild_with_progress_cancelled(crawl_cancellation, on_progress)
                    .await?
            }
            DaemonWorkRequest::ChangedPaths { changed_paths } => {
                indexer
                    .rebuild_paths_with_progress_cancelled(
                        changed_paths,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::CoverageRepair { scopes } => {
                indexer
                    .repair_scopes_with_progress_cancelled(scopes, crawl_cancellation, on_progress)
                    .await?
            }
            DaemonWorkRequest::WatchBudgetPatrol { directories } => {
                indexer
                    .patrol_unwatched_directories_with_progress_cancelled(
                        directories,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::DirtyRootRecovery { roots } => {
                indexer
                    .recover_dirty_roots_with_progress_cancelled(
                        roots,
                        crawl_cancellation,
                        on_progress,
                    )
                    .await?
            }
            DaemonWorkRequest::Shutdown => return Ok::<_, SearchError>((Default::default(), 0)),
        };
        let total = writer.count()?;
        Ok::<_, SearchError>((stats, total))
    });
    release_allocator_idle_pages();
    let mut snapshot = lifecycle_snapshot
        .lock()
        .expect("search daemon lifecycle mutex poisoned");
    match maintenance_outcome {
        Ok((stats, total)) => {
            if stats.directory_snapshots_changed > 0 {
                directory_snapshot_epoch.fetch_add(1, Ordering::AcqRel);
            }
            if high_frequency_maintenance {
                tracing::debug!(
                    target: "file_search::daemon",
                    event = "index_maintenance_completed",
                    maintenance = %work_label,
                    checked = stats.checked,
                    changed = stats.changed,
                    reindexed = stats.reindexed,
                    directories_enumerated = stats.directories_enumerated,
                    database_mutations = stats.database_mutations,
                    content_reads = stats.content_reads,
                    indexed_files = total,
                    "index maintenance completed"
                );
            } else {
                tracing::info!(
                    target: "file_search::daemon",
                    event = "index_maintenance_completed",
                    maintenance = %work_label,
                    checked = stats.checked,
                    changed = stats.changed,
                    reindexed = stats.reindexed,
                    directories_enumerated = stats.directories_enumerated,
                    database_mutations = stats.database_mutations,
                    content_reads = stats.content_reads,
                    indexed_files = total,
                    "index maintenance completed"
                );
            }
            snapshot.finish_watch_cycle(total);
            true
        }
        Err(_) if crawl_cancellation.is_cancelled() => false,
        Err(SearchError::Cancelled) => false,
        Err(error) => {
            let error = error.to_string();
            let log_error = bounded_search_log_detail(&error);
            tracing::error!(
                target: "file_search::daemon",
                event = "index_maintenance_failed",
                maintenance = %work_label,
                error = %log_error,
                "index maintenance failed"
            );
            snapshot.record_error(error);
            false
        }
    }
}

fn work_request_label(work_request: &DaemonWorkRequest) -> String {
    match work_request {
        DaemonWorkRequest::StartupCheck => "startup check".to_owned(),
        DaemonWorkRequest::ChangedPaths { changed_paths } => {
            format!("changed path batch ({})", changed_paths.len())
        }
        DaemonWorkRequest::CoverageRepair { scopes } => {
            format!("coverage repair ({})", scopes.len())
        }
        DaemonWorkRequest::WatchBudgetPatrol { directories } => {
            format!("watch budget patrol ({})", directories.len())
        }
        DaemonWorkRequest::DirtyRootRecovery { roots } => {
            format!("dirty root recovery ({})", roots.len())
        }
        DaemonWorkRequest::Shutdown => "shutdown".to_owned(),
    }
}

fn daemon_core_stopped() -> SearchError {
    SearchError::WorkerFailed("search daemon core is no longer running".to_owned())
}

#[cfg(test)]
#[path = "daemon/tests.rs"]
mod tests;
