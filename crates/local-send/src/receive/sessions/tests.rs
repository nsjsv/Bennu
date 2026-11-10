//! 会话层的单元测试：写盘失败与中途断开（网络层测不出的路径）。

use super::*;

const SESSION: &str = "session";
const FILE: &str = "file";
const OWNER: &str = "SENDER";

/// 开好一次只有一个文件的会话，返回它的上传参数。
async fn one_file_session(
    download_dir: PathBuf,
    file_name: &str,
    size: u64,
) -> (
    ReceiveSessions,
    UploadQuery,
    broadcast::Receiver<ServiceEvent>,
) {
    let (events, events_rx) = broadcast::channel(16);
    let sessions = ReceiveSessions::new(download_dir, SessionEvents::Service(events));
    let files = HashMap::from([(
        FILE.to_owned(),
        FileMetadata {
            id: FILE.into(),
            file_name: file_name.into(),
            size,
            file_type: None,
            sha256: None,
            preview: None,
        },
    )]);
    let response = sessions.open(SESSION, OWNER, &files).await;
    let query = UploadQuery {
        session_id: SESSION.into(),
        file_id: FILE.into(),
        token: response.files[FILE].clone(),
    };
    (sessions, query, events_rx)
}

/// /dev/full 能打开，写入时报 ENOSPC：等同于写到一半磁盘满了。
#[tokio::test]
async fn a_file_that_cannot_reach_the_disk_fails_instead_of_reporting_success() {
    let full = PathBuf::from("/dev/full");
    if !full.exists() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (sessions, query, mut events) =
        one_file_session(directory.path().to_path_buf(), "note.txt", 5).await;
    sessions
        .sessions
        .lock()
        .await
        .get_mut(SESSION)
        .unwrap()
        .files
        .get_mut(FILE)
        .unwrap()
        .dest = Some(full);

    let (status, _) = sessions.upload(OWNER, &query, Body::from("hello")).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let ServiceEvent::ReceiveFinished { report, .. } = events.recv().await.unwrap() else {
        panic!("the session should finish with the failed file");
    };
    assert!(report.saved_paths().is_empty());
    let expected = vec![("note.txt".to_owned(), "write failed".to_owned())];
    assert_eq!(report.failures(), expected);
}

/// 第一次上传中途断了：落盘路径已经登记，重传写回同一个文件，不会另起名字留下半成品。
#[tokio::test]
async fn an_interrupted_upload_is_retried_into_the_same_file() {
    let directory = tempfile::tempdir().unwrap();
    let (sessions, query, _events) =
        one_file_session(directory.path().to_path_buf(), "note.txt", 5).await;
    let broken_body = Body::from_stream(futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"he")),
        Err(std::io::Error::other("connection dropped")),
    ]));

    let (broken, _) = sessions.upload(OWNER, &query, broken_body).await;
    let (retried, _) = sessions.upload(OWNER, &query, Body::from("hello")).await;

    assert_eq!(broken, StatusCode::BAD_REQUEST);
    assert_eq!(retried, StatusCode::OK);
    assert_eq!(
        std::fs::read(directory.path().join("note.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn only_the_sender_that_opened_a_session_can_upload_or_cancel_it() {
    let directory = tempfile::tempdir().unwrap();
    let (sessions, query, _events) =
        one_file_session(directory.path().to_path_buf(), "note.txt", 5).await;

    let (status, _) = sessions
        .upload("SOMEONE ELSE", &query, Body::from("hello"))
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        sessions.cancel_from("SOMEONE ELSE", SESSION).await,
        StatusCode::FORBIDDEN
    );
    assert!(!sessions.is_empty().await);
    assert_eq!(sessions.cancel_from(OWNER, SESSION).await, StatusCode::OK);
    assert!(sessions.is_empty().await);
}
