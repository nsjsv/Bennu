//! receive 处理函数的单元测试（保持宿主文件 ≤800 行）。

use axum::response::IntoResponse;

use super::*;

const SESSION: &str = "session";
const FILE: &str = "file";
const TOKEN: &str = "token";
const SENDER: &str = "SENDER";

fn receive_state(
    download_dir: PathBuf,
    file_name: &str,
    size: u64,
) -> (Arc<ReceiveState>, broadcast::Receiver<ServiceEvent>) {
    let (events, events_rx) = broadcast::channel(16);
    let state = Arc::new(ReceiveState::new(
        DeviceInfo {
            alias: "TestPC".into(),
            version: "2.0".into(),
            fingerprint: "own-fp".into(),
            device_model: None,
            device_type: None,
            download: None,
            port: 53317,
            protocol: "http".into(),
            address: None,
        },
        download_dir,
        Arc::new(|_| true),
        Arc::new(Mutex::new(crate::discovery::DeviceTable::default())),
        events,
        CONFIRM_TIMEOUT,
    ));
    let slot = FileSlot {
        meta: FileMetadata {
            id: FILE.into(),
            file_name: file_name.into(),
            size,
            file_type: None,
            sha256: None,
        },
        token: TOKEN.into(),
        dest: None,
        received: 0,
        outcome: SlotOutcome::Pending,
    };
    state.sessions.try_lock().unwrap().insert(
        SESSION.into(),
        Session {
            sender: SENDER.into(),
            files: HashMap::from([(FILE.to_owned(), slot)]),
            finished_emitted: false,
        },
    );
    (state, events_rx)
}

async fn upload_status(state: &Arc<ReceiveState>, body: Body) -> StatusCode {
    let query = UploadQuery {
        session_id: SESSION.into(),
        file_id: FILE.into(),
        token: TOKEN.into(),
    };
    let peer = Peer {
        address: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        fingerprint: SENDER.into(),
    };
    upload(
        State(Arc::clone(state)),
        ConnectInfo(peer),
        Query(query),
        body,
    )
    .await
    .into_response()
    .status()
}

/// /dev/full 能打开，写入时报 ENOSPC：等同于写到一半磁盘满了。
#[tokio::test]
async fn a_file_that_cannot_reach_the_disk_fails_instead_of_reporting_success() {
    let full = PathBuf::from("/dev/full");
    if !full.exists() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (state, mut events) = receive_state(directory.path().to_path_buf(), "note.txt", 5);
    state
        .sessions
        .lock()
        .await
        .get_mut(SESSION)
        .unwrap()
        .files
        .get_mut(FILE)
        .unwrap()
        .dest = Some(full);

    assert_eq!(
        upload_status(&state, Body::from("hello")).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let ServiceEvent::ReceiveFinished {
        saved_paths,
        failed,
        ..
    } = events.recv().await.unwrap()
    else {
        panic!("the session should finish with the failed file");
    };
    assert!(saved_paths.is_empty());
    let expected = vec![("note.txt".to_owned(), "write failed".to_owned())];
    assert_eq!(failed, expected);
}

/// 第一次上传中途断了：落盘路径已经登记，重传写回同一个文件，不会另起名字留下半成品。
#[tokio::test]
async fn an_interrupted_upload_is_retried_into_the_same_file() {
    let directory = tempfile::tempdir().unwrap();
    let (state, _events) = receive_state(directory.path().to_path_buf(), "note.txt", 5);
    let broken_body = Body::from_stream(futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"he")),
        Err(std::io::Error::other("connection dropped")),
    ]));

    assert_eq!(
        upload_status(&state, broken_body).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        upload_status(&state, Body::from("hello")).await,
        StatusCode::OK
    );
    assert_eq!(
        std::fs::read(directory.path().join("note.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
