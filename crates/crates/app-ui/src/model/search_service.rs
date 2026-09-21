use std::collections::HashSet;
use std::time::SystemTime;

use file_search::{
    IndexHealth, IndexPhase, IndexedQueryAvailability, SearchPathPreferences, SearchServicePhase,
    SearchServiceStatus, VersionedSearchPathPreferences,
};

use super::sanitized_application_log_detail;

const TEMPORARY_FAILURE_CONFIRMATION_COUNT: u8 = 3;
const SEARCH_SERVICE_INCIDENT_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SearchServiceDiagnosticKind {
    EndpointTimedOut,
    EndpointUnavailable,
    ServiceChanging,
    ServiceUnverified,
    EndpointInvalid,
    ComponentIncompatible,
    RecoveryFailed,
    ServiceDegraded,
    ServiceFailed,
    IndexedQueriesUnavailable,
    IndexFailed,
    IndexMaintenanceDegraded,
    IndexMaintenanceFailed,
}

impl SearchServiceDiagnosticKind {
    fn confirmation_count(self) -> u8 {
        match self {
            Self::EndpointTimedOut
            | Self::EndpointUnavailable
            | Self::ServiceChanging
            | Self::ServiceUnverified => TEMPORARY_FAILURE_CONFIRMATION_COUNT,
            Self::EndpointInvalid
            | Self::ComponentIncompatible
            | Self::RecoveryFailed
            | Self::ServiceDegraded
            | Self::ServiceFailed
            | Self::IndexedQueriesUnavailable
            | Self::IndexFailed
            | Self::IndexMaintenanceDegraded
            | Self::IndexMaintenanceFailed => 1,
        }
    }

    fn is_connection_issue(self) -> bool {
        matches!(
            self,
            Self::EndpointTimedOut
                | Self::EndpointUnavailable
                | Self::ServiceChanging
                | Self::ServiceUnverified
                | Self::EndpointInvalid
                | Self::ComponentIncompatible
                | Self::RecoveryFailed
        )
    }

    fn is_status_issue(self) -> bool {
        !self.is_connection_issue()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchServiceDiagnostic {
    pub(crate) kind: SearchServiceDiagnosticKind,
    pub(crate) technical_detail: String,
}

impl SearchServiceDiagnostic {
    pub(crate) fn new(
        kind: SearchServiceDiagnosticKind,
        technical_detail: impl AsRef<str>,
    ) -> Self {
        Self {
            kind,
            technical_detail: sanitized_application_log_detail(technical_detail.as_ref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchServiceStatusRequest {
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchEndpointState {
    Starting,
    Connected,
    Rechecking {
        consecutive_failures: u8,
        diagnostic: SearchServiceDiagnostic,
    },
    Unavailable {
        diagnostic: SearchServiceDiagnostic,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchServiceRecoveryAction {
    Restart,
    ForceRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchServiceRecoveryState {
    Idle,
    ConfirmingForceRestart,
    Running(SearchServiceRecoveryAction),
    Succeeded(SearchServiceRecoveryAction),
    Failed {
        action: SearchServiceRecoveryAction,
        diagnostic: SearchServiceDiagnostic,
    },
}

impl SearchServiceRecoveryState {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchServiceIncidentState {
    Current,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchServiceIncident {
    pub(crate) kind: SearchServiceDiagnosticKind,
    pub(crate) technical_detail: String,
    pub(crate) occurrence_count: u64,
    pub(crate) first_seen: SystemTime,
    pub(crate) last_seen: SystemTime,
    pub(crate) state: SearchServiceIncidentState,
    pub(crate) technical_detail_expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPathEntryKind {
    CustomRoot,
    Exclusion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchPathConfigureRequest {
    pub(crate) expected_revision: u64,
    pub(crate) preferences: SearchPathPreferences,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchPathSettingsState {
    pub(crate) confirmed: Option<VersionedSearchPathPreferences>,
    pub(crate) draft: SearchPathPreferences,
    pub(crate) apply_in_flight: bool,
    pub(crate) failure: Option<String>,
    pub(crate) custom_root_input: String,
    pub(crate) exclusion_input: String,
    pub(crate) picker_in_flight: Option<SearchPathEntryKind>,
    pub(crate) picker_failure: Option<(SearchPathEntryKind, String)>,
    pending: Option<SearchPathPreferences>,
    retry_after_refresh: bool,
}

impl SearchPathSettingsState {
    fn new() -> Self {
        Self {
            confirmed: None,
            draft: SearchPathPreferences::default(),
            apply_in_flight: false,
            failure: None,
            custom_root_input: String::new(),
            exclusion_input: String::new(),
            picker_in_flight: None,
            picker_failure: None,
            pending: None,
            retry_after_refresh: false,
        }
    }

    pub(crate) fn is_stale_revision(&self, revision: u64) -> bool {
        self.confirmed
            .as_ref()
            .is_some_and(|confirmed| revision < confirmed.revision)
    }

    pub(crate) fn accept_snapshot(
        &mut self,
        snapshot: VersionedSearchPathPreferences,
    ) -> Option<SearchPathConfigureRequest> {
        if self.is_stale_revision(snapshot.revision) {
            return None;
        }
        let retry_current = std::mem::take(&mut self.retry_after_refresh);
        self.confirmed = Some(snapshot.clone());
        self.failure = None;
        if retry_current && self.pending.is_none() {
            self.pending = Some(snapshot.preferences.clone());
        }
        if self.pending.is_none() && !self.apply_in_flight {
            self.draft = snapshot.preferences;
        }
        self.begin_pending(retry_current)
    }

    pub(crate) fn queue(
        &mut self,
        preferences: SearchPathPreferences,
    ) -> Option<SearchPathConfigureRequest> {
        self.draft = preferences.clone();
        self.pending = Some(preferences);
        self.failure = None;
        self.begin_pending(false)
    }

    pub(crate) fn accept_applied(
        &mut self,
        snapshot: VersionedSearchPathPreferences,
    ) -> Option<SearchPathConfigureRequest> {
        self.apply_in_flight = false;
        if self.is_stale_revision(snapshot.revision) {
            self.failure = None;
            if self.pending.is_none() {
                self.draft = self
                    .confirmed
                    .as_ref()
                    .expect("stale revision requires a newer confirmed snapshot")
                    .preferences
                    .clone();
            }
            return self.begin_pending(false);
        }
        self.confirmed = Some(snapshot.clone());
        self.failure = None;
        if self.pending.is_none() {
            self.draft = snapshot.preferences;
        }
        self.begin_pending(false)
    }

    pub(crate) fn request_retry_after_refresh(&mut self) -> bool {
        if self.apply_in_flight || self.retry_after_refresh {
            return false;
        }
        self.retry_after_refresh = true;
        true
    }

    pub(crate) fn accept_refresh_failure(&mut self, message: String) {
        self.retry_after_refresh = false;
        self.failure = Some(message);
    }

    pub(crate) fn accept_apply_failure(&mut self, message: String) {
        self.apply_in_flight = false;
        self.failure = Some(message);
        self.pending = Some(self.draft.clone());
    }

    fn begin_pending(
        &mut self,
        allow_confirmed_preferences: bool,
    ) -> Option<SearchPathConfigureRequest> {
        if self.apply_in_flight {
            return None;
        }
        let confirmed = self.confirmed.as_ref()?;
        let preferences = self.pending.take()?;
        if preferences == confirmed.preferences && !allow_confirmed_preferences {
            self.draft = confirmed.preferences.clone();
            return None;
        }
        self.apply_in_flight = true;
        Some(SearchPathConfigureRequest {
            expected_revision: confirmed.revision,
            preferences,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchServiceState {
    pub(crate) endpoint: SearchEndpointState,
    pub(crate) confirmed_status: Option<SearchServiceStatus>,
    pub(crate) last_confirmed_at: Option<SystemTime>,
    pub(crate) recovery: SearchServiceRecoveryState,
    pub(crate) incidents: Vec<SearchServiceIncident>,
    pub(crate) path_settings: SearchPathSettingsState,
    pending_status_request: Option<SearchServiceStatusRequest>,
    next_status_generation: u64,
    consecutive_temporary_failures: u8,
}

impl SearchServiceState {
    pub(crate) fn new() -> Self {
        Self {
            endpoint: SearchEndpointState::Starting,
            confirmed_status: None,
            last_confirmed_at: None,
            recovery: SearchServiceRecoveryState::Idle,
            incidents: Vec::new(),
            path_settings: SearchPathSettingsState::new(),
            pending_status_request: None,
            next_status_generation: 0,
            consecutive_temporary_failures: 0,
        }
    }

    pub(crate) fn begin_initial_status_request(&mut self) -> SearchServiceStatusRequest {
        debug_assert!(self.pending_status_request.is_none());
        self.begin_status_request()
    }

    pub(crate) fn request_status_refresh(&mut self) -> Option<SearchServiceStatusRequest> {
        if self.pending_status_request.is_some() || self.recovery.is_running() {
            return None;
        }
        Some(self.begin_status_request())
    }

    pub(crate) fn accept_status_request(
        &mut self,
        request: SearchServiceStatusRequest,
        outcome: Result<SearchServiceStatus, SearchServiceDiagnostic>,
    ) -> bool {
        self.accept_status_request_at(request, outcome, SystemTime::now())
    }

    pub(crate) fn observe_query_transport_failure(&mut self, technical_detail: impl AsRef<str>) {
        let diagnostic = SearchServiceDiagnostic::new(
            SearchServiceDiagnosticKind::EndpointUnavailable,
            technical_detail,
        );
        self.accept_connection_failure(diagnostic, SystemTime::now());
    }

    pub(crate) fn begin_restart(&mut self) -> Option<SearchServiceRecoveryAction> {
        if self.recovery.is_running() {
            return None;
        }
        self.pending_status_request = None;
        let action = SearchServiceRecoveryAction::Restart;
        self.recovery = SearchServiceRecoveryState::Running(action);
        Some(action)
    }

    pub(crate) fn press_force_restart(&mut self) -> Option<SearchServiceRecoveryAction> {
        match &self.recovery {
            SearchServiceRecoveryState::Running(_) => None,
            SearchServiceRecoveryState::ConfirmingForceRestart => {
                self.pending_status_request = None;
                let action = SearchServiceRecoveryAction::ForceRestart;
                self.recovery = SearchServiceRecoveryState::Running(action);
                Some(action)
            }
            _ => {
                self.recovery = SearchServiceRecoveryState::ConfirmingForceRestart;
                None
            }
        }
    }

    pub(crate) fn cancel_force_restart_confirmation(&mut self) -> bool {
        if self.recovery == SearchServiceRecoveryState::ConfirmingForceRestart {
            self.recovery = SearchServiceRecoveryState::Idle;
            true
        } else {
            false
        }
    }

    pub(crate) fn accept_recovery_completion(
        &mut self,
        action: SearchServiceRecoveryAction,
        outcome: Result<SearchServiceStatus, SearchServiceDiagnostic>,
    ) -> bool {
        if self.recovery != SearchServiceRecoveryState::Running(action) {
            return false;
        }

        match outcome {
            Ok(status) => {
                self.recovery = SearchServiceRecoveryState::Succeeded(action);
                self.accept_confirmed_status(status, SystemTime::now());
            }
            Err(diagnostic) => {
                self.record_distinct_incident(&diagnostic, SystemTime::now());
                self.recovery = SearchServiceRecoveryState::Failed { action, diagnostic };
            }
        }
        true
    }

    pub(crate) fn toggle_incident_technical_detail(&mut self, kind: SearchServiceDiagnosticKind) {
        if let Some(incident) = self
            .incidents
            .iter_mut()
            .find(|incident| incident.kind == kind)
        {
            incident.technical_detail_expanded = !incident.technical_detail_expanded;
        }
    }

    pub(crate) fn incident_technical_detail(
        &self,
        kind: SearchServiceDiagnosticKind,
    ) -> Option<String> {
        self.incidents
            .iter()
            .find(|incident| incident.kind == kind)
            .map(|incident| incident.technical_detail.clone())
    }

    fn begin_status_request(&mut self) -> SearchServiceStatusRequest {
        self.next_status_generation = self.next_status_generation.wrapping_add(1);
        let request = SearchServiceStatusRequest {
            generation: self.next_status_generation,
        };
        self.pending_status_request = Some(request);
        request
    }

    fn accept_status_request_at(
        &mut self,
        request: SearchServiceStatusRequest,
        outcome: Result<SearchServiceStatus, SearchServiceDiagnostic>,
        observed_at: SystemTime,
    ) -> bool {
        if self.pending_status_request != Some(request) {
            return false;
        }
        self.pending_status_request = None;

        match outcome {
            Ok(status) => self.accept_confirmed_status(status, observed_at),
            Err(diagnostic) => self.accept_connection_failure(diagnostic, observed_at),
        }
        true
    }

    fn accept_confirmed_status(&mut self, status: SearchServiceStatus, observed_at: SystemTime) {
        self.endpoint = SearchEndpointState::Connected;
        self.confirmed_status = Some(status.clone());
        self.last_confirmed_at = Some(observed_at);
        self.consecutive_temporary_failures = 0;
        self.recover_matching_incidents(|kind| kind.is_connection_issue(), observed_at);
        self.synchronize_status_incidents(&status, observed_at);
    }

    fn accept_connection_failure(
        &mut self,
        diagnostic: SearchServiceDiagnostic,
        observed_at: SystemTime,
    ) {
        self.record_distinct_incident(&diagnostic, observed_at);
        self.consecutive_temporary_failures = self.consecutive_temporary_failures.saturating_add(1);

        let failure_is_confirmed =
            self.consecutive_temporary_failures >= diagnostic.kind.confirmation_count();
        if failure_is_confirmed {
            let was_unavailable = matches!(self.endpoint, SearchEndpointState::Unavailable { .. });
            self.endpoint = SearchEndpointState::Unavailable {
                diagnostic: diagnostic.clone(),
            };
            if !was_unavailable {
                tracing::error!(
                    target: "app_ui::search_service",
                    event = "search_service_unavailable",
                    issue = ?diagnostic.kind,
                    error = %diagnostic.technical_detail,
                    "search service availability failure confirmed"
                );
            }
        } else if !matches!(self.endpoint, SearchEndpointState::Unavailable { .. }) {
            self.endpoint = SearchEndpointState::Rechecking {
                consecutive_failures: self.consecutive_temporary_failures,
                diagnostic,
            };
        }
    }

    fn synchronize_status_incidents(
        &mut self,
        status: &SearchServiceStatus,
        observed_at: SystemTime,
    ) {
        let active_diagnostics = diagnostics_from_status(status);
        let active_kinds = active_diagnostics
            .iter()
            .map(|diagnostic| diagnostic.kind)
            .collect::<HashSet<_>>();

        self.recover_matching_incidents(
            |kind| kind.is_status_issue() && !active_kinds.contains(&kind),
            observed_at,
        );
        for diagnostic in active_diagnostics {
            self.activate_status_incident(&diagnostic, observed_at);
        }
    }

    fn activate_status_incident(
        &mut self,
        diagnostic: &SearchServiceDiagnostic,
        observed_at: SystemTime,
    ) {
        if let Some(incident) = self
            .incidents
            .iter_mut()
            .find(|incident| incident.kind == diagnostic.kind)
        {
            incident.technical_detail = diagnostic.technical_detail.clone();
            if incident.state == SearchServiceIncidentState::Current {
                return;
            }
        }
        self.record_distinct_incident(diagnostic, observed_at);
    }

    fn record_distinct_incident(
        &mut self,
        diagnostic: &SearchServiceDiagnostic,
        observed_at: SystemTime,
    ) {
        if let Some(position) = self
            .incidents
            .iter()
            .position(|incident| incident.kind == diagnostic.kind)
        {
            let mut incident = self.incidents.remove(position);
            let incident_reopened = incident.state == SearchServiceIncidentState::Recovered;
            incident.occurrence_count = incident.occurrence_count.saturating_add(1);
            incident.last_seen = observed_at;
            incident.state = SearchServiceIncidentState::Current;
            incident.technical_detail = diagnostic.technical_detail.clone();
            self.incidents.insert(0, incident);
            if incident_reopened {
                tracing::warn!(
                    target: "app_ui::search_service",
                    event = "search_service_issue_reopened",
                    issue = ?diagnostic.kind,
                    error = %diagnostic.technical_detail,
                    "search service issue occurred again"
                );
            }
            return;
        }

        tracing::warn!(
            target: "app_ui::search_service",
            event = "search_service_issue_observed",
            issue = ?diagnostic.kind,
            error = %diagnostic.technical_detail,
            "search service issue observed"
        );
        self.incidents.insert(
            0,
            SearchServiceIncident {
                kind: diagnostic.kind,
                technical_detail: diagnostic.technical_detail.clone(),
                occurrence_count: 1,
                first_seen: observed_at,
                last_seen: observed_at,
                state: SearchServiceIncidentState::Current,
                technical_detail_expanded: false,
            },
        );
        if self.incidents.len() > SEARCH_SERVICE_INCIDENT_LIMIT {
            let removal_position = self
                .incidents
                .iter()
                .rposition(|incident| incident.state == SearchServiceIncidentState::Recovered)
                .unwrap_or(self.incidents.len() - 1);
            self.incidents.remove(removal_position);
        }
    }

    fn recover_matching_incidents(
        &mut self,
        mut should_recover: impl FnMut(SearchServiceDiagnosticKind) -> bool,
        observed_at: SystemTime,
    ) {
        for incident in &mut self.incidents {
            if incident.state == SearchServiceIncidentState::Current
                && should_recover(incident.kind)
            {
                incident.state = SearchServiceIncidentState::Recovered;
                tracing::info!(
                    target: "app_ui::search_service",
                    event = "search_service_issue_recovered",
                    issue = ?incident.kind,
                    recovered_at = ?observed_at,
                    "search service issue recovered"
                );
            }
        }
    }
}

fn diagnostics_from_status(status: &SearchServiceStatus) -> Vec<SearchServiceDiagnostic> {
    let mut diagnostics = Vec::new();
    match (&status.phase, &status.query_availability) {
        (SearchServicePhase::Degraded { message }, _) => {
            diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::ServiceDegraded,
                format!("search service reported a degraded state: {message}"),
            ));
        }
        (SearchServicePhase::Failed { message }, _) => {
            diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::ServiceFailed,
                format!("search service reported a failed state: {message}"),
            ));
        }
        (SearchServicePhase::Ready, IndexedQueryAvailability::Unavailable { message }) => {
            diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::IndexedQueriesUnavailable,
                format!("indexed queries are unavailable: {message}"),
            ))
        }
        (
            SearchServicePhase::Starting
            | SearchServicePhase::Ready
            | SearchServicePhase::ShuttingDown,
            _,
        ) => {}
    }
    if let Some(index_status) = &status.index_status {
        if let IndexPhase::Failed { message } = &index_status.phase {
            diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::IndexFailed,
                format!("index task failed: {message}"),
            ));
        }
        match &index_status.health {
            IndexHealth::Degraded { message } => diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::IndexMaintenanceDegraded,
                format!("index maintenance is degraded: {message}"),
            )),
            IndexHealth::Error { message } => diagnostics.push(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::IndexMaintenanceFailed,
                format!("index maintenance failed: {message}"),
            )),
            IndexHealth::Healthy => {}
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use file_search::{
        IndexedQueryAvailability, SearchPathPreferences, SearchServicePhase, SearchServiceStatus,
        VersionedSearchPathPreferences,
    };

    use super::*;

    fn healthy_status() -> SearchServiceStatus {
        SearchServiceStatus {
            phase: SearchServicePhase::Ready,
            query_availability: IndexedQueryAvailability::Available,
            index_status: None,
        }
    }

    fn timeout_diagnostic() -> SearchServiceDiagnostic {
        SearchServiceDiagnostic::new(
            SearchServiceDiagnosticKind::EndpointTimedOut,
            "endpoint timed out",
        )
    }

    #[test]
    fn path_configuration_ignores_older_read_snapshots() {
        let mut state = SearchPathSettingsState::new();
        let current = VersionedSearchPathPreferences {
            revision: 7,
            preferences: SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/mnt/current")],
                exclusions: Vec::new(),
            },
        };
        state.accept_snapshot(current.clone());

        assert!(state
            .accept_snapshot(VersionedSearchPathPreferences {
                revision: 6,
                preferences: SearchPathPreferences::default(),
            })
            .is_none());
        assert_eq!(state.confirmed.as_ref(), Some(&current));
        assert_eq!(state.draft, current.preferences);
    }

    #[test]
    fn path_configuration_stale_read_preserves_the_active_retry() {
        let mut state = SearchPathSettingsState::new();
        let current = VersionedSearchPathPreferences {
            revision: 7,
            preferences: SearchPathPreferences::default(),
        };
        state.accept_snapshot(current.clone());
        state.accept_apply_failure("transition failed".to_owned());
        assert!(state.request_retry_after_refresh());

        assert!(state
            .accept_snapshot(VersionedSearchPathPreferences {
                revision: 6,
                preferences: SearchPathPreferences::default(),
            })
            .is_none());

        assert_eq!(state.confirmed.as_ref(), Some(&current));
        assert!(!state.request_retry_after_refresh());
        let retry = state
            .accept_snapshot(current)
            .expect("the retry response must still replay the failed configuration");
        assert_eq!(retry.expected_revision, 7);
        assert_eq!(retry.preferences, SearchPathPreferences::default());
    }

    #[test]
    fn path_configuration_stale_apply_rebases_pending_on_the_newer_snapshot() {
        let mut state = SearchPathSettingsState::new();
        state.accept_snapshot(VersionedSearchPathPreferences {
            revision: 4,
            preferences: SearchPathPreferences::default(),
        });
        let first = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/mnt/first")],
            exclusions: Vec::new(),
        };
        let pending = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/mnt/pending")],
            exclusions: Vec::new(),
        };
        let newer = VersionedSearchPathPreferences {
            revision: 6,
            preferences: SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/mnt/peer")],
                exclusions: Vec::new(),
            },
        };
        state.queue(first.clone()).unwrap();
        assert!(state.queue(pending.clone()).is_none());
        assert!(state.accept_snapshot(newer.clone()).is_none());

        let follow_up = state
            .accept_applied(VersionedSearchPathPreferences {
                revision: 5,
                preferences: first,
            })
            .expect("pending draft must rebase on the newer snapshot");

        assert_eq!(state.confirmed.as_ref(), Some(&newer));
        assert_eq!(follow_up.expected_revision, 6);
        assert_eq!(follow_up.preferences, pending);
    }

    #[test]
    fn path_configuration_coalesces_to_one_follow_up_write() {
        let mut state = SearchPathSettingsState::new();
        state.accept_snapshot(VersionedSearchPathPreferences {
            revision: 4,
            preferences: SearchPathPreferences::default(),
        });
        let first_preferences = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/media/first")],
            exclusions: Vec::new(),
        };
        let second_preferences = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/media/second")],
            exclusions: vec![PathBuf::from("/media/second/private")],
        };

        let first = state
            .queue(first_preferences.clone())
            .expect("first write must start");
        assert_eq!(first.expected_revision, 4);
        assert!(state.queue(second_preferences.clone()).is_none());
        let follow_up = state
            .accept_applied(VersionedSearchPathPreferences {
                revision: 5,
                preferences: first_preferences,
            })
            .expect("latest pending snapshot must follow the first write");

        assert_eq!(follow_up.expected_revision, 5);
        assert_eq!(follow_up.preferences, second_preferences);
        assert!(state.apply_in_flight);
    }

    #[test]
    fn path_configuration_failure_rebases_pending_draft_before_retry() {
        let mut state = SearchPathSettingsState::new();
        state.accept_snapshot(VersionedSearchPathPreferences {
            revision: 2,
            preferences: SearchPathPreferences::default(),
        });
        let draft = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/mnt/archive")],
            exclusions: Vec::new(),
        };
        state.queue(draft.clone()).unwrap();
        state.accept_apply_failure("stale revision".to_owned());

        let retry = state
            .accept_snapshot(VersionedSearchPathPreferences {
                revision: 9,
                preferences: SearchPathPreferences {
                    custom_roots: vec![PathBuf::from("/mnt/peer")],
                    exclusions: Vec::new(),
                },
            })
            .expect("fresh snapshot must rebase the pending draft");

        assert_eq!(retry.expected_revision, 9);
        assert_eq!(retry.preferences, draft);
        assert!(state.failure.is_none());
    }

    #[test]
    fn path_configuration_retry_replays_the_failed_desired_revision() {
        let mut state = SearchPathSettingsState::new();
        let desired = SearchPathPreferences {
            custom_roots: vec![PathBuf::from("/mnt/archive")],
            exclusions: Vec::new(),
        };
        state.accept_snapshot(VersionedSearchPathPreferences {
            revision: 2,
            preferences: SearchPathPreferences::default(),
        });
        state.queue(desired.clone()).unwrap();
        state.accept_apply_failure("transition failed".to_owned());

        assert!(state.request_retry_after_refresh());
        let retry = state
            .accept_snapshot(VersionedSearchPathPreferences {
                revision: 3,
                preferences: desired.clone(),
            })
            .expect("explicit retry must replay the desired snapshot");

        assert_eq!(retry.expected_revision, 3);
        assert_eq!(retry.preferences, desired);
        assert!(state.apply_in_flight);
    }

    #[test]
    fn status_refresh_is_single_flight_and_rejects_late_outcomes() {
        let mut state = SearchServiceState::new();
        let first_request = state.begin_initial_status_request();

        assert!(state.request_status_refresh().is_none());
        assert!(state.accept_status_request_at(first_request, Ok(healthy_status()), UNIX_EPOCH,));
        let second_request = state.request_status_refresh().unwrap();
        assert!(!state.accept_status_request_at(
            first_request,
            Err(timeout_diagnostic()),
            UNIX_EPOCH + Duration::from_secs(1),
        ));
        assert!(state.accept_status_request_at(
            second_request,
            Ok(healthy_status()),
            UNIX_EPOCH + Duration::from_secs(2),
        ));
    }

    #[test]
    fn one_timeout_preserves_the_confirmed_snapshot() {
        let mut state = SearchServiceState::new();
        let initial_request = state.begin_initial_status_request();
        state.accept_status_request_at(initial_request, Ok(healthy_status()), UNIX_EPOCH);
        let refresh_request = state.request_status_refresh().unwrap();

        state.accept_status_request_at(
            refresh_request,
            Err(timeout_diagnostic()),
            UNIX_EPOCH + Duration::from_secs(1),
        );

        assert_eq!(state.confirmed_status, Some(healthy_status()));
        assert!(matches!(
            state.endpoint,
            SearchEndpointState::Rechecking {
                consecutive_failures: 1,
                ..
            }
        ));
    }

    #[test]
    fn temporary_failure_requires_three_consecutive_observations() {
        let mut state = SearchServiceState::new();

        for failure_index in 0..3 {
            let request = if failure_index == 0 {
                state.begin_initial_status_request()
            } else {
                state.request_status_refresh().unwrap()
            };
            state.accept_status_request_at(
                request,
                Err(timeout_diagnostic()),
                UNIX_EPOCH + Duration::from_secs(failure_index),
            );
        }

        assert!(matches!(
            state.endpoint,
            SearchEndpointState::Unavailable { .. }
        ));
        assert_eq!(state.incidents[0].occurrence_count, 3);
    }

    #[test]
    fn success_recovers_connection_incident_without_deleting_history() {
        let mut state = SearchServiceState::new();
        let failed_request = state.begin_initial_status_request();
        state.accept_status_request_at(failed_request, Err(timeout_diagnostic()), UNIX_EPOCH);
        let successful_request = state.request_status_refresh().unwrap();

        state.accept_status_request_at(
            successful_request,
            Ok(healthy_status()),
            UNIX_EPOCH + Duration::from_secs(1),
        );

        assert_eq!(state.endpoint, SearchEndpointState::Connected);
        assert_eq!(state.incidents.len(), 1);
        assert_eq!(
            state.incidents[0].state,
            SearchServiceIncidentState::Recovered
        );
    }

    #[test]
    fn explicit_endpoint_inconsistency_is_confirmed_immediately() {
        let mut state = SearchServiceState::new();
        let request = state.begin_initial_status_request();
        let diagnostic = SearchServiceDiagnostic::new(
            SearchServiceDiagnosticKind::EndpointInvalid,
            "endpoint owner does not match",
        );

        state.accept_status_request_at(request, Err(diagnostic), UNIX_EPOCH);

        assert!(matches!(
            state.endpoint,
            SearchEndpointState::Unavailable { .. }
        ));
    }

    #[test]
    fn recovery_failure_does_not_discard_the_last_confirmed_status() {
        let mut state = SearchServiceState::new();
        let initial_request = state.begin_initial_status_request();
        state.accept_status_request_at(initial_request, Ok(healthy_status()), UNIX_EPOCH);
        let endpoint_before_recovery = state.endpoint.clone();
        let action = state.begin_restart().unwrap();
        let diagnostic = SearchServiceDiagnostic::new(
            SearchServiceDiagnosticKind::RecoveryFailed,
            "systemctl restart failed",
        );

        assert!(state.accept_recovery_completion(action, Err(diagnostic)));

        assert_eq!(state.endpoint, endpoint_before_recovery);
        assert_eq!(state.confirmed_status, Some(healthy_status()));
        assert!(matches!(
            state.recovery,
            SearchServiceRecoveryState::Failed { .. }
        ));
    }

    #[test]
    fn service_phase_and_query_availability_create_only_contract_violations() {
        let unavailable = IndexedQueryAvailability::Unavailable {
            message: "not ready".to_owned(),
        };
        let cases = [
            (SearchServicePhase::Starting, Vec::new()),
            (SearchServicePhase::ShuttingDown, Vec::new()),
            (
                SearchServicePhase::Ready,
                vec![SearchServiceDiagnosticKind::IndexedQueriesUnavailable],
            ),
            (
                SearchServicePhase::Degraded {
                    message: "degraded".to_owned(),
                },
                vec![SearchServiceDiagnosticKind::ServiceDegraded],
            ),
            (
                SearchServicePhase::Failed {
                    message: "failed".to_owned(),
                },
                vec![SearchServiceDiagnosticKind::ServiceFailed],
            ),
        ];

        for (phase, expected_kinds) in cases {
            let status = SearchServiceStatus {
                phase,
                query_availability: unavailable.clone(),
                index_status: None,
            };
            assert_eq!(
                diagnostics_from_status(&status)
                    .into_iter()
                    .map(|diagnostic| diagnostic.kind)
                    .collect::<Vec<_>>(),
                expected_kinds
            );
        }
    }

    #[test]
    fn repeated_status_issue_counts_only_distinct_lifecycles() {
        let mut state = SearchServiceState::new();
        let degraded_status = SearchServiceStatus {
            phase: SearchServicePhase::Degraded {
                message: "watcher unavailable".to_owned(),
            },
            ..healthy_status()
        };

        for observed_second in 0..2 {
            let request = if observed_second == 0 {
                state.begin_initial_status_request()
            } else {
                state.request_status_refresh().unwrap()
            };
            state.accept_status_request_at(
                request,
                Ok(degraded_status.clone()),
                UNIX_EPOCH + Duration::from_secs(observed_second),
            );
        }

        assert_eq!(state.incidents.len(), 1);
        assert_eq!(state.incidents[0].occurrence_count, 1);
        let recovery_request = state.request_status_refresh().unwrap();
        state.accept_status_request_at(
            recovery_request,
            Ok(healthy_status()),
            UNIX_EPOCH + Duration::from_secs(2),
        );
        let recurrence_request = state.request_status_refresh().unwrap();
        state.accept_status_request_at(
            recurrence_request,
            Ok(degraded_status),
            UNIX_EPOCH + Duration::from_secs(3),
        );
        assert_eq!(state.incidents[0].occurrence_count, 2);
    }
}
