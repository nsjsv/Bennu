use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::duplex;

use crate::database::{EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase};
use crate::extractor::ExtractionStatus;
use crate::model::{
    IndexHealth, IndexPhase, IndexStatus, IndexedQueryAvailability, MatchSource, SearchFileKind,
    SearchHit, SearchMatchMode, SearchPathConfigurationPhase, SearchPathConfigurationStatus,
    SearchQuery, SearchResultBatch, SearchRootAvailability, SearchRootStatus, SearchScope,
    SearchServiceEvent, SearchServicePhase, SearchServiceRequest, SearchServiceStatus,
    SearchTextScope,
};
use crate::{SearchPathPreferences, VersionedSearchPathPreferences};

use super::{
    read_service_event, read_service_request, search_via_socket, serve_search_socket,
    write_service_event, write_service_request,
};

fn indexed_file(path: PathBuf) -> IndexedFile {
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        path,
        display_name: "note.txt".to_owned(),
        kind: SearchFileKind::File,
        size: 6,
        modified_ms: Some(1),
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: Some("needle".to_owned()),
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(1),
        mtime_ns: Some(1),
        ctime_ns: Some(1),
    }
}

fn ready_status(visible_indexed_files: u64) -> IndexStatus {
    IndexStatus {
        phase: IndexPhase::Complete,
        visible_indexed_files,
        health: IndexHealth::Healthy,
        capabilities: Vec::new(),
        path_configuration: Default::default(),
    }
}

async fn wait_for_socket(path: &Path) {
    for _ in 0..50 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("socket was not bound: {path:?}");
}

#[tokio::test]
async fn non_utf8_scope_hit_and_phase_round_trip_through_protocol_frames() {
    let scope = PathBuf::from(OsString::from_vec(b"/tmp/\x80".to_vec()));
    let hit_path = scope.join("note.txt");
    let request = SearchServiceRequest::Search(SearchQuery {
        query_id: 9,
        terms: "needle".to_owned(),
        text_scope: SearchTextScope::NameAndContent,
        match_mode: SearchMatchMode::Plain,
        scope: SearchScope::Directory(scope.clone()),
        recursive: true,
        filters: Default::default(),
        limit: 50,
        cursor: None,
    });
    let result_event = SearchServiceEvent::Results(SearchResultBatch {
        query_id: 9,
        hits: vec![SearchHit {
            path: hit_path,
            display_name: "note.txt".to_owned(),
            kind: SearchFileKind::File,
            size: 1,
            modified_ms: None,
            accessed_ms: None,
            created_ms: None,
            rank: 1.0,
            snippet: None,
            match_source: MatchSource::Name,
        }],
        next_cursor: None,
        finished: true,
    });
    let status_event = SearchServiceEvent::Status(SearchServiceStatus {
        phase: SearchServicePhase::Ready,
        query_availability: IndexedQueryAvailability::Available,
        index_status: Some(IndexStatus {
            phase: IndexPhase::Crawling {
                scanned_entries: 4,
                current_scope: scope,
            },
            visible_indexed_files: 3,
            health: IndexHealth::Healthy,
            capabilities: Vec::new(),
            path_configuration: Default::default(),
        }),
    });
    let (mut request_writer, mut request_reader) = duplex(2048);
    let (mut event_writer, mut event_reader) = duplex(2048);

    write_service_request(&mut request_writer, &request)
        .await
        .unwrap();
    write_service_event(&mut event_writer, &result_event)
        .await
        .unwrap();
    write_service_event(&mut event_writer, &status_event)
        .await
        .unwrap();

    assert_eq!(
        read_service_request(&mut request_reader).await.unwrap(),
        request
    );
    assert_eq!(
        read_service_event(&mut event_reader).await.unwrap(),
        result_event
    );
    assert_eq!(
        read_service_event(&mut event_reader).await.unwrap(),
        status_event
    );
}

#[tokio::test]
async fn non_utf8_path_configuration_round_trips_in_desired_effective_and_status_fields() {
    let custom_root = PathBuf::from(OsString::from_vec(b"/media/\x80".to_vec()));
    let exclusion = PathBuf::from(OsString::from_vec(b"/media/\x80/private-\x81".to_vec()));
    let desired = VersionedSearchPathPreferences {
        revision: 8,
        preferences: SearchPathPreferences {
            custom_roots: vec![custom_root.clone()],
            exclusions: vec![exclusion.clone()],
        },
    };
    let event = SearchServiceEvent::PathConfiguration {
        configuration: desired.clone(),
        status: SearchPathConfigurationStatus {
            desired_revision: 8,
            effective_revision: 7,
            effective_preferences: desired.preferences.clone(),
            phase: SearchPathConfigurationPhase::Applying,
            roots: vec![SearchRootStatus {
                path: custom_root,
                availability: SearchRootAvailability::Available,
            }],
        },
    };
    let request = SearchServiceRequest::ConfigurePathPreferences {
        expected_revision: 7,
        preferences: desired.preferences,
    };
    let (mut request_writer, mut request_reader) = duplex(4096);
    let (mut event_writer, mut event_reader) = duplex(4096);

    write_service_request(&mut request_writer, &request)
        .await
        .unwrap();
    write_service_event(&mut event_writer, &event)
        .await
        .unwrap();

    assert_eq!(
        read_service_request(&mut request_reader).await.unwrap(),
        request
    );
    assert_eq!(read_service_event(&mut event_reader).await.unwrap(), event);
    assert_eq!(
        exclusion.as_os_str().as_bytes(),
        b"/media/\x80/private-\x81"
    );
}

#[tokio::test]
async fn socket_search_preserves_two_lossy_colliding_path_identities() {
    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("search.sock");
    let database_path = directory.path().join("search.sqlite");
    let mut first_root_bytes = directory.path().as_os_str().as_bytes().to_vec();
    first_root_bytes.extend_from_slice(b"/\x80");
    let mut second_root_bytes = directory.path().as_os_str().as_bytes().to_vec();
    second_root_bytes.extend_from_slice(b"/\x81");
    let first_root = PathBuf::from(OsString::from_vec(first_root_bytes));
    let second_root = PathBuf::from(OsString::from_vec(second_root_bytes));
    assert_eq!(first_root.to_string_lossy(), second_root.to_string_lossy());
    let first_path = first_root.join("note.txt");
    let second_path = second_root.join("note.txt");
    let writer_database = SearchDatabase::open(&database_path).unwrap();
    writer_database
        .upsert_file(&indexed_file(first_path.clone()))
        .unwrap();
    writer_database
        .upsert_file(&indexed_file(second_path.clone()))
        .unwrap();

    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        database_path,
        ready_status(2),
    ));
    wait_for_socket(&socket_path).await;

    let global_paths = search_via_socket(&socket_path, SearchQuery::global(1, "needle"))
        .await
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        global_paths,
        BTreeSet::from([first_path.clone(), second_path])
    );
    let scoped_paths = search_via_socket(
        &socket_path,
        SearchQuery {
            query_id: 2,
            terms: "needle".to_owned(),
            text_scope: SearchTextScope::NameAndContent,
            match_mode: SearchMatchMode::Plain,
            scope: SearchScope::Directory(first_root),
            recursive: true,
            filters: Default::default(),
            limit: 50,
            cursor: None,
        },
    )
    .await
    .unwrap()
    .hits
    .into_iter()
    .map(|hit| hit.path)
    .collect::<Vec<_>>();
    assert_eq!(scoped_paths, vec![first_path]);

    drop(writer_database);
    server.abort();
    let _ = server.await;
}
