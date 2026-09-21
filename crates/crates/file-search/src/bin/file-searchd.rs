use std::ffi::{OsStr, OsString};
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use file_search::{
    daemon_build_id, read_service_event, serve_bound_search_socket, shutdown_connected_service,
    shutdown_via_socket, write_service_request, BoundSearchSocket, SearchIndexConfig,
    SearchRuntimeIdentity, SearchServiceEvent, SearchServiceRequest, SearchServiceRuntime,
    SearchSocketService, PROTOCOL_VERSION,
};
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};

mod file_searchd_runtime_logging;

use file_searchd_runtime_logging::{bounded_daemon_log_detail, init_runtime_logging};

const EXISTING_ENDPOINT_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const EXISTING_ENDPOINT_CONNECT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonInvocation {
    Serve,
    ShutdownExisting,
    CheckExisting { expected_main_pid: NonZeroU32 },
}

impl DaemonInvocation {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let invocation = match arguments.next() {
            None => Self::Serve,
            Some(argument) if argument == "--shutdown-existing" => Self::ShutdownExisting,
            Some(argument) if argument == "--check-existing" => {
                let expected_main_pid = arguments
                    .next()
                    .ok_or_else(|| "--check-existing requires a systemd MainPID".to_owned())?;
                let expected_main_pid = expected_main_pid
                    .to_str()
                    .ok_or_else(|| "--check-existing MainPID must be valid UTF-8".to_owned())?
                    .parse::<u32>()
                    .ok()
                    .and_then(NonZeroU32::new)
                    .ok_or_else(|| {
                        "--check-existing MainPID must be a positive 32-bit integer".to_owned()
                    })?;
                Self::CheckExisting { expected_main_pid }
            }
            Some(argument) => {
                return Err(format!(
                    "unknown file-searchd argument: {}",
                    argument.to_string_lossy()
                ));
            }
        };
        if let Some(argument) = arguments.next() {
            return Err(format!(
                "unexpected file-searchd argument: {}",
                argument.to_string_lossy()
            ));
        }
        Ok(invocation)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    init_runtime_logging();
    if let Err(error) = run().await {
        let log_error = bounded_daemon_log_detail(&error.to_string());
        tracing::error!(
            target: "file_search::daemon",
            event = "daemon_fatal",
            error = %log_error,
            "file-searchd stopped with a fatal error"
        );
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let runtime_identity = SearchRuntimeIdentity::from_environment()?;
    let socket_path = runtime_identity.socket_path();
    match DaemonInvocation::parse(std::env::args_os().skip(1))? {
        DaemonInvocation::Serve => serve(socket_path).await,
        DaemonInvocation::ShutdownExisting => shutdown_existing(&socket_path).await,
        DaemonInvocation::CheckExisting { expected_main_pid } => {
            check_existing_with_timeout(&socket_path, expected_main_pid).await
        }
    }
}

async fn check_existing_with_timeout(
    socket_path: &Path,
    expected_main_pid: NonZeroU32,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(
        EXISTING_ENDPOINT_CHECK_TIMEOUT,
        check_existing(socket_path, expected_main_pid),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "search service endpoint check timed out",
        )
    })?
}

async fn check_existing(
    socket_path: &Path,
    expected_main_pid: NonZeroU32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut peer_stream = connect_existing_endpoint(socket_path).await?;
    let peer_process_id = connected_peer_process_id(&peer_stream)?;
    if peer_process_id != expected_main_pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "search socket peer pid {} does not match systemd MainPID {}",
                peer_process_id, expected_main_pid
            ),
        )
        .into());
    }

    // 所有身份字段必须来自同一连接，避免在 systemd 重启期间接受混合快照。
    write_service_request(&mut peer_stream, &SearchServiceRequest::Version).await?;
    validate_peer_version(read_service_event(&mut peer_stream).await?)?;

    write_service_request(&mut peer_stream, &SearchServiceRequest::Status).await?;
    match read_service_event(&mut peer_stream).await? {
        SearchServiceEvent::Status(_) => Ok(()),
        event => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("search service returned an unexpected status response: {event:?}"),
        )
        .into()),
    }
}

async fn connect_existing_endpoint(socket_path: &Path) -> io::Result<UnixStream> {
    loop {
        match UnixStream::connect(socket_path).await {
            Ok(peer_stream) => return Ok(peer_stream),
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                // Type=exec 会先进入 active，再由 daemon 完成 socket bind；只重试这个启动窗口。
                tokio::time::sleep(EXISTING_ENDPOINT_CONNECT_INTERVAL).await;
            }
            Err(source) => return Err(source),
        }
    }
}

async fn shutdown_existing(socket_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut peer_stream = match UnixStream::connect(socket_path).await {
        Ok(peer_stream) => peer_stream,
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(());
        }
        Err(source) => return Err(source.into()),
    };
    let peer_process_id = connected_peer_process_id(&peer_stream)?;
    let executable_path = std::fs::read_link(
        Path::new("/proc")
            .join(peer_process_id.to_string())
            .join("exe"),
    )?;
    if !is_file_searchd_executable_path(&executable_path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to shut down search socket owner {peer_process_id}: {}",
                executable_path.display()
            ),
        )
        .into());
    }

    write_service_request(&mut peer_stream, &SearchServiceRequest::Version).await?;
    let version_event = read_service_event(&mut peer_stream).await?;
    validate_peer_version(version_event)?;

    shutdown_connected_service(peer_stream).await?;
    Ok(())
}

fn connected_peer_process_id(peer_stream: &UnixStream) -> io::Result<NonZeroU32> {
    peer_stream
        .peer_cred()?
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .and_then(NonZeroU32::new)
        .ok_or_else(|| io::Error::other("search socket peer has no valid process ID"))
}

fn validate_peer_version(
    version_event: SearchServiceEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    match version_event {
        SearchServiceEvent::Version { protocol, build }
            if protocol == PROTOCOL_VERSION && build == daemon_build_id() =>
        {
            Ok(())
        }
        SearchServiceEvent::Version { protocol, build } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "search service is incompatible: expected_protocol={}, actual_protocol={protocol}, expected_build={}, actual_build={build}",
                PROTOCOL_VERSION,
                daemon_build_id()
            ),
        )
        .into()),
        event => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("search service returned an unexpected version response: {event:?}"),
        )
        .into()),
    }
}

fn is_file_searchd_executable_path(executable_path: &Path) -> bool {
    matches!(
        executable_path.file_name(),
        Some(file_name)
            if file_name == OsStr::new("bennu-searchd")
                || file_name == OsStr::new("bennu-searchd (deleted)")
    )
}

async fn serve(socket_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminate_signal = signal(SignalKind::terminate())?;
    let database_path = database_path()?;
    let bound_socket = BoundSearchSocket::bind(socket_path)?;
    let shutdown_socket_path = bound_socket.path().to_path_buf();
    let service_runtime = Arc::new(SearchServiceRuntime::new());
    service_runtime.start_in_background(database_path, SearchIndexConfig::default());

    tracing::info!(
        target: "file_search::daemon",
        event = "daemon_endpoint_ready",
        build = %daemon_build_id(),
        "search service endpoint ready"
    );
    let socket_service: Arc<dyn SearchSocketService> = service_runtime.clone();
    let shutdown_runtime = Arc::clone(&service_runtime);
    let socket_server = serve_bound_search_socket(bound_socket, socket_service, move || {
        let shutdown_runtime = Arc::clone(&shutdown_runtime);
        async move { shutdown_runtime.shutdown() }
    });
    let terminate_shutdown = async move {
        terminate_signal.recv().await.ok_or_else(|| {
            std::io::Error::other("SIGTERM signal stream closed before receiving a signal")
        })?;
        shutdown_via_socket(&shutdown_socket_path).await
    };

    tokio::pin!(socket_server);
    tokio::pin!(terminate_shutdown);
    tokio::select! {
        server_outcome = &mut socket_server => server_outcome?,
        signal_outcome = &mut terminate_shutdown => {
            signal_outcome?;
            socket_server.await?;
        }
    }
    Ok(())
}

fn database_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = dirs::data_dir()
        .or_else(dirs::cache_dir)
        .ok_or("could not find XDG data or cache directory")?;
    // 守护进程独立启动，同样要在读索引前完成改名迁移。
    file_core::data_dir_migration::migrate_legacy_data_dir(&base);
    Ok(base.join("bennu").join("search.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve_compatible_probe(listener: tokio::net::UnixListener) {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(
            file_search::read_service_request(&mut stream)
                .await
                .unwrap(),
            SearchServiceRequest::Version
        );
        file_search::write_service_event(
            &mut stream,
            &SearchServiceEvent::Version {
                protocol: PROTOCOL_VERSION,
                build: daemon_build_id(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            file_search::read_service_request(&mut stream)
                .await
                .unwrap(),
            SearchServiceRequest::Status
        );
        file_search::write_service_event(
            &mut stream,
            &SearchServiceEvent::Status(file_search::SearchServiceStatus {
                phase: file_search::SearchServicePhase::Ready,
                query_availability: file_search::IndexedQueryAvailability::Available,
                index_status: None,
            }),
        )
        .await
        .unwrap();
    }

    #[test]
    fn invocation_defaults_to_serve() {
        assert_eq!(
            DaemonInvocation::parse([]).unwrap(),
            DaemonInvocation::Serve
        );
    }

    #[test]
    fn invocation_accepts_shutdown_existing() {
        assert_eq!(
            DaemonInvocation::parse([OsString::from("--shutdown-existing")]).unwrap(),
            DaemonInvocation::ShutdownExisting
        );
    }

    #[test]
    fn invocation_accepts_check_existing_with_main_pid() {
        assert_eq!(
            DaemonInvocation::parse([OsString::from("--check-existing"), OsString::from("42")])
                .unwrap(),
            DaemonInvocation::CheckExisting {
                expected_main_pid: NonZeroU32::new(42).unwrap()
            }
        );
    }

    #[test]
    fn invocation_rejects_unknown_or_extra_arguments() {
        assert!(DaemonInvocation::parse([OsString::from("--unknown")]).is_err());
        assert!(DaemonInvocation::parse([
            OsString::from("--shutdown-existing"),
            OsString::from("extra")
        ])
        .is_err());
        for arguments in [
            vec![OsString::from("--check-existing")],
            vec![OsString::from("--check-existing"), OsString::from("0")],
            vec![
                OsString::from("--check-existing"),
                OsString::from("invalid"),
            ],
            vec![
                OsString::from("--check-existing"),
                OsString::from("42"),
                OsString::from("extra"),
            ],
        ] {
            assert!(DaemonInvocation::parse(arguments).is_err());
        }
    }

    #[test]
    fn owner_path_accepts_file_searchd_and_deleted_executable() {
        assert!(is_file_searchd_executable_path(Path::new(
            "/usr/lib/bennu/bennu-searchd"
        )));
        assert!(is_file_searchd_executable_path(Path::new(
            "/usr/lib/bennu/bennu-searchd (deleted)"
        )));
    }

    #[test]
    fn owner_path_rejects_unknown_executable() {
        assert!(!is_file_searchd_executable_path(Path::new("/usr/bin/bash")));
        assert!(!is_file_searchd_executable_path(Path::new(
            "/usr/lib/bennu/bennu-searchd-old"
        )));
    }

    #[test]
    fn peer_version_requires_an_exact_protocol_and_build_handshake() {
        assert!(validate_peer_version(SearchServiceEvent::Version {
            protocol: PROTOCOL_VERSION,
            build: daemon_build_id(),
        })
        .is_ok());
        assert!(validate_peer_version(SearchServiceEvent::Version {
            protocol: PROTOCOL_VERSION + 1,
            build: daemon_build_id(),
        })
        .is_err());
        assert!(validate_peer_version(SearchServiceEvent::Version {
            protocol: PROTOCOL_VERSION,
            build: "older-build".to_owned(),
        })
        .is_err());
        assert!(validate_peer_version(SearchServiceEvent::Status(
            file_search::SearchServiceStatus {
                phase: file_search::SearchServicePhase::Starting,
                query_availability: file_search::IndexedQueryAvailability::Unavailable {
                    message: "starting".to_owned(),
                },
                index_status: None,
            }
        ))
        .is_err());
    }

    #[tokio::test]
    async fn check_existing_accepts_matching_pid_version_and_status() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let endpoint_server = tokio::spawn(serve_compatible_probe(listener));

        check_existing(&socket_path, NonZeroU32::new(std::process::id()).unwrap())
            .await
            .unwrap();
        endpoint_server.await.unwrap();
    }

    #[tokio::test]
    async fn check_existing_waits_for_endpoint_bind() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let server_socket_path = socket_path.clone();
        let endpoint_server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let listener = tokio::net::UnixListener::bind(server_socket_path).unwrap();
            serve_compatible_probe(listener).await;
        });

        check_existing_with_timeout(&socket_path, NonZeroU32::new(std::process::id()).unwrap())
            .await
            .unwrap();
        endpoint_server.await.unwrap();
    }

    #[tokio::test]
    async fn check_existing_times_out_when_peer_does_not_respond() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let endpoint_server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                file_search::read_service_request(&mut stream)
                    .await
                    .unwrap(),
                SearchServiceRequest::Version
            );
            std::future::pending::<()>().await;
        });

        let endpoint_error = tokio::time::timeout(
            Duration::from_secs(6),
            check_existing_with_timeout(&socket_path, NonZeroU32::new(std::process::id()).unwrap()),
        )
        .await
        .expect("endpoint check exceeded its outer test deadline")
        .unwrap_err();

        assert_eq!(
            endpoint_error.to_string(),
            "search service endpoint check timed out"
        );
        endpoint_server.abort();
    }

    #[tokio::test]
    async fn check_existing_rejects_peer_pid_mismatch() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let endpoint_server = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });
        let unexpected_pid = if std::process::id() == 1 { 2 } else { 1 };

        assert!(
            check_existing(&socket_path, NonZeroU32::new(unexpected_pid).unwrap())
                .await
                .is_err()
        );
        endpoint_server.await.unwrap();
    }

    #[tokio::test]
    async fn check_existing_rejects_incompatible_version() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let endpoint_server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                file_search::read_service_request(&mut stream)
                    .await
                    .unwrap(),
                SearchServiceRequest::Version
            );
            file_search::write_service_event(
                &mut stream,
                &SearchServiceEvent::Version {
                    protocol: PROTOCOL_VERSION + 1,
                    build: "older-build".to_owned(),
                },
            )
            .await
            .unwrap();
        });

        assert!(
            check_existing(&socket_path, NonZeroU32::new(std::process::id()).unwrap(),)
                .await
                .is_err()
        );
        endpoint_server.await.unwrap();
    }

    #[tokio::test]
    async fn check_existing_rejects_non_status_response() {
        let temporary_directory = tempfile::tempdir().unwrap();
        let socket_path = temporary_directory.path().join("search.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let endpoint_server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                file_search::read_service_request(&mut stream)
                    .await
                    .unwrap(),
                SearchServiceRequest::Version
            );
            file_search::write_service_event(
                &mut stream,
                &SearchServiceEvent::Version {
                    protocol: PROTOCOL_VERSION,
                    build: daemon_build_id(),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                file_search::read_service_request(&mut stream)
                    .await
                    .unwrap(),
                SearchServiceRequest::Status
            );
            file_search::write_service_event(
                &mut stream,
                &SearchServiceEvent::Version {
                    protocol: PROTOCOL_VERSION,
                    build: daemon_build_id(),
                },
            )
            .await
            .unwrap();
        });

        assert!(
            check_existing(&socket_path, NonZeroU32::new(std::process::id()).unwrap(),)
                .await
                .is_err()
        );
        endpoint_server.await.unwrap();
    }
}
