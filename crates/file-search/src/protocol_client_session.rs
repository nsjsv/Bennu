use std::io;

use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::database::SearchDatabase;
use crate::error::{SearchError, SearchResult};
use crate::model::{
    daemon_build_id, SearchPathConfigurationStatus, SearchProviderFailure, SearchQuery,
    SearchResultBatch, SearchServiceEvent, SearchServiceRequest, PROTOCOL_VERSION,
};
use crate::{SearchPathPreferences, VersionedSearchPathPreferences};

use super::{
    read_service_request, read_service_request_before, search_provider_failure_from_error,
    service_status, validate_wire_query, wait_for_signal, write_service_event,
    SearchServiceBackend, CLIENT_IDLE_TIMEOUT,
};

struct ActiveClientQuery {
    query_id: u64,
    interrupt_receiver: oneshot::Receiver<rusqlite::InterruptHandle>,
    worker: JoinHandle<ClientQueryCompletion>,
}

struct ClientQueryCompletion {
    reader: Option<SearchDatabase>,
    outcome: Result<SearchResultBatch, SearchProviderFailure>,
}

struct ClientShutdownReadiness(Option<mpsc::UnboundedSender<()>>);

impl ClientShutdownReadiness {
    fn report(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for ClientShutdownReadiness {
    fn drop(&mut self) {
        self.report();
    }
}

pub(super) async fn handle_client(
    mut stream: UnixStream,
    backend: SearchServiceBackend,
    shutdown_requested_sender: watch::Sender<bool>,
    mut shutdown_requested_receiver: watch::Receiver<bool>,
    mut shutdown_finished_receiver: watch::Receiver<bool>,
    client_ready_sender: mpsc::UnboundedSender<()>,
) -> SearchResult<()> {
    let mut shutdown_readiness = ClientShutdownReadiness(Some(client_ready_sender));
    let mut client_reader = None;
    'client: loop {
        let request_outcome = tokio::select! {
            biased;
            request = read_service_request_before(&mut stream, CLIENT_IDLE_TIMEOUT) => request,
            _ = wait_for_signal(&mut shutdown_requested_receiver) => {
                shutdown_readiness.report();
                wait_for_signal(&mut shutdown_finished_receiver).await;
                return Ok(());
            }
            _ = wait_for_signal(&mut shutdown_finished_receiver) => return Ok(()),
        };
        let request = match request_outcome {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(SearchError::ProtocolIo(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match request {
            SearchServiceRequest::Status => {
                write_service_event(
                    &mut stream,
                    &SearchServiceEvent::Status(service_status(&backend)),
                )
                .await?;
            }
            SearchServiceRequest::GetPathConfiguration => {
                write_path_configuration_outcome(
                    &mut stream,
                    backend_path_configuration(&backend),
                    None,
                )
                .await?;
            }
            SearchServiceRequest::ConfigurePathPreferences {
                expected_revision,
                preferences,
            } => {
                let configuration_backend = backend.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    backend_configure_path_preferences(
                        &configuration_backend,
                        expected_revision,
                        preferences,
                    )
                })
                .await
                .map_err(|error| {
                    SearchError::WorkerFailed(format!(
                        "search path configuration worker failed: {error}"
                    ))
                })?;
                let failure_status = backend_path_configuration(&backend)
                    .ok()
                    .map(|(_, status)| status);
                write_path_configuration_outcome(&mut stream, outcome, failure_status).await?;
            }
            SearchServiceRequest::Search(query) => {
                let query_id = query.query_id;
                if let Err(message) = validate_wire_query(&query) {
                    write_service_event(
                        &mut stream,
                        &SearchServiceEvent::SearchFailed {
                            query_id,
                            failure: SearchProviderFailure::InvalidQuery { message },
                        },
                    )
                    .await?;
                    continue;
                }
                if *shutdown_requested_receiver.borrow() {
                    write_service_event(
                        &mut stream,
                        &SearchServiceEvent::SearchFailed {
                            query_id,
                            failure: SearchProviderFailure::Unavailable {
                                message: "search service is shutting down".to_owned(),
                            },
                        },
                    )
                    .await?;
                    continue;
                }

                let mut active_query =
                    spawn_client_query(backend.clone(), client_reader.take(), query);
                loop {
                    tokio::select! {
                        biased;
                        request = read_service_request(&mut stream) => {
                            let request = match request {
                                Ok(request) => request,
                                Err(SearchError::ProtocolIo(error))
                                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                                {
                                    let _ = interrupt_and_join_client_query(active_query).await?;
                                    return Ok(());
                                }
                                Err(error) => {
                                    let _ = interrupt_and_join_client_query(active_query).await?;
                                    return Err(error);
                                }
                            };
                            match request {
                                SearchServiceRequest::Cancel { query_id }
                                    if query_id == active_query.query_id =>
                                {
                                    client_reader = interrupt_and_join_client_query(active_query).await?;
                                    write_service_event(
                                        &mut stream,
                                        &SearchServiceEvent::Cancelled { query_id },
                                    )
                                    .await?;
                                    continue 'client;
                                }
                                SearchServiceRequest::Cancel { .. } => {}
                                SearchServiceRequest::Status => {
                                    write_service_event(
                                        &mut stream,
                                        &SearchServiceEvent::Status(service_status(&backend)),
                                    )
                                    .await?;
                                }
                                SearchServiceRequest::GetPathConfiguration => {
                                    write_path_configuration_outcome(
                                        &mut stream,
                                        backend_path_configuration(&backend),
                                        None,
                                    )
                                    .await?;
                                }
                                SearchServiceRequest::ConfigurePathPreferences { .. } => {
                                    write_service_event(
                                        &mut stream,
                                        &SearchServiceEvent::PathConfigurationFailed {
                                            failure: SearchProviderFailure::Unavailable {
                                                message: "cancel the active search before changing indexed paths"
                                                    .to_owned(),
                                            },
                                            status: backend_path_configuration(&backend)
                                                .ok()
                                                .map(|(_, status)| status),
                                        },
                                    )
                                    .await?;
                                }
                                SearchServiceRequest::Version => {
                                    write_service_event(
                                        &mut stream,
                                        &SearchServiceEvent::Version {
                                            protocol: PROTOCOL_VERSION,
                                            build: daemon_build_id(),
                                        },
                                    )
                                    .await?;
                                }
                                SearchServiceRequest::Shutdown => {
                                    shutdown_requested_sender.send_replace(true);
                                    let _ = interrupt_and_join_client_query(active_query).await?;
                                    shutdown_readiness.report();
                                    wait_for_signal(&mut shutdown_finished_receiver).await;
                                    return Ok(());
                                }
                                SearchServiceRequest::Search(_) => {
                                    let _ = interrupt_and_join_client_query(active_query).await?;
                                    return Err(SearchError::InvalidQuery(
                                        "a client cannot start a second search while one is active"
                                            .to_owned(),
                                    ));
                                }
                            }
                        }
                        _ = wait_for_signal(&mut shutdown_requested_receiver) => {
                            let _ = interrupt_and_join_client_query(active_query).await?;
                            shutdown_readiness.report();
                            wait_for_signal(&mut shutdown_finished_receiver).await;
                            return Ok(());
                        }
                        joined = &mut active_query.worker => {
                            let completion = joined.map_err(|error| {
                                SearchError::WorkerFailed(format!("search worker failed: {error}"))
                            })?;
                            client_reader = completion.reader;
                            let event = match completion.outcome {
                                Ok(batch) => SearchServiceEvent::Results(batch),
                                Err(failure) => SearchServiceEvent::SearchFailed { query_id, failure },
                            };
                            write_service_event(&mut stream, &event).await?;
                            continue 'client;
                        }
                    }
                }
            }
            SearchServiceRequest::Cancel { .. } => {}
            SearchServiceRequest::Version => {
                write_service_event(
                    &mut stream,
                    &SearchServiceEvent::Version {
                        protocol: PROTOCOL_VERSION,
                        build: daemon_build_id(),
                    },
                )
                .await?;
            }
            SearchServiceRequest::Shutdown => {
                shutdown_requested_sender.send_replace(true);
                shutdown_readiness.report();
                wait_for_signal(&mut shutdown_finished_receiver).await;
                return Ok(());
            }
        }
    }
}

fn backend_path_configuration(
    backend: &SearchServiceBackend,
) -> Result<
    (
        VersionedSearchPathPreferences,
        SearchPathConfigurationStatus,
    ),
    SearchProviderFailure,
> {
    match backend {
        SearchServiceBackend::DirectDatabase { .. } => Err(SearchProviderFailure::Unavailable {
            message: "search path configuration requires the daemon core".to_owned(),
        }),
        SearchServiceBackend::Runtime(service) => service.path_configuration(),
    }
}

fn backend_configure_path_preferences(
    backend: &SearchServiceBackend,
    expected_revision: u64,
    preferences: SearchPathPreferences,
) -> Result<
    (
        VersionedSearchPathPreferences,
        SearchPathConfigurationStatus,
    ),
    SearchProviderFailure,
> {
    match backend {
        SearchServiceBackend::DirectDatabase { .. } => Err(SearchProviderFailure::Unavailable {
            message: "search path configuration requires the daemon core".to_owned(),
        }),
        SearchServiceBackend::Runtime(service) => {
            service.configure_path_preferences(expected_revision, preferences)
        }
    }
}

async fn write_path_configuration_outcome(
    stream: &mut UnixStream,
    outcome: Result<
        (
            VersionedSearchPathPreferences,
            SearchPathConfigurationStatus,
        ),
        SearchProviderFailure,
    >,
    failure_status: Option<SearchPathConfigurationStatus>,
) -> SearchResult<()> {
    let event = match outcome {
        Ok((configuration, status)) => SearchServiceEvent::PathConfiguration {
            configuration,
            status,
        },
        Err(failure) => SearchServiceEvent::PathConfigurationFailed {
            failure,
            status: failure_status,
        },
    };
    write_service_event(stream, &event).await
}

fn open_query_reader(
    backend: &SearchServiceBackend,
) -> Result<SearchDatabase, SearchProviderFailure> {
    match backend {
        SearchServiceBackend::DirectDatabase { database_path, .. } => {
            SearchDatabase::open_read_only(database_path)
                .map_err(search_provider_failure_from_error)
        }
        SearchServiceBackend::Runtime(service) => service.open_query_reader(),
    }
}

fn spawn_client_query(
    backend: SearchServiceBackend,
    reader: Option<SearchDatabase>,
    query: SearchQuery,
) -> ActiveClientQuery {
    let query_id = query.query_id;
    let (interrupt_sender, interrupt_receiver) = oneshot::channel();
    let worker = tokio::task::spawn_blocking(move || {
        let reader = match reader {
            Some(reader) => reader,
            None => match open_query_reader(&backend) {
                Ok(reader) => reader,
                Err(failure) => {
                    return ClientQueryCompletion {
                        reader: None,
                        outcome: Err(failure),
                    };
                }
            },
        };
        if interrupt_sender.send(reader.interrupt_handle()).is_err() {
            return ClientQueryCompletion {
                reader: Some(reader),
                outcome: Err(SearchProviderFailure::Unavailable {
                    message: "search client disconnected".to_owned(),
                }),
            };
        }
        let outcome = reader
            .search(&query)
            .map_err(search_provider_failure_from_error);
        ClientQueryCompletion {
            reader: Some(reader),
            outcome,
        }
    });
    ActiveClientQuery {
        query_id,
        interrupt_receiver,
        worker,
    }
}

async fn interrupt_and_join_client_query(
    active_query: ActiveClientQuery,
) -> SearchResult<Option<SearchDatabase>> {
    if let Ok(interrupt) = active_query.interrupt_receiver.await {
        interrupt.interrupt();
    }
    active_query
        .worker
        .await
        .map(|completion| completion.reader)
        .map_err(|error| SearchError::WorkerFailed(format!("search worker failed: {error}")))
}
