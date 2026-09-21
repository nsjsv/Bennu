use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use file_search::{
    daemon_build_id, read_service_event, write_service_request, SearchServiceEvent,
    SearchServiceRequest, SearchServiceStatus, PROTOCOL_VERSION,
};
use tokio::net::UnixStream;

const SEARCH_ENDPOINT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SearchEndpointProbeFailure {
    TimedOut,
    TemporarilyUnavailable(String),
    RestartRequired(String),
    Incompatible {
        actual_protocol: u32,
        actual_build: String,
    },
}

impl SearchEndpointProbeFailure {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::TimedOut => "search service endpoint inspection timed out".to_owned(),
            Self::TemporarilyUnavailable(message) | Self::RestartRequired(message) => message,
            Self::Incompatible {
                actual_protocol,
                actual_build,
            } => format!(
                "search service endpoint is incompatible: expected_protocol={}, actual_protocol={actual_protocol}, expected_build={}, actual_build={actual_build}",
                PROTOCOL_VERSION,
                daemon_build_id()
            ),
        }
    }
}

pub(super) async fn inspect_search_endpoint(
    socket_path: &Path,
    expected_main_pid: NonZeroU32,
) -> Result<SearchServiceStatus, SearchEndpointProbeFailure> {
    tokio::time::timeout(
        SEARCH_ENDPOINT_INSPECTION_TIMEOUT,
        inspect_search_endpoint_without_timeout(socket_path, expected_main_pid),
    )
    .await
    .map_err(|_| SearchEndpointProbeFailure::TimedOut)?
}

async fn inspect_search_endpoint_without_timeout(
    socket_path: &Path,
    expected_main_pid: NonZeroU32,
) -> Result<SearchServiceStatus, SearchEndpointProbeFailure> {
    let mut stream = UnixStream::connect(socket_path).await.map_err(|error| {
        SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
            "could not connect to search service endpoint: {error}"
        ))
    })?;
    let peer_pid = stream
        .peer_cred()
        .map_err(|error| {
            SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
                "could not inspect search service endpoint owner: {error}"
            ))
        })?
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            SearchEndpointProbeFailure::RestartRequired(
                "search service endpoint did not report a valid peer pid".to_owned(),
            )
        })?;
    if peer_pid != expected_main_pid {
        return Err(SearchEndpointProbeFailure::RestartRequired(format!(
            "search service endpoint owner pid {} does not match systemd MainPID {}",
            peer_pid, expected_main_pid
        )));
    }

    // PID、版本和状态必须来自同一连接，避免在 daemon 重启期间接受混合快照。
    write_service_request(&mut stream, &SearchServiceRequest::Version)
        .await
        .map_err(|error| {
            SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
                "could not request search service version: {error}"
            ))
        })?;
    match read_service_event(&mut stream).await.map_err(|error| {
        SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
            "could not read search service version: {error}"
        ))
    })? {
        SearchServiceEvent::Version { protocol, build }
            if protocol == PROTOCOL_VERSION && build == daemon_build_id() => {}
        SearchServiceEvent::Version { protocol, build } => {
            return Err(SearchEndpointProbeFailure::Incompatible {
                actual_protocol: protocol,
                actual_build: build,
            });
        }
        event => {
            return Err(SearchEndpointProbeFailure::RestartRequired(format!(
                "search service returned an unexpected version event: {event:?}"
            )));
        }
    }

    write_service_request(&mut stream, &SearchServiceRequest::Status)
        .await
        .map_err(|error| {
            SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
                "could not request search service status: {error}"
            ))
        })?;
    match read_service_event(&mut stream).await.map_err(|error| {
        SearchEndpointProbeFailure::TemporarilyUnavailable(format!(
            "could not read search service status: {error}"
        ))
    })? {
        SearchServiceEvent::Status(service_status) => Ok(service_status),
        event => Err(SearchEndpointProbeFailure::RestartRequired(format!(
            "search service returned an unexpected status event: {event:?}"
        ))),
    }
}
