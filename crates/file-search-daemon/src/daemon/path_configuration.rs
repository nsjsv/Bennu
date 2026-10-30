use std::path::{Path, PathBuf};

use crate::database::{SearchDatabase, SearchRootMount};
use crate::error::{SearchError, SearchResult};
use crate::model::{SearchPathConfigurationPhase, SearchRootAvailability, SearchRootStatus};
use crate::search_path_store::SearchPathStore;
use crate::{
    SearchIndexConfig, SearchPathPolicy, SearchPathPreferences, VersionedSearchPathPreferences,
};

use super::root_mount::observe_root_mounts;

/// 搜索位置的所有者状态，只由 `DaemonState` 在操作锁下改。
pub(super) struct PathConfigurationRuntime {
    pub(super) desired: VersionedSearchPathPreferences,
    pub(super) effective: VersionedSearchPathPreferences,
    pub(super) policy: Option<SearchPathPolicy>,
    pub(super) effective_config: SearchIndexConfig,
    pub(super) mounts: Vec<SearchRootMount>,
    pub(super) unavailable_roots: Vec<PathBuf>,
    pub(super) pending_frontiers: Vec<PathBuf>,
    /// 最近一次挂载观测得出的各自定义位置状态；查状态只读它，不自己观测。
    pub(super) root_statuses: Vec<SearchRootStatus>,
}

impl PathConfigurationRuntime {
    pub(super) fn roots(&self) -> Vec<PathBuf> {
        self.policy
            .as_ref()
            .map(|policy| policy.roots().to_vec())
            .unwrap_or_default()
    }

    pub(super) fn refresh_root_statuses(
        &mut self,
        mount_observation: &SearchResult<Vec<SearchRootMount>>,
    ) {
        self.root_statuses = self
            .policy
            .as_ref()
            .map(|policy| {
                root_statuses_from_mount_observation(
                    &policy.preferences().custom_roots,
                    &self.mounts,
                    mount_observation,
                )
            })
            .unwrap_or_default();
    }
}

pub(super) fn initialize_path_configuration(
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
                root_statuses: Vec::new(),
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
    let mut runtime = PathConfigurationRuntime {
        desired: desired.clone(),
        effective: desired,
        effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
        policy: Some(policy),
        mounts,
        unavailable_roots: unavailable,
        pending_frontiers: policy_change.newly_included_frontiers,
        root_statuses: Vec::new(),
    };
    runtime.refresh_root_statuses(&Ok(observed_mounts));
    Ok((runtime, SearchPathConfigurationPhase::Ready))
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
    let mut runtime = PathConfigurationRuntime {
        desired: desired.unwrap_or_else(|| effective.clone()),
        effective,
        effective_config: index_config_for_policy(base_config, &policy, unavailable.clone()),
        policy: Some(policy),
        mounts,
        unavailable_roots: unavailable,
        pending_frontiers: Vec::new(),
        root_statuses: Vec::new(),
    };
    runtime.refresh_root_statuses(&Ok(observed_mounts));
    Ok((runtime, SearchPathConfigurationPhase::Failed { message }))
}

pub(super) fn index_config_for_policy(
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

pub(super) fn unavailable_roots_for_mounts(
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

pub(super) fn retained_root_mounts(
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

pub(super) fn changed_mount_roots(
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

pub(super) fn invalidated_roots(
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

pub(super) fn root_statuses_from_mount_observation(
    roots: &[PathBuf],
    expected_mounts: &[SearchRootMount],
    mount_observation: &SearchResult<Vec<SearchRootMount>>,
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
