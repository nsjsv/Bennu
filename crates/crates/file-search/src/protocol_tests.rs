use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::database::{IndexedFile, SearchDatabase};
use crate::error::SearchError;
use crate::extractor::ExtractionStatus;
use crate::model::{
    IndexHealth, IndexPhase, IndexStatus, IndexedQueryAvailability, MimePattern,
    SearchEntryTypeRule, SearchFileKind, SearchProviderFailure, SearchQuery, SearchServiceEvent,
    SearchServicePhase, SearchServiceRequest, SearchServiceStatus, PROTOCOL_VERSION,
};

use super::{
    read_service_event, read_service_request, read_service_request_before, search_via_socket,
    search_via_socket_with_cancellation, serve_bound_search_socket, serve_search_socket,
    shutdown_via_socket, status_via_socket, validate_wire_query, version_via_socket,
    write_service_event, write_service_request, BoundSearchSocket, SearchSocketService,
    MAX_ACTIVE_CLIENTS, MAX_QUERY_ENTRY_TYPE_RULES, MAX_QUERY_TERMS_BYTES, MAX_REQUEST_FRAME_BYTES,
};

struct UnavailableSearchService;

impl SearchSocketService for UnavailableSearchService {
    fn status(&self) -> SearchServiceStatus {
        SearchServiceStatus {
            phase: SearchServicePhase::Starting,
            query_availability: IndexedQueryAvailability::Unavailable {
                message: "index is opening".to_owned(),
            },
            index_status: None,
        }
    }

    fn open_query_reader(&self) -> Result<SearchDatabase, SearchProviderFailure> {
        Err(SearchProviderFailure::Unavailable {
            message: "index is opening".to_owned(),
        })
    }
}

struct BlockingSearchService {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Arc<Barrier>,
}

fn complete_index_status(visible_indexed_files: u64) -> IndexStatus {
    IndexStatus {
        phase: IndexPhase::Complete,
        visible_indexed_files,
        health: IndexHealth::Healthy,
        capabilities: Vec::new(),
        path_configuration: Default::default(),
    }
}

impl SearchSocketService for BlockingSearchService {
    fn status(&self) -> SearchServiceStatus {
        SearchServiceStatus {
            phase: SearchServicePhase::Ready,
            query_availability: IndexedQueryAvailability::Available,
            index_status: Some(complete_index_status(0)),
        }
    }

    fn open_query_reader(&self) -> Result<SearchDatabase, SearchProviderFailure> {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        self.release.wait();
        SearchDatabase::in_memory().map_err(|error| SearchProviderFailure::Fatal {
            message: error.to_string(),
        })
    }
}

fn insert_note(database: &SearchDatabase, parent_path: &std::path::Path) {
    database
        .upsert_file(&IndexedFile {
            path: parent_path.join("note.txt"),
            parent_path: parent_path.to_path_buf(),
            display_name: "note.txt".to_owned(),
            kind: SearchFileKind::File,
            size: 5,
            modified_ms: Some(1),
            accessed_ms: None,
            created_ms: None,
            mime_type: Some("text/plain".to_owned()),
            stage_state: crate::IndexedEntryStageState {
                metadata: crate::EntryStageProgress::Complete,
                content: crate::EntryStageProgress::Complete,
            },
            content: Some("needle".to_owned()),
            extraction_status: ExtractionStatus::Indexed,
            device: Some(1),
            inode: Some(1),
            mtime_ns: Some(1),
            ctime_ns: Some(1),
        })
        .unwrap();
}

async fn wait_for_socket_path(socket_path: &std::path::Path) {
    for _ in 0..50 {
        if socket_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("socket was not bound: {socket_path:?}");
}

#[tokio::test]
async fn consecutive_request_frames_round_trip_without_trailing_payload() {
    let (mut client, mut server) = duplex(2048);
    let first = SearchServiceRequest::Version;
    let mut search_query = SearchQuery::global(7, "needle");
    search_query.filters.entry_type_rules = vec![
        SearchEntryTypeRule::Mime(MimePattern::Exact("application/pdf".to_owned())),
        SearchEntryTypeRule::Mime(MimePattern::Prefix("image/".to_owned())),
    ];
    let second = SearchServiceRequest::Search(search_query);

    write_service_request(&mut client, &first).await.unwrap();
    write_service_request(&mut client, &second).await.unwrap();

    assert_eq!(read_service_request(&mut server).await.unwrap(), first);
    assert_eq!(read_service_request(&mut server).await.unwrap(), second);
}

#[tokio::test]
async fn request_frame_limit_is_checked_before_payload_allocation() {
    let (mut client, mut server) = duplex(16);
    client.write_u32(MAX_REQUEST_FRAME_BYTES + 1).await.unwrap();

    assert!(matches!(
        read_service_request(&mut server).await,
        Err(SearchError::ProtocolFrameTooLarge(size))
            if size == MAX_REQUEST_FRAME_BYTES + 1
    ));
}

#[tokio::test]
async fn idle_request_deadline_releases_the_client_slot() {
    let (_client, mut server) = duplex(16);

    assert!(
        read_service_request_before(&mut server, Duration::from_millis(10))
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn wire_query_validation_rejects_resource_amplifying_fields() {
    let mut query = SearchQuery::global(1, "x".repeat(MAX_QUERY_TERMS_BYTES + 1));
    assert!(validate_wire_query(&query).is_err());

    query = SearchQuery::global(1, "needle");
    query.limit = 201;
    assert!(validate_wire_query(&query).is_err());

    query.limit = 50;
    query.filters.modified = Some(crate::TimeRange {
        start_ms: 2,
        end_ms: 1,
    });
    assert!(validate_wire_query(&query).is_err());

    query.filters.modified = None;
    query.filters.entry_type_rules =
        vec![SearchEntryTypeRule::Mime(MimePattern::Exact(String::new()))];
    assert!(validate_wire_query(&query).is_err());

    query.filters.entry_type_rules = vec![SearchEntryTypeRule::Mime(MimePattern::Prefix(
        "ímage/".to_owned(),
    ))];
    assert!(validate_wire_query(&query).is_err());

    query.filters.entry_type_rules = vec![SearchEntryTypeRule::Mime(MimePattern::Exact(
        "text/plain' OR 1=1 --".to_owned(),
    ))];
    assert!(validate_wire_query(&query).is_err());

    query.filters.entry_type_rules = (0..=MAX_QUERY_ENTRY_TYPE_RULES)
        .map(|index| {
            SearchEntryTypeRule::Mime(MimePattern::Exact(format!("application/x-{index}")))
        })
        .collect();
    assert!(validate_wire_query(&query).is_err());

    query.filters.entry_type_rules = vec![
        SearchEntryTypeRule::Kind(SearchFileKind::Directory),
        SearchEntryTypeRule::Mime(MimePattern::Exact("application/pdf".to_owned())),
        SearchEntryTypeRule::Mime(MimePattern::Prefix(
            "application/vnd.openxmlformats-officedocument.".to_owned(),
        )),
    ];
    assert!(validate_wire_query(&query).is_ok());
}

#[test]
fn wire_query_validation_rejects_invalid_extension_filters() {
    let mut query = SearchQuery::global(1, "needle");
    query.filters.extensions = vec!["pdf".to_owned(), ".PDF".to_owned()];
    assert!(validate_wire_query(&query).is_ok());

    query.filters.extensions = vec![String::new()];
    assert!(validate_wire_query(&query).is_err());

    query.filters.extensions = vec!["pd*f".to_owned()];
    assert!(validate_wire_query(&query).is_err());

    query.filters.extensions = vec!["pdf %_".to_owned()];
    assert!(validate_wire_query(&query).is_err());

    query.filters.extensions = vec!["x".repeat(crate::model::MAX_EXTENSION_TOKEN_BYTES + 1)];
    assert!(validate_wire_query(&query).is_err());

    query.filters.extensions = (0..=crate::model::MAX_QUERY_EXTENSIONS)
        .map(|index| format!("x{index}"))
        .collect();
    assert!(validate_wire_query(&query).is_err());
}

#[tokio::test]
async fn socket_server_never_owns_more_than_the_fixed_client_limit() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(UnavailableSearchService),
        || async { Ok(()) },
    ));

    let mut admitted_clients = Vec::new();
    for _ in 0..MAX_ACTIVE_CLIENTS {
        let mut stream = UnixStream::connect(&socket_path).await.unwrap();
        write_service_request(&mut stream, &SearchServiceRequest::Version)
            .await
            .unwrap();
        assert!(matches!(
            read_service_event(&mut stream).await.unwrap(),
            SearchServiceEvent::Version { .. }
        ));
        admitted_clients.push(stream);
    }

    let mut queued_client = UnixStream::connect(&socket_path).await.unwrap();
    write_service_request(&mut queued_client, &SearchServiceRequest::Version)
        .await
        .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(30),
        read_service_event(&mut queued_client)
    )
    .await
    .is_err());

    drop(admitted_clients.pop());
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            read_service_event(&mut queued_client)
        )
        .await
        .unwrap()
        .unwrap(),
        SearchServiceEvent::Version { .. }
    ));

    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_reader_open_does_not_block_status_on_the_current_thread_runtime() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let (entered_sender, entered_receiver) = oneshot::channel();
    let release = Arc::new(Barrier::new(2));
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(BlockingSearchService {
            entered: Mutex::new(Some(entered_sender)),
            release: Arc::clone(&release),
        }),
        || async { Ok(()) },
    ));

    let search_socket_path = socket_path.clone();
    let search = tokio::spawn(async move {
        search_via_socket(&search_socket_path, SearchQuery::global(1, "blocked")).await
    });
    entered_receiver.await.unwrap();

    let status = tokio::time::timeout(Duration::from_secs(1), status_via_socket(&socket_path))
        .await
        .expect("Status must remain responsive while SQLite search is blocked")
        .unwrap();
    assert_eq!(status.phase, SearchServicePhase::Ready);

    release.wait();
    search.await.unwrap().unwrap();
    shutdown_via_socket(&socket_path).await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn matching_cancel_is_the_only_terminal_event_and_does_not_interrupt_another_client() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let database_path = temporary_directory.path().join("search.sqlite");
    let writer_database = SearchDatabase::open(&database_path).unwrap();
    insert_note(&writer_database, temporary_directory.path());
    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        database_path,
        complete_index_status(1),
    ));
    wait_for_socket_path(&socket_path).await;

    let mut cancelled_client = UnixStream::connect(&socket_path).await.unwrap();
    let mut independent_client = UnixStream::connect(&socket_path).await.unwrap();
    write_service_request(
        &mut cancelled_client,
        &SearchServiceRequest::Search(SearchQuery::global(11, "needle")),
    )
    .await
    .unwrap();
    write_service_request(
        &mut cancelled_client,
        &SearchServiceRequest::Cancel { query_id: 11 },
    )
    .await
    .unwrap();
    write_service_request(
        &mut cancelled_client,
        &SearchServiceRequest::Cancel { query_id: 11 },
    )
    .await
    .unwrap();
    write_service_request(
        &mut independent_client,
        &SearchServiceRequest::Search(SearchQuery::global(12, "needle")),
    )
    .await
    .unwrap();

    assert_eq!(
        read_service_event(&mut cancelled_client).await.unwrap(),
        SearchServiceEvent::Cancelled { query_id: 11 }
    );
    assert!(matches!(
        read_service_event(&mut independent_client).await.unwrap(),
        SearchServiceEvent::Results(batch) if batch.query_id == 12 && batch.hits.len() == 1
    ));
    assert!(tokio::time::timeout(
        Duration::from_millis(30),
        read_service_event(&mut cancelled_client)
    )
    .await
    .is_err());

    shutdown_via_socket(&socket_path).await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn client_write_eof_stops_the_active_query_without_a_terminal_event() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let database_path = temporary_directory.path().join("search.sqlite");
    let writer_database = SearchDatabase::open(&database_path).unwrap();
    insert_note(&writer_database, temporary_directory.path());
    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        database_path,
        complete_index_status(1),
    ));
    wait_for_socket_path(&socket_path).await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    write_service_request(
        &mut client,
        &SearchServiceRequest::Search(SearchQuery::global(15, "needle")),
    )
    .await
    .unwrap();
    client.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    client.read_to_end(&mut bytes).await.unwrap();

    assert!(bytes.is_empty());
    shutdown_via_socket(&socket_path).await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellable_client_waits_for_the_server_to_finish_cancelling() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let (entered_sender, entered_receiver) = oneshot::channel();
    let release = Arc::new(Barrier::new(2));
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(BlockingSearchService {
            entered: Mutex::new(Some(entered_sender)),
            release: Arc::clone(&release),
        }),
        || async { Ok(()) },
    ));
    let cancellation = CancellationToken::new();
    let search_socket_path = socket_path.clone();
    let search_cancellation = cancellation.clone();
    let search = tokio::spawn(async move {
        search_via_socket_with_cancellation(
            &search_socket_path,
            SearchQuery::global(13, "blocked"),
            search_cancellation,
        )
        .await
    });
    entered_receiver.await.unwrap();

    cancellation.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!search.is_finished());
    release.wait();

    assert!(matches!(search.await.unwrap(), Err(SearchError::Cancelled)));
    shutdown_via_socket(&socket_path).await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_the_active_query_worker_to_exit() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let socket_path = temporary_directory.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let (entered_sender, entered_receiver) = oneshot::channel();
    let release = Arc::new(Barrier::new(2));
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(BlockingSearchService {
            entered: Mutex::new(Some(entered_sender)),
            release: Arc::clone(&release),
        }),
        || async { Ok(()) },
    ));
    let mut search_client = UnixStream::connect(&socket_path).await.unwrap();
    write_service_request(
        &mut search_client,
        &SearchServiceRequest::Search(SearchQuery::global(14, "blocked")),
    )
    .await
    .unwrap();
    entered_receiver.await.unwrap();

    let shutdown_socket_path = socket_path.clone();
    let mut shutdown =
        tokio::spawn(async move { shutdown_via_socket(&shutdown_socket_path).await });
    let premature_shutdown = tokio::time::timeout(Duration::from_millis(30), &mut shutdown).await;
    release.wait();
    assert!(premature_shutdown.is_err());

    shutdown.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn consecutive_event_frames_preserve_typed_search_failure() {
    let (mut server, mut client) = duplex(2048);
    let first = SearchServiceEvent::Version {
        protocol: 2,
        build: "test".to_owned(),
    };
    let second = SearchServiceEvent::SearchFailed {
        query_id: 41,
        failure: SearchProviderFailure::InvalidQuery {
            message: "bad filter".to_owned(),
        },
    };

    write_service_event(&mut server, &first).await.unwrap();
    write_service_event(&mut server, &second).await.unwrap();

    assert_eq!(read_service_event(&mut client).await.unwrap(), first);
    assert_eq!(read_service_event(&mut client).await.unwrap(), second);
}

#[test]
fn active_socket_owner_is_not_unlinked_by_second_bind() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let first_owner = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let original_metadata = std::fs::symlink_metadata(&socket_path).unwrap();
    assert_eq!(original_metadata.permissions().mode() & 0o777, 0o600);

    let second_bind = BoundSearchSocket::bind(socket_path.clone());

    assert!(matches!(
        second_bind,
        Err(SearchError::SocketAlreadyOwned { .. })
    ));
    let current_metadata = std::fs::symlink_metadata(&socket_path).unwrap();
    assert_eq!(original_metadata.dev(), current_metadata.dev());
    assert_eq!(original_metadata.ino(), current_metadata.ino());
    std::os::unix::net::UnixStream::connect(&socket_path).unwrap();

    drop(first_owner);
    assert!(!socket_path.exists());
}

#[test]
fn active_peer_without_instance_lock_is_not_reclaimed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let active_listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let original_metadata = std::fs::symlink_metadata(&socket_path).unwrap();

    let bind_outcome = BoundSearchSocket::bind(socket_path.clone());

    assert!(matches!(
        bind_outcome,
        Err(SearchError::SocketAlreadyOwned { .. })
    ));
    let current_metadata = std::fs::symlink_metadata(&socket_path).unwrap();
    assert_eq!(original_metadata.dev(), current_metadata.dev());
    assert_eq!(original_metadata.ino(), current_metadata.ino());
    drop(active_listener);
    std::fs::remove_file(socket_path).unwrap();
}

#[test]
fn stale_socket_is_reclaimed_after_owner_has_gone() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let stale_listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    drop(stale_listener);

    let owner = BoundSearchSocket::bind(socket_path.clone()).unwrap();

    std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
    drop(owner);
    assert!(!socket_path.exists());
}

#[test]
fn owner_cleanup_keeps_replacement_socket_with_different_identity() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let owner = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    std::fs::remove_file(&socket_path).unwrap();
    let replacement = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let replacement_metadata = std::fs::symlink_metadata(&socket_path).unwrap();

    drop(owner);

    let current_metadata = std::fs::symlink_metadata(&socket_path).unwrap();
    assert_eq!(replacement_metadata.dev(), current_metadata.dev());
    assert_eq!(replacement_metadata.ino(), current_metadata.ino());
    drop(replacement);
    std::fs::remove_file(socket_path).unwrap();
}

#[tokio::test]
async fn search_client_preserves_query_id_and_unavailable_failure() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(UnavailableSearchService),
        || async { Ok(()) },
    ));

    let search_error = search_via_socket(&socket_path, SearchQuery::global(23, "needle"))
        .await
        .unwrap_err();

    assert!(matches!(
        search_error,
        SearchError::SearchFailed {
            query_id: 23,
            failure: SearchProviderFailure::Unavailable { .. }
        }
    ));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn control_plane_responds_without_query_core() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(UnavailableSearchService),
        || async { Ok(()) },
    ));

    assert_eq!(
        version_via_socket(&socket_path).await.unwrap().0,
        PROTOCOL_VERSION
    );
    let status = status_via_socket(&socket_path).await.unwrap();
    assert_eq!(status.phase, SearchServicePhase::Starting);
    assert!(matches!(
        status.query_availability,
        IndexedQueryAvailability::Unavailable { .. }
    ));

    shutdown_via_socket(&socket_path).await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_ignores_only_missing_or_refused_connect_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_socket_path = temp_dir.path().join("missing.sock");
    shutdown_via_socket(&missing_socket_path).await.unwrap();

    let refused_socket_path = temp_dir.path().join("refused.sock");
    let listener = std::os::unix::net::UnixListener::bind(&refused_socket_path).unwrap();
    drop(listener);
    shutdown_via_socket(&refused_socket_path).await.unwrap();

    let non_directory_path = temp_dir.path().join("not-a-directory");
    std::fs::write(&non_directory_path, b"fixture").unwrap();
    let invalid_socket_path = non_directory_path.join("search.sock");
    assert!(shutdown_via_socket(&invalid_socket_path).await.is_err());
}

#[tokio::test]
async fn socket_search_returns_indexed_results() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let db_path = temp_dir.path().join("search.sqlite");
    let writer_db = SearchDatabase::open(&db_path).unwrap();
    insert_note(&writer_db, temp_dir.path());

    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        db_path,
        complete_index_status(1),
    ));
    wait_for_socket_path(&socket_path).await;

    let batch = search_via_socket(&socket_path, SearchQuery::global(1, "needle"))
        .await
        .unwrap();

    assert_eq!(batch.hits.len(), 1);
    assert_eq!(batch.hits[0].display_name, "note.txt");
    drop(writer_db);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn each_search_client_opens_its_own_read_only_connection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let db_path = temp_dir.path().join("search.sqlite");
    let writer_db = SearchDatabase::open(&db_path).unwrap();
    insert_note(&writer_db, temp_dir.path());

    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        db_path.clone(),
        complete_index_status(1),
    ));
    wait_for_socket_path(&socket_path).await;

    let first_batch = search_via_socket(&socket_path, SearchQuery::global(1, "needle"))
        .await
        .unwrap();
    assert_eq!(first_batch.hits.len(), 1);

    drop(writer_db);
    std::fs::remove_file(&db_path).unwrap();
    assert!(
        search_via_socket(&socket_path, SearchQuery::global(2, "needle"))
            .await
            .is_err()
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn shutdown_is_sticky_for_concurrent_requests_and_idle_client() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let bound_socket = BoundSearchSocket::bind(socket_path.clone()).unwrap();
    let (cleanup_started_sender, cleanup_started_receiver) = oneshot::channel();
    let (cleanup_release_sender, cleanup_release_receiver) = oneshot::channel();
    let server = tokio::spawn(serve_bound_search_socket(
        bound_socket,
        Arc::new(UnavailableSearchService),
        move || async move {
            cleanup_started_sender.send(()).unwrap();
            cleanup_release_receiver.await.unwrap();
            Ok(())
        },
    ));

    let mut idle_client = UnixStream::connect(&socket_path).await.unwrap();
    let mut first_shutdown = UnixStream::connect(&socket_path).await.unwrap();
    let mut second_shutdown = UnixStream::connect(&socket_path).await.unwrap();
    for client in [&mut idle_client, &mut first_shutdown, &mut second_shutdown] {
        write_service_request(client, &SearchServiceRequest::Version)
            .await
            .unwrap();
        assert!(matches!(
            read_service_event(client).await.unwrap(),
            SearchServiceEvent::Version { .. }
        ));
    }
    write_service_request(&mut first_shutdown, &SearchServiceRequest::Shutdown)
        .await
        .unwrap();
    write_service_request(&mut second_shutdown, &SearchServiceRequest::Shutdown)
        .await
        .unwrap();
    cleanup_started_receiver.await.unwrap();

    assert!(UnixStream::connect(&socket_path).await.is_err());

    let mut byte = [0_u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(20), first_shutdown.read(&mut byte))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second_shutdown.read(&mut byte))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), idle_client.read(&mut byte))
            .await
            .is_err()
    );
    assert!(socket_path.exists());

    cleanup_release_sender.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert_eq!(first_shutdown.read(&mut byte).await.unwrap(), 0);
    assert_eq!(second_shutdown.read(&mut byte).await.unwrap(), 0);
    assert_eq!(idle_client.read(&mut byte).await.unwrap(), 0);
    assert!(!socket_path.exists());
}
