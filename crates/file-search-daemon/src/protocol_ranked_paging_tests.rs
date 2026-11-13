use std::path::Path;
use std::time::Duration;

use file_search::search_via_socket;

use crate::database::{IndexedFile, SearchDatabase};
use crate::error::SearchError;
use crate::extractor::ExtractionStatus;
use crate::model::{
    IndexHealth, IndexPhase, IndexStatus, SearchCursor, SearchFileKind, SearchProviderFailure,
    SearchQuery,
};

use super::serve_search_socket;

fn insert_needle(database: &SearchDatabase, path: &Path) {
    database
        .upsert_file(&IndexedFile {
            path: path.to_path_buf(),
            parent_path: path.parent().unwrap().to_path_buf(),
            display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            kind: SearchFileKind::File,
            size: 6,
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

/// 界面每翻一页都重新连接服务：定格必须跨连接可取，失效时客户端拿到结构化的过期失败。
#[tokio::test]
async fn ranked_pages_continue_across_connections_and_report_expiry() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("search.sock");
    let db_path = temp_dir.path().join("search.sqlite");
    let writer_db = SearchDatabase::open(&db_path).unwrap();
    for index in 0..3 {
        insert_needle(
            &writer_db,
            &temp_dir.path().join(format!("note-{index}.txt")),
        );
    }
    let server = tokio::spawn(serve_search_socket(
        socket_path.clone(),
        db_path,
        IndexStatus {
            phase: IndexPhase::Complete,
            visible_indexed_files: 3,
            health: IndexHealth::Healthy,
            capabilities: Vec::new(),
            path_configuration: Default::default(),
        },
    ));
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let mut query = SearchQuery::global(1, "needle");
    query.limit = 1;
    let mut names = Vec::new();
    let mut first_snapshot = None;
    loop {
        let batch = search_via_socket(&socket_path, query.clone())
            .await
            .unwrap();
        names.extend(batch.hits.into_iter().map(|hit| hit.display_name));
        let Some(cursor) = batch.next_cursor else {
            break;
        };
        if let SearchCursor::Snapshot { snapshot_id, .. } = cursor {
            first_snapshot.get_or_insert(snapshot_id);
        }
        query.query_id += 1;
        query.cursor = Some(cursor);
    }
    names.sort();
    assert_eq!(names, ["note-0.txt", "note-1.txt", "note-2.txt"]);

    let mut expired = SearchQuery::global(9, "needle");
    expired.cursor = Some(SearchCursor::Snapshot {
        snapshot_id: first_snapshot.unwrap().wrapping_add(1_000),
        position: 1,
    });
    let error = search_via_socket(&socket_path, expired).await.unwrap_err();
    assert!(matches!(
        error,
        SearchError::SearchFailed {
            query_id: 9,
            failure: SearchProviderFailure::SnapshotExpired,
        }
    ));

    drop(writer_db);
    server.abort();
    let _ = server.await;
}
