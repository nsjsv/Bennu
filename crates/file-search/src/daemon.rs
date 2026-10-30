use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::database::{SearchDatabase, SearchRootMount};
use crate::error::{SearchError, SearchResult};
use crate::logging::bounded_search_log_detail;
use crate::managed_search_index::ManagedSearchIndex;
use crate::model::{IndexStatus, SearchPathConfigurationPhase};
use crate::search_path_store::SearchPathStore;
use crate::writer::IndexWriter;
use crate::{
    SearchIndexConfig, SearchPathPolicy, SearchPathPreferences, VersionedSearchPathPreferences,
};

mod bounded_paths;
mod lifecycle;
mod maintenance;
mod memory;
mod path_configuration;
mod root_monitor;
mod root_mount;
mod watch_budget_patrol;
mod watch_coverage;
mod watch_ingress;
mod work_queue;

#[cfg(test)]
use self::bounded_paths::BoundedPathSet;
use self::lifecycle::DaemonLifecycleSnapshot;
#[cfg(test)]
use self::lifecycle::IndexCompleteness;
use self::maintenance::{spawn_active_maintenance, stop_active_maintenance, ActiveMaintenance};
use self::path_configuration::{
    changed_mount_roots, index_config_for_policy, initialize_path_configuration, invalidated_roots,
    retained_root_mounts, unavailable_roots_for_mounts, PathConfigurationRuntime,
};
use self::root_monitor::{RootAvailabilityMonitor, RootChange};
use self::root_mount::observe_root_mounts;
#[cfg(test)]
use self::watch_ingress::changed_paths_from_watch_event;
#[cfg(test)]
use self::work_queue::{
    dirty_root_retry_delay, PendingDaemonWork, WorkOutcome, DIRTY_ROOT_QUIET_WINDOW,
    DIRTY_ROOT_RETRY_BASE, DIRTY_ROOT_RETRY_MAX,
};
use self::work_queue::{DaemonWorkQueue, DaemonWorkRequest};

#[derive(Debug, Default, PartialEq, Eq)]
enum DaemonRuntimePhase {
    #[default]
    Created,
    Maintaining,
    Stopped,
}

#[derive(Default)]
struct DaemonRuntimeState {
    phase: DaemonRuntimePhase,
    maintenance_requested: bool,
    maintenance: Option<ActiveMaintenance>,
}

/// 搜索服务核心。查状态、开查询连接、读路径配置都是纯读；会改状态的操作
/// （启动维护、改搜索位置、关机）交给 `DaemonState`，挂载和文件夹变化由监视线程交给它。
pub struct SearchDaemonCore {
    state: Arc<DaemonState>,
    /// 监视线程拿着 `state` 并会去拿操作锁，所以由外层持有，并且关机时先于维护停掉。
    root_monitor: Mutex<Option<RootAvailabilityMonitor>>,
}

impl SearchDaemonCore {
    /// `search_path_file` 是搜索路径偏好文件的位置,由调用方解析(正式入口见
    /// `bin/file-searchd.rs`,位置规则见 `search_path_file_in`)。
    pub fn new(
        database_path: PathBuf,
        search_path_file: PathBuf,
        config: SearchIndexConfig,
    ) -> SearchResult<Self> {
        Ok(Self {
            state: Arc::new(DaemonState::open(database_path, search_path_file, config)?),
            root_monitor: Mutex::new(None),
        })
    }

    pub fn start_index_maintenance(&self) -> SearchResult<()> {
        let mut root_monitor = self
            .root_monitor
            .lock()
            .expect("search root monitor mutex poisoned");
        self.state.start_requested_maintenance()?;
        if root_monitor.is_none() {
            match RootAvailabilityMonitor::spawn(Arc::clone(&self.state)) {
                Ok(monitor) => *root_monitor = Some(monitor),
                Err(error) => self.state.record_root_monitor_failure(format!(
                    "search roots are not monitored: {error}"
                )),
            }
        }
        Ok(())
    }

    pub fn current_status(&self) -> IndexStatus {
        self.state
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .to_index_status()
    }

    pub fn current_path_preferences(&self) -> VersionedSearchPathPreferences {
        self.state
            .path_configuration
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
        let configured = self
            .state
            .configure_search_paths(expected_revision, preferences);
        if let Some(monitor) = self
            .root_monitor
            .lock()
            .expect("search root monitor mutex poisoned")
            .as_ref()
        {
            monitor.follow_new_roots();
        }
        configured
    }

    pub fn open_query_reader(&self) -> SearchResult<SearchDatabase> {
        SearchDatabase::open_read_only(&self.state.database_path)
    }

    /// 给服务运行时的测试模拟维护后端坏掉。
    #[cfg(test)]
    pub(crate) fn record_maintenance_failure(&self, message: String) {
        self.state
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .record_maintenance_failure(message);
    }

    pub fn shutdown(&self) -> SearchResult<()> {
        let root_monitor = self
            .root_monitor
            .lock()
            .expect("search root monitor mutex poisoned")
            .take();
        if let Some(monitor) = root_monitor {
            monitor.stop();
        }
        self.state.shutdown()
    }
}

impl Drop for SearchDaemonCore {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// 会改状态的操作都在 `operation_serialization` 下串行执行。
struct DaemonState {
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

impl DaemonState {
    fn open(
        database_path: PathBuf,
        search_path_file: PathBuf,
        config: SearchIndexConfig,
    ) -> SearchResult<Self> {
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
            .map(|_| SearchPathStore::at(search_path_file));
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
        let (path_configuration, path_configuration_phase) = initialize_path_configuration(
            &managed_index_open.database,
            &config,
            home_directory.as_deref(),
            path_store.as_ref(),
        )?;
        let writer = Arc::new(IndexWriter::spawn(managed_index_open.database));
        let known_count = writer.count()?;
        let mut lifecycle = DaemonLifecycleSnapshot::starting(
            known_count,
            recovery_rebuild_message,
            path_configuration_phase,
        );
        lifecycle.publish_path_configuration(
            path_configuration.desired.revision,
            &path_configuration.effective,
        );
        lifecycle.publish_root_statuses(path_configuration.root_statuses.clone());

        Ok(Self {
            database_path,
            home_directory,
            base_config: config,
            path_store,
            path_configuration: Mutex::new(path_configuration),
            writer,
            lifecycle_snapshot: Arc::new(Mutex::new(lifecycle)),
            directory_snapshot_epoch: Arc::new(AtomicU64::new(0)),
            runtime_state: Mutex::new(DaemonRuntimeState::default()),
            operation_serialization: Mutex::new(()),
        })
    }

    fn start_requested_maintenance(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            match runtime_state.phase {
                DaemonRuntimePhase::Maintaining => return Ok(()),
                DaemonRuntimePhase::Stopped => return Err(daemon_core_stopped()),
                DaemonRuntimePhase::Created => runtime_state.maintenance_requested = true,
            }
        }
        self.start_maintenance()
    }

    fn configure_search_paths(
        &self,
        expected_revision: u64,
        preferences: SearchPathPreferences,
    ) -> SearchResult<VersionedSearchPathPreferences> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        if self.stopped() {
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
        let (next, previous_mounts, previous_policy) = {
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
                self.lifecycle_snapshot
                    .lock()
                    .expect("search daemon lifecycle mutex poisoned")
                    .path_configuration_phase(),
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
            self.publish_path_configuration(&path_configuration);
            self.lifecycle_snapshot
                .lock()
                .expect("search daemon lifecycle mutex poisoned")
                .begin_applying_path_configuration();
        }

        let policy_change = previous_policy
            .as_ref()
            .map(|previous| previous.change_to(&policy))
            .unwrap_or_else(|| policy.change_to(&policy));
        self.replace_maintenance(|| {
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

            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.effective = next.clone();
            path_configuration.policy = Some(policy.clone());
            path_configuration.effective_config = effective_config;
            path_configuration.mounts = mounts;
            path_configuration.unavailable_roots = unavailable_roots;
            path_configuration.pending_frontiers = policy_change.newly_included_frontiers.clone();
            path_configuration.refresh_root_statuses(&Ok(observed_mounts));
            self.publish_path_configuration(&path_configuration);
            Ok(())
        })?;
        Ok(next)
    }

    /// 只由监视线程调用：挂载表或某个位置的上级目录变了。
    fn reconcile_root_availability(&self, change: RootChange) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        if self.stopped() {
            return Ok(());
        }
        let (policy, effective, previous_mounts, previous_unavailable) = {
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
            )
        };
        let observed_mounts = match observe_root_mounts(policy.roots()) {
            Ok(observed_mounts) => observed_mounts,
            Err(error) => {
                // 挂载表读不了或有坏行：所有位置报"无法确认"，不做任何配置变化，也不清索引。
                let observation = Err(error);
                self.refresh_root_statuses(&observation);
                return observation.map(|_| ());
            }
        };
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
        let observation = Ok(observed_mounts);
        if mount_changes.is_empty() && !availability_changed {
            self.refresh_root_statuses(&observation);
            if change == RootChange::MountTable {
                self.retry_failed_roots_now();
            }
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
        self.replace_maintenance(|| {
            self.writer.apply_path_configuration_transition(
                effective,
                policy.clone(),
                mounts.clone(),
                invalidated,
                Vec::new(),
            )?;
            let mut path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            path_configuration.mounts = mounts;
            path_configuration.unavailable_roots = current_unavailable;
            path_configuration.effective_config = next_effective_config;
            // 挂载变化不是应用搜索位置：只发布各位置的新状态，"正在应用/失败/就绪"不动。
            // 状态要在重启维护之前发布，否则新位置的结果可能比"可用"先出现。
            path_configuration.refresh_root_statuses(&observation);
            self.lifecycle_snapshot
                .lock()
                .expect("search daemon lifecycle mutex poisoned")
                .publish_root_statuses(path_configuration.root_statuses.clone());
            Ok(())
        })
    }

    fn configured_roots(&self) -> Vec<PathBuf> {
        self.path_configuration
            .lock()
            .expect("search path configuration mutex poisoned")
            .roots()
    }

    fn record_root_monitor_failure(&self, message: String) {
        let log_error = bounded_search_log_detail(&message);
        tracing::error!(
            target: "file_search::daemon",
            event = "root_monitor_failed",
            error = %log_error,
            "search roots are no longer monitored"
        );
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .record_root_monitor_failure(message);
    }

    fn shutdown(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
        {
            let mut runtime_state = self
                .runtime_state
                .lock()
                .expect("search daemon runtime mutex poisoned");
            runtime_state.phase = DaemonRuntimePhase::Stopped;
            runtime_state.maintenance_requested = false;
        }
        let maintenance_outcome = self.stop_maintenance();
        self.writer.cancel_index_maintenance();
        let writer_outcome = self.writer.shutdown();
        maintenance_outcome.and(writer_outcome)
    }

    fn stopped(&self) -> bool {
        self.runtime_state
            .lock()
            .expect("search daemon runtime mutex poisoned")
            .phase
            == DaemonRuntimePhase::Stopped
    }

    fn refresh_root_statuses(&self, mount_observation: &SearchResult<Vec<SearchRootMount>>) {
        let mut path_configuration = self
            .path_configuration
            .lock()
            .expect("search path configuration mutex poisoned");
        path_configuration.refresh_root_statuses(mount_observation);
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .publish_root_statuses(path_configuration.root_statuses.clone());
    }

    fn publish_path_configuration(&self, path_configuration: &PathConfigurationRuntime) {
        let mut lifecycle = self
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned");
        lifecycle.publish_path_configuration(
            path_configuration.desired.revision,
            &path_configuration.effective,
        );
        lifecycle.publish_root_statuses(path_configuration.root_statuses.clone());
    }

    fn fail_path_configuration(&self, message: String) {
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .fail_path_configuration(message);
    }

    fn retry_failed_roots_now(&self) {
        if let Some(maintenance) = self
            .runtime_state
            .lock()
            .expect("search daemon runtime mutex poisoned")
            .maintenance
            .as_ref()
        {
            maintenance.retry_failed_roots_now();
        }
    }

    /// 唯一启动维护的地方。配置和新纳入的前沿目录都从路径配置里取，调用方没有参数可传错；
    /// 启动成功与失败后的状态也只在这里更新。
    fn start_maintenance(&self) -> SearchResult<()> {
        let (config, newly_included_frontiers) = {
            let path_configuration = self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned");
            (
                path_configuration.effective_config.clone(),
                path_configuration.pending_frontiers.clone(),
            )
        };
        match spawn_active_maintenance(
            Arc::clone(&self.writer),
            config,
            newly_included_frontiers,
            self.database_path.clone(),
            Arc::clone(&self.lifecycle_snapshot),
            Arc::clone(&self.directory_snapshot_epoch),
        ) {
            Ok((maintenance, watch_failure)) => {
                {
                    let mut runtime_state = self
                        .runtime_state
                        .lock()
                        .expect("search daemon runtime mutex poisoned");
                    runtime_state.phase = DaemonRuntimePhase::Maintaining;
                    runtime_state.maintenance = Some(maintenance);
                }
                self.path_configuration
                    .lock()
                    .expect("search path configuration mutex poisoned")
                    .pending_frontiers
                    .clear();
                let mut lifecycle = self
                    .lifecycle_snapshot
                    .lock()
                    .expect("search daemon lifecycle mutex poisoned");
                match watch_failure {
                    Some(error) => lifecycle.record_maintenance_failure(error.to_string()),
                    None => lifecycle.clear_maintenance_failure(),
                }
                Ok(())
            }
            Err(error) => {
                self.runtime_state
                    .lock()
                    .expect("search daemon runtime mutex poisoned")
                    .phase = DaemonRuntimePhase::Created;
                let mut lifecycle = self
                    .lifecycle_snapshot
                    .lock()
                    .expect("search daemon lifecycle mutex poisoned");
                lifecycle.record_maintenance_failure(error.to_string());
                // 新的搜索位置要靠维护才能生效，维护起不来就是这次应用失败了。
                if lifecycle.path_configuration_phase() == &SearchPathConfigurationPhase::Applying {
                    lifecycle.fail_path_configuration(error.to_string());
                }
                drop(lifecycle);
                Err(error)
            }
        }
    }

    /// 唯一停止维护的地方。
    fn stop_maintenance(&self) -> SearchResult<()> {
        let maintenance = self
            .runtime_state
            .lock()
            .expect("search daemon runtime mutex poisoned")
            .maintenance
            .take();
        maintenance.map_or(Ok(()), stop_active_maintenance)
    }

    /// 配置变更与挂载变化共用：先停维护，再应用变化，原来在维护就重启。变化失败时
    /// `transition` 没改路径配置，重启用的自然还是旧配置。
    fn replace_maintenance(
        &self,
        transition: impl FnOnce() -> SearchResult<()>,
    ) -> SearchResult<()> {
        let restart = self
            .runtime_state
            .lock()
            .expect("search daemon runtime mutex poisoned")
            .maintenance_requested;
        let transition = self.stop_maintenance().and_then(|()| transition());
        if let Err(error) = &transition {
            self.fail_path_configuration(error.to_string());
        }
        if !restart {
            return transition;
        }
        let restarted = self.start_maintenance();
        transition.and(restarted)
    }
}

fn daemon_core_stopped() -> SearchError {
    SearchError::WorkerFailed("search daemon core is no longer running".to_owned())
}

#[cfg(test)]
#[path = "daemon/tests.rs"]
mod tests;
