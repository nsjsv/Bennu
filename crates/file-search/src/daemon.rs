use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::database::{SearchDatabase, SearchRootMount};
use crate::error::{SearchError, SearchResult};
use crate::logging::bounded_search_log_detail;
use crate::managed_search_index::ManagedSearchIndex;
use crate::model::{
    IndexStatus, SearchPathConfigurationPhase, SearchRootAvailability, SearchRootStatus,
};
use crate::search_path_store::SearchPathStore;
use crate::writer::IndexWriter;
use crate::{
    SearchIndexConfig, SearchPathPolicy, SearchPathPreferences, VersionedSearchPathPreferences,
};

mod bounded_paths;
mod lifecycle;
mod maintenance;
mod memory;
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

struct PathConfigurationRuntime {
    desired: VersionedSearchPathPreferences,
    effective: VersionedSearchPathPreferences,
    policy: Option<SearchPathPolicy>,
    effective_config: SearchIndexConfig,
    mounts: Vec<SearchRootMount>,
    unavailable_roots: Vec<PathBuf>,
    pending_frontiers: Vec<PathBuf>,
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
    /// `search_path_file` 是搜索路径偏好文件的位置,由调用方解析(正式入口见
    /// `bin/file-searchd.rs`,位置规则见 `search_path_file_in`)。
    pub fn new(
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
        lifecycle.publish_root_statuses(configured_root_statuses(&path_configuration));
        let lifecycle_snapshot = Arc::new(Mutex::new(lifecycle));

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

    pub fn current_status(&self) -> IndexStatus {
        let _ = self.reconcile_root_mounts();
        let roots = configured_root_statuses(
            &self
                .path_configuration
                .lock()
                .expect("search path configuration mutex poisoned"),
        );
        let mut lifecycle = self
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned");
        lifecycle.publish_root_statuses(roots);
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
            self.publish_path_configuration(&path_configuration);
            Ok(())
        })?;
        Ok(next)
    }

    fn reconcile_root_mounts(&self) -> SearchResult<()> {
        let _operation = self
            .operation_serialization
            .lock()
            .expect("search daemon operation mutex poisoned");
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
            // 挂载变化不是应用搜索位置：只刷新各位置的状态，"正在应用/失败/就绪"不动。
            self.publish_path_configuration(&path_configuration);
            Ok(())
        })
    }

    pub fn open_query_reader(&self) -> SearchResult<SearchDatabase> {
        self.reconcile_root_mounts()?;
        SearchDatabase::open_read_only(&self.database_path)
    }

    /// 给服务运行时的测试模拟维护后端坏掉。
    #[cfg(test)]
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

    fn publish_path_configuration(&self, path_configuration: &PathConfigurationRuntime) {
        let roots = configured_root_statuses(path_configuration);
        let mut lifecycle = self
            .lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned");
        lifecycle.publish_path_configuration(
            path_configuration.desired.revision,
            &path_configuration.effective,
        );
        lifecycle.publish_root_statuses(roots);
    }

    fn fail_path_configuration(&self, message: String) {
        self.lifecycle_snapshot
            .lock()
            .expect("search daemon lifecycle mutex poisoned")
            .fail_path_configuration(message);
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

impl Drop for SearchDaemonCore {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn initialize_path_configuration(
    database: &SearchDatabase,
    base_config: &SearchIndexConfig,
    home_directory: Option<&Path>,
    path_store: Option<&SearchPathStore>,
) -> SearchResult<(PathConfigurationRuntime, SearchPathConfigurationPhase)> {
    let Some(home_directory) = home_directory else {
        let initial = VersionedSearchPathPreferences {
            revision: 0,
            preferences: SearchPathPreferences::default(),
        };
        let effective = database.initialize_search_path_configuration(&initial)?;
        return Ok((
            PathConfigurationRuntime {
                desired: effective.clone(),
                effective,
                policy: None,
                effective_config: base_config.clone(),
                mounts: Vec::new(),
                unavailable_roots: Vec::new(),
                pending_frontiers: Vec::new(),
            },
            SearchPathConfigurationPhase::Ready,
        ));
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
    Ok((
        PathConfigurationRuntime {
            desired: desired.clone(),
            effective: desired,
            effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
            policy: Some(policy),
            mounts,
            unavailable_roots: unavailable,
            pending_frontiers: policy_change.newly_included_frontiers,
        },
        SearchPathConfigurationPhase::Ready,
    ))
}

fn failed_path_configuration_runtime(
    base_config: &SearchIndexConfig,
    home_directory: &Path,
    desired: Option<VersionedSearchPathPreferences>,
    effective: Option<VersionedSearchPathPreferences>,
    previous_mounts: Vec<SearchRootMount>,
    message: String,
) -> SearchResult<(PathConfigurationRuntime, SearchPathConfigurationPhase)> {
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
    Ok((
        PathConfigurationRuntime {
            desired: desired.unwrap_or_else(|| effective.clone()),
            effective,
            effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
            policy: Some(policy),
            mounts,
            unavailable_roots: unavailable,
            pending_frontiers: Vec::new(),
        },
        SearchPathConfigurationPhase::Failed { message },
    ))
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

fn configured_root_statuses(
    path_configuration: &PathConfigurationRuntime,
) -> Vec<SearchRootStatus> {
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

fn daemon_core_stopped() -> SearchError {
    SearchError::WorkerFailed("search daemon core is no longer running".to_owned())
}

#[cfg(test)]
#[path = "daemon/tests.rs"]
mod tests;
