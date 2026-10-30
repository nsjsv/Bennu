use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use file_search::{SearchPathPreferences, SearchRuntimeIdentity, SearchServiceStatus};
use iced::Task;

use super::search_service_endpoint::{inspect_search_endpoint, SearchEndpointProbeFailure};
pub(super) use super::search_service_systemd::{
    SearchUnitAction, SearchUnitController, SearchUnitSnapshot, UnitActiveState,
};
use crate::model::{
    Message, SearchPathConfigureRequest, SearchPathEntryKind, SearchServiceDiagnostic,
    SearchServiceDiagnosticKind, SearchServiceStatusRequest,
};

pub(crate) fn ensure_search_service_command(request: SearchServiceStatusRequest) -> Task<Message> {
    Task::perform(ensure_search_service(), move |outcome| {
        Message::SearchServiceEnsured(request, outcome)
    })
}

pub(crate) fn search_service_status_command(request: SearchServiceStatusRequest) -> Task<Message> {
    Task::perform(read_search_service_status(), move |outcome| {
        Message::SearchServiceStatusLoaded(request, outcome)
    })
}

pub(crate) fn search_path_configuration_command() -> Task<Message> {
    Task::perform(
        read_search_path_configuration(),
        Message::SearchPathConfigurationLoaded,
    )
}

pub(crate) fn configure_search_paths_command(request: SearchPathConfigureRequest) -> Task<Message> {
    Task::perform(
        configure_search_paths(request.expected_revision, request.preferences),
        Message::SearchPathConfigurationApplied,
    )
}

pub(crate) fn search_path_directory_chooser_command(kind: SearchPathEntryKind) -> Task<Message> {
    Task::perform(choose_search_directory(), move |outcome| {
        Message::SearchPathDirectoryChosen(kind, outcome)
    })
}

/// 目录回退的跳过规则：直接读服务同一份路径偏好文件，不经服务进程——服务没运行时
/// 回退照样可用（PRD：发行包默认不启用服务）。`config_directory` 为 None 时没有配置目录，
/// 按读不了处理，回退拒绝搜索。
pub(crate) async fn read_directory_fallback_preferences(
    config_directory: PathBuf,
) -> Result<SearchPathPreferences, String> {
    tokio::task::spawn_blocking(move || {
        file_search::load_search_path_preferences(&config_directory)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn read_search_path_configuration() -> Result<
    (
        file_search::VersionedSearchPathPreferences,
        file_search::SearchPathConfigurationStatus,
    ),
    SearchServiceDiagnostic,
> {
    let runtime_identity = configured_runtime_identity().map_err(|error| {
        SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceUnverified, error)
    })?;
    file_search::path_configuration_via_socket(&runtime_identity.socket_path())
        .await
        .map_err(path_configuration_diagnostic)
}

async fn configure_search_paths(
    expected_revision: u64,
    preferences: SearchPathPreferences,
) -> Result<
    (
        file_search::VersionedSearchPathPreferences,
        file_search::SearchPathConfigurationStatus,
    ),
    SearchServiceDiagnostic,
> {
    let runtime_identity = configured_runtime_identity().map_err(|error| {
        SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceUnverified, error)
    })?;
    file_search::configure_path_preferences_via_socket(
        &runtime_identity.socket_path(),
        expected_revision,
        preferences,
    )
    .await
    .map_err(path_configuration_diagnostic)
}

fn path_configuration_diagnostic(error: file_search::SearchError) -> SearchServiceDiagnostic {
    SearchServiceDiagnostic::new(
        SearchServiceDiagnosticKind::EndpointUnavailable,
        error.to_string(),
    )
}

async fn choose_search_directory() -> Result<Option<PathBuf>, String> {
    let title = tr!("Select a search location");
    let accept_label = tr!("Select");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .directory(true)
        .multiple(false)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    selected_search_directory(selected.uris())
}

fn selected_search_directory(uris: &[url::Url]) -> Result<Option<PathBuf>, String> {
    uris.first()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| tr!("the selected location is not a local directory"))
        })
        .transpose()
}

async fn ensure_search_service() -> Result<SearchServiceStatus, SearchServiceDiagnostic> {
    let runtime_identity = configured_runtime_identity().map_err(|error| {
        SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceUnverified, error)
    })?;
    let unit_controller = SearchUnitController::system(runtime_identity);
    ensure_search_service_with(&unit_controller, &runtime_identity.socket_path())
        .await
        .map_err(|error| {
            SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceUnverified, error)
        })
}

async fn read_search_service_status() -> Result<SearchServiceStatus, SearchServiceDiagnostic> {
    let runtime_identity = configured_runtime_identity().map_err(|error| {
        SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceUnverified, error)
    })?;
    let unit_controller = SearchUnitController::system(runtime_identity);
    read_search_service_status_with(&unit_controller, &runtime_identity.socket_path()).await
}

fn configured_runtime_identity() -> Result<SearchRuntimeIdentity, String> {
    SearchRuntimeIdentity::from_environment().map_err(|error| error.to_string())
}

async fn read_search_service_status_with(
    unit_controller: &SearchUnitController,
    socket_path: &Path,
) -> Result<SearchServiceStatus, SearchServiceDiagnostic> {
    Ok(
        read_validated_search_service_with(unit_controller, socket_path)
            .await
            .map_err(ValidatedSearchServiceFailure::into_diagnostic)?
            .status,
    )
}

pub(super) struct ValidatedSearchService {
    pub(super) main_pid: NonZeroU32,
    pub(super) status: SearchServiceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ValidatedSearchServiceFailure {
    OwnerUnverified(String),
    OwnerChanged(String),
    StableOwnerEndpoint {
        main_pid: NonZeroU32,
        endpoint_failure: SearchEndpointProbeFailure,
        unit_description: String,
    },
}

impl ValidatedSearchServiceFailure {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::OwnerUnverified(message) | Self::OwnerChanged(message) => message,
            Self::StableOwnerEndpoint {
                endpoint_failure,
                unit_description,
                ..
            } => format!("{}; {unit_description}", endpoint_failure.into_message()),
        }
    }

    pub(super) fn into_diagnostic(self) -> SearchServiceDiagnostic {
        match self {
            Self::OwnerUnverified(message) => SearchServiceDiagnostic::new(
                SearchServiceDiagnosticKind::ServiceUnverified,
                message,
            ),
            Self::OwnerChanged(message) => {
                SearchServiceDiagnostic::new(SearchServiceDiagnosticKind::ServiceChanging, message)
            }
            Self::StableOwnerEndpoint {
                endpoint_failure,
                unit_description,
                ..
            } => {
                let diagnostic_kind = match &endpoint_failure {
                    SearchEndpointProbeFailure::TimedOut => {
                        SearchServiceDiagnosticKind::EndpointTimedOut
                    }
                    SearchEndpointProbeFailure::TemporarilyUnavailable(_) => {
                        SearchServiceDiagnosticKind::EndpointUnavailable
                    }
                    SearchEndpointProbeFailure::RestartRequired(_) => {
                        SearchServiceDiagnosticKind::EndpointInvalid
                    }
                    SearchEndpointProbeFailure::Incompatible { .. } => {
                        SearchServiceDiagnosticKind::ComponentIncompatible
                    }
                };
                SearchServiceDiagnostic::new(
                    diagnostic_kind,
                    format!("{}; {unit_description}", endpoint_failure.into_message()),
                )
            }
        }
    }
}

impl From<String> for ValidatedSearchServiceFailure {
    fn from(message: String) -> Self {
        Self::OwnerUnverified(message)
    }
}

pub(super) async fn read_validated_search_service_with(
    unit_controller: &SearchUnitController,
    socket_path: &Path,
) -> Result<ValidatedSearchService, ValidatedSearchServiceFailure> {
    let initial_snapshot = unit_controller.show().await?;
    let expected_main_pid = unit_controller
        .validated_main_pid(&initial_snapshot)
        .await?;
    let endpoint_observation = inspect_search_endpoint(socket_path, expected_main_pid).await;
    let confirmed_snapshot = unit_controller.show().await.map_err(|error| {
        ValidatedSearchServiceFailure::OwnerChanged(format!(
            "search service changed while its endpoint was inspected: {error}"
        ))
    })?;
    let confirmed_main_pid = unit_controller
        .validated_main_pid(&confirmed_snapshot)
        .await
        .map_err(|error| {
            ValidatedSearchServiceFailure::OwnerChanged(format!(
                "search service changed while its endpoint was inspected: {error}"
            ))
        })?;
    if confirmed_main_pid != expected_main_pid {
        return Err(ValidatedSearchServiceFailure::OwnerChanged(format!(
            "search service changed while its endpoint was inspected: {}",
            confirmed_snapshot.description()
        )));
    }
    let service_status = match endpoint_observation {
        Ok(service_status) => service_status,
        Err(endpoint_failure) => {
            return Err(ValidatedSearchServiceFailure::StableOwnerEndpoint {
                main_pid: expected_main_pid,
                endpoint_failure,
                unit_description: confirmed_snapshot.description(),
            });
        }
    };
    Ok(ValidatedSearchService {
        main_pid: expected_main_pid,
        status: service_status,
    })
}

async fn ensure_search_service_with(
    unit_controller: &SearchUnitController,
    socket_path: &Path,
) -> Result<SearchServiceStatus, String> {
    // unit 是 Type=notify：start/restart 等到服务建好 socket 报就绪（或启动失败、
    // 超过 TimeoutStartSec）才返回，之后确认一次端点即可，不用轮询。
    let initial_snapshot = unit_controller.show().await?;
    let action = match (&initial_snapshot.active_state, initial_snapshot.main_pid) {
        (UnitActiveState::Active, Some(_)) => None,
        (UnitActiveState::Active, None) => Some(SearchUnitAction::Restart),
        // 启动中的 unit 再 start 会并入同一个启动任务，一起等它就绪。
        (
            UnitActiveState::Activating
            | UnitActiveState::Inactive
            | UnitActiveState::Failed
            | UnitActiveState::Deactivating,
            _,
        ) => Some(SearchUnitAction::Start),
        (UnitActiveState::Other(active_state), _) => {
            return Err(format!(
                "search service unit has unsupported ActiveState={active_state}: {}",
                initial_snapshot.description()
            ));
        }
    };
    if let Some(action) = action {
        unit_controller
            .reload_then_execute(action)
            .await
            .map_err(|error| format!("{error}; {}", initial_snapshot.description()))?;
    }

    match read_validated_search_service_with(unit_controller, socket_path).await {
        Ok(service) => Ok(service.status),
        // 在跑的是旧版本或要求重启：重启一次换成当前安装的服务，再确认一次。
        Err(ValidatedSearchServiceFailure::StableOwnerEndpoint {
            endpoint_failure:
                SearchEndpointProbeFailure::RestartRequired(_)
                | SearchEndpointProbeFailure::Incompatible { .. },
            unit_description,
            ..
        }) if action != Some(SearchUnitAction::Restart) => {
            unit_controller
                .reload_then_execute(SearchUnitAction::Restart)
                .await
                .map_err(|error| format!("{error}; {unit_description}"))?;
            read_validated_search_service_with(unit_controller, socket_path)
                .await
                .map(|service| service.status)
                .map_err(ValidatedSearchServiceFailure::into_message)
        }
        Err(failure) => Err(failure.into_message()),
    }
}

#[cfg(test)]
#[path = "search_service_tests.rs"]
mod tests;
