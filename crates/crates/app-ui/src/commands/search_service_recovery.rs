use std::num::NonZeroU32;
use std::path::Path;
use std::time::{Duration, Instant};

use file_search::{SearchRuntimeIdentity, SearchServiceStatus};
use iced::Task;

use super::search_service::{
    read_validated_search_service_with, SearchUnitAction, SearchUnitController, SearchUnitSnapshot,
    ValidatedSearchServiceFailure,
};
use super::search_service_endpoint::SearchEndpointProbeFailure;
use crate::model::{
    Message, SearchServiceDiagnostic, SearchServiceDiagnosticKind, SearchServiceRecoveryAction,
};

const SEARCH_SERVICE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(60);
const SEARCH_SERVICE_RECOVERY_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn search_service_recovery_command(
    action: SearchServiceRecoveryAction,
) -> Task<Message> {
    Task::perform(recover_search_service(action), move |outcome| {
        Message::SearchServiceRecoveryFinished(action, outcome)
    })
}

async fn recover_search_service(
    action: SearchServiceRecoveryAction,
) -> Result<SearchServiceStatus, SearchServiceDiagnostic> {
    let runtime_identity = SearchRuntimeIdentity::from_environment().map_err(|error| {
        SearchServiceDiagnostic::new(
            SearchServiceDiagnosticKind::RecoveryFailed,
            error.to_string(),
        )
    })?;
    let unit_controller = SearchUnitController::system(runtime_identity);
    recover_search_service_with(&unit_controller, &runtime_identity.socket_path(), action).await
}

pub(super) async fn recover_search_service_with(
    unit_controller: &SearchUnitController,
    socket_path: &Path,
    action: SearchServiceRecoveryAction,
) -> Result<SearchServiceStatus, SearchServiceDiagnostic> {
    let initial_snapshot = unit_controller.show().await.map_err(|error| {
        SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::RecoveryFailed, error)
    })?;
    let previous_main_pid = initial_snapshot.main_pid();
    issue_recovery_actions(unit_controller, &initial_snapshot, action)
        .await
        .map_err(|error| {
            SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::RecoveryFailed,
                format!(
                    "{error}; before recovery: {}",
                    initial_snapshot.description()
                ),
            )
        })?;

    let readiness_deadline = Instant::now() + SEARCH_SERVICE_RECOVERY_TIMEOUT;
    loop {
        let last_observation = match read_validated_search_service_with(
            unit_controller,
            socket_path,
        )
        .await
        {
            Ok(service) if owner_was_replaced(previous_main_pid, service.main_pid) => {
                return Ok(service.status);
            }
            Ok(service) => format!(
                "search service still uses previous MainPID={}",
                service.main_pid
            ),
            Err(ValidatedSearchServiceFailure::StableOwnerEndpoint {
                main_pid,
                endpoint_failure: incompatibility @ SearchEndpointProbeFailure::Incompatible { .. },
                unit_description,
            }) if owner_was_replaced(previous_main_pid, main_pid) => {
                return Err(SearchServiceDiagnostic::new(
                    SearchServiceDiagnosticKind::ComponentIncompatible,
                    format!(
                        "{}; {unit_description}; reinstall the search service components from the current Bennu bundle, then try again",
                        incompatibility.into_message()
                    ),
                ));
            }
            Err(failure) => failure.into_message(),
        };

        if Instant::now() >= readiness_deadline {
            return Err(SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::RecoveryFailed,
                format!(
                    "search service recovery did not produce a verified replacement owner within {} seconds; last observation: {last_observation}",
                    SEARCH_SERVICE_RECOVERY_TIMEOUT.as_secs()
                ),
            ));
        }
        tokio::time::sleep(SEARCH_SERVICE_RECOVERY_POLL_INTERVAL).await;
    }
}

async fn issue_recovery_actions(
    unit_controller: &SearchUnitController,
    initial_snapshot: &SearchUnitSnapshot,
    action: SearchServiceRecoveryAction,
) -> Result<(), String> {
    match action {
        SearchServiceRecoveryAction::Restart => {
            unit_controller
                .reload_then_execute(SearchUnitAction::Restart)
                .await
        }
        SearchServiceRecoveryAction::ForceRestart => {
            if initial_snapshot.may_have_processes() {
                unit_controller
                    .execute(SearchUnitAction::KillControlGroup)
                    .await?;
            }
            unit_controller
                .execute(SearchUnitAction::ResetFailed)
                .await?;
            unit_controller
                .reload_then_execute(SearchUnitAction::Restart)
                .await
        }
    }
}

fn owner_was_replaced(previous_main_pid: Option<NonZeroU32>, current_main_pid: NonZeroU32) -> bool {
    previous_main_pid != Some(current_main_pid)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::owner_was_replaced;

    #[test]
    fn recovery_accepts_only_a_replacement_owner() {
        let previous_main_pid = NonZeroU32::new(42);

        assert!(!owner_was_replaced(
            previous_main_pid,
            NonZeroU32::new(42).unwrap()
        ));
        assert!(owner_was_replaced(
            previous_main_pid,
            NonZeroU32::new(43).unwrap()
        ));
        assert!(owner_was_replaced(None, NonZeroU32::new(43).unwrap()));
    }
}
