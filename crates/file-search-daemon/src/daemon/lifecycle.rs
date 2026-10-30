use std::path::PathBuf;

use crate::crawler::IndexMaintenanceProgress;
use crate::model::{
    ExtractorCapability, IndexHealth, IndexPhase, IndexStatus, SearchPathConfigurationPhase,
    SearchPathConfigurationStatus, SearchRootStatus,
};
use crate::VersionedSearchPathPreferences;

use super::watch_coverage::WatchCoverageHealth;

/// 当前正在做的那件维护工作。
#[derive(Debug, Clone, PartialEq, Eq)]
enum MaintenanceActivity {
    Idle,
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
}

/// 索引是否反映了文件系统。只在一件工作结束时由队列按"还有没有失败待重查的位置"推导，
/// 不跟着某一件工作的成败走，所以全量检查失败后一次小更新成功也盖不掉它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IndexCompleteness {
    /// 这次启动后的全量检查还没结束。
    Unverified,
    Complete,
    Incomplete {
        message: String,
    },
}

/// 对外状态的唯一存储。每个字段只有一个写入方：活动、完整度、可见条目数由 worker 写；
/// 维护后端失败由启动维护处写；监视线程失败由它的所有者写；监听健康度由监听线程写；
/// 搜索位置由所有者写，只有"正在应用 → 就绪"由 worker 在完整度变成 `Complete` 时写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DaemonLifecycleSnapshot {
    activity: MaintenanceActivity,
    completeness: IndexCompleteness,
    maintenance_backend_failure: Option<String>,
    root_monitor_failure: Option<String>,
    recovery_rebuild_message: Option<String>,
    watch_coverage_health: WatchCoverageHealth,
    visible_indexed_files: u64,
    capabilities: Vec<ExtractorCapability>,
    path_configuration: SearchPathConfigurationStatus,
}

impl DaemonLifecycleSnapshot {
    pub(super) fn starting(
        visible_indexed_files: u64,
        recovery_rebuild_message: Option<String>,
        path_configuration_phase: SearchPathConfigurationPhase,
    ) -> Self {
        Self {
            activity: MaintenanceActivity::Idle,
            completeness: IndexCompleteness::Unverified,
            maintenance_backend_failure: None,
            root_monitor_failure: None,
            recovery_rebuild_message,
            watch_coverage_health: WatchCoverageHealth::Healthy,
            visible_indexed_files,
            capabilities: Vec::new(),
            path_configuration: SearchPathConfigurationStatus {
                phase: path_configuration_phase,
                ..SearchPathConfigurationStatus::default()
            },
        }
    }

    pub(super) fn begin_work(&mut self) {
        self.activity = MaintenanceActivity::Checking {
            checked_entries: 0,
            changed_entries: 0,
        };
    }

    pub(super) fn apply_progress(&mut self, progress: IndexMaintenanceProgress) {
        self.activity = match progress {
            IndexMaintenanceProgress::Checking {
                checked_entries,
                changed_entries,
            } => MaintenanceActivity::Checking {
                checked_entries,
                changed_entries,
            },
            IndexMaintenanceProgress::Crawling {
                scanned_entries,
                current_scope,
            } => MaintenanceActivity::Crawling {
                scanned_entries,
                current_scope,
            },
            IndexMaintenanceProgress::Applying { pending_mutations } => {
                MaintenanceActivity::Applying { pending_mutations }
            }
        };
    }

    pub(super) fn record_visible_count(&mut self, visible_indexed_files: u64) {
        self.visible_indexed_files = visible_indexed_files;
    }

    /// 一件工作做完（成功或失败）。
    pub(super) fn finish_work(&mut self, completeness: IndexCompleteness) {
        self.activity = MaintenanceActivity::Idle;
        if completeness == IndexCompleteness::Complete {
            self.recovery_rebuild_message = None;
            if self.path_configuration.phase == SearchPathConfigurationPhase::Applying
                && self.path_configuration.desired_revision
                    == self.path_configuration.effective_revision
            {
                self.path_configuration.phase = SearchPathConfigurationPhase::Ready;
            }
        }
        self.completeness = completeness;
    }

    /// 工作被重启或关机打断：没有结论，完整度不变。
    pub(super) fn end_cancelled_work(&mut self) {
        self.activity = MaintenanceActivity::Idle;
    }

    pub(super) fn record_maintenance_failure(&mut self, message: String) {
        self.maintenance_backend_failure = Some(message);
    }

    pub(super) fn clear_maintenance_failure(&mut self) {
        self.maintenance_backend_failure = None;
    }

    /// 监视线程没了：索引照常可用，但挂载和搜索位置的变化跟不上了。
    pub(super) fn record_root_monitor_failure(&mut self, message: String) {
        self.root_monitor_failure = Some(message);
    }

    pub(super) fn update_watch_coverage(&mut self, health: WatchCoverageHealth) {
        self.watch_coverage_health = health;
    }

    pub(super) fn publish_path_configuration(
        &mut self,
        desired_revision: u64,
        effective: &VersionedSearchPathPreferences,
    ) {
        self.path_configuration.desired_revision = desired_revision;
        self.path_configuration.effective_revision = effective.revision;
        self.path_configuration.effective_preferences = effective.preferences.clone();
    }

    pub(super) fn publish_root_statuses(&mut self, roots: Vec<SearchRootStatus>) {
        self.path_configuration.roots = roots;
    }

    pub(super) fn begin_applying_path_configuration(&mut self) {
        self.path_configuration.phase = SearchPathConfigurationPhase::Applying;
    }

    pub(super) fn fail_path_configuration(&mut self, message: String) {
        self.path_configuration.phase = SearchPathConfigurationPhase::Failed { message };
    }

    pub(super) fn path_configuration_phase(&self) -> &SearchPathConfigurationPhase {
        &self.path_configuration.phase
    }

    pub(super) fn to_index_status(&self) -> IndexStatus {
        let phase = match &self.activity {
            MaintenanceActivity::Checking {
                checked_entries,
                changed_entries,
            } => IndexPhase::Checking {
                checked_entries: *checked_entries,
                changed_entries: *changed_entries,
            },
            MaintenanceActivity::Crawling {
                scanned_entries,
                current_scope,
            } => IndexPhase::Crawling {
                scanned_entries: *scanned_entries,
                current_scope: current_scope.clone(),
            },
            MaintenanceActivity::Applying { pending_mutations } => IndexPhase::Applying {
                pending_mutations: *pending_mutations,
            },
            MaintenanceActivity::Idle => match &self.completeness {
                IndexCompleteness::Unverified => IndexPhase::Starting,
                IndexCompleteness::Complete => IndexPhase::Complete,
                IndexCompleteness::Incomplete { message } => IndexPhase::Failed {
                    message: message.clone(),
                },
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
                match self
                    .root_monitor_failure
                    .as_ref()
                    .or(self.recovery_rebuild_message.as_ref())
                {
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
