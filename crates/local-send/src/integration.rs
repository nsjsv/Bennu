//! 集成测试：对真实 HTTP 服务跑 A3/A4/A5 验收路径。
//! 全部封在 127.0.0.1 随机端口与临时目录内，不触碰真实组播网络。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::{broadcast, Mutex};

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::discovery::DeviceTable;
use crate::receive::{router as receive_router, ReceiveState};
use crate::{QrDownloadServer, QrEvent, QrItem, QrSource, ServiceEvent};

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn own_device() -> DeviceInfo {
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
    }
}

fn remote_device(fingerprint: &str, alias: &str) -> DeviceInfo {
    DeviceInfo {
        alias: alias.into(),
        version: "2.0".into(),
        fingerprint: fingerprint.into(),
        device_model: None,
        device_type: None,
        download: None,
        port: 53317,
        protocol: "http".into(),
        address: None,
    }
}

fn meta(id: &str, name: &str, size: u64) -> FileMetadata {
    FileMetadata {
        id: id.into(),
        file_name: name.into(),
        size,
        file_type: Some("text/plain".into()),
        sha256: None,
    }
}

fn files_map(entries: Vec<(&str, FileMetadata)>) -> HashMap<String, FileMetadata> {
    entries
        .into_iter()
        .map(|(id, meta)| (id.to_string(), meta))
        .collect()
}

struct ReceiveServer {
    base: String,
    state: std::sync::Arc<ReceiveState>,
    events: broadcast::Receiver<ServiceEvent>,
    // 目录随句柄一起释放，服务任务持有的 download_dir 在测试期间始终有效。
    _dir: tempfile::TempDir,
}

async fn spawn_receive(trusted: bool, confirm_timeout: Duration) -> ReceiveServer {
    let dir = tempfile::tempdir().unwrap();
    let (events_tx, events_rx) = broadcast::channel(128);
    let state = std::sync::Arc::new(ReceiveState::new(
        own_device(),
        dir.path().to_path_buf(),
        std::sync::Arc::new(move |_| trusted),
        std::sync::Arc::new(Mutex::new(DeviceTable::default())),
        events_tx,
        confirm_timeout,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = receive_router(std::sync::Arc::clone(&state));
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    ReceiveServer {
        base: format!("http://127.0.0.1:{port}"),
        state,
        events: events_rx,
        _dir: dir,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

async fn prepare(base: &str, files: HashMap<String, FileMetadata>) -> reqwest::Response {
    client()
        .post(format!("{base}/api/localsend/v2/prepare-upload"))
        .json(&PrepareUploadRequest {
            info: remote_device("sender-fp", "Sender"),
            files,
        })
        .send()
        .await
        .unwrap()
}

fn post(base: &str, path: &str) -> reqwest::RequestBuilder {
    client().post(format!("{base}{path}"))
}

async fn next_event<T: Clone>(events: &mut broadcast::Receiver<T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, events.recv())
        .await
        .unwrap()
        .unwrap()
}

/// 等到匹配的事件；中间事件（如 prepare 的 RequestAccepted）直接跳过。
async fn wait_for<E: Clone>(events: &mut broadcast::Receiver<E>, want: fn(&E) -> bool) -> E {
    loop {
        let event = next_event(events).await;
        if want(&event) {
            return event;
        }
    }
}

// —— A3：receive 端点 ——

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_replies_with_own_device_and_updates_table() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let response = post(&server.base, "/api/localsend/v2/register")
        .json(&remote_device("phone-fp", "Phone"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let device: DeviceInfo = response.json().await.unwrap();
    assert_eq!(device.alias, "TestPC");
    // register 是双向发现的另一半：来源设备必须进设备表并广播 DeviceSeen。
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::DeviceSeen(_))
    })
    .await;
    let ServiceEvent::DeviceSeen(device) = event else {
        unreachable!()
    };
    assert_eq!(device.fingerprint, "phone-fp");
    assert_eq!(device.address, Some("127.0.0.1".parse().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_prepare_upload_returns_session_and_tokens() {
    let server = spawn_receive(true, Duration::from_secs(10)).await;
    let response = prepare(
        &server.base,
        files_map(vec![
            ("0", meta("0", "a.txt", 5)),
            ("1", meta("1", "b.txt", 6)),
        ]),
    )
    .await;
    assert_eq!(response.status(), 201);
    let payload: PrepareUploadResponse = response.json().await.unwrap();
    assert!(!payload.session_id.is_empty());
    assert_eq!(payload.files.len(), 2);
    assert!(payload.files.values().all(|token| token.len() == 32));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_prepare_upload_pends_then_accepts_and_remembers() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let base = server.base.clone();
    let task =
        tokio::spawn(
            async move { prepare(&base, files_map(vec![("0", meta("0", "a.txt", 5))])).await },
        );

    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::IncomingRequest { .. })
    })
    .await;
    let ServiceEvent::IncomingRequest {
        device,
        session_id,
        files,
    } = event
    else {
        unreachable!()
    };
    assert_eq!(device.fingerprint, "sender-fp");
    assert_eq!(files.len(), 1);

    // remember=true：本次放行，且后续同设备请求不再挂起。
    assert!(server.state.resolve_pending(&session_id, true, true).await);
    let response = task.await.unwrap();
    assert_eq!(response.status(), 201);

    let second = prepare(
        &server.base,
        files_map(vec![("0", meta("0", "again.txt", 2))]),
    )
    .await;
    assert_eq!(second.status(), 201);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_prepare_upload_rejects_and_stale_resolve_fails() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let base = server.base.clone();
    let task =
        tokio::spawn(
            async move { prepare(&base, files_map(vec![("0", meta("0", "a.txt", 5))])).await },
        );
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::IncomingRequest { .. })
    })
    .await;
    let ServiceEvent::IncomingRequest { session_id, .. } = event else {
        unreachable!()
    };
    assert!(
        server
            .state
            .resolve_pending(&session_id, false, false)
            .await
    );
    assert_eq!(task.await.unwrap().status(), 403);
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::RequestRejected { .. })
    })
    .await;
    let ServiceEvent::RequestRejected {
        session_id: rejected,
    } = event
    else {
        unreachable!()
    };
    assert_eq!(rejected, session_id);
    // 决策已消费：重复回写必须返回 false，UI 据此感知决策落空。
    assert!(!server.state.resolve_pending(&session_id, true, false).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_prepare_upload_times_out_with_403() {
    let mut server = spawn_receive(false, Duration::from_millis(200)).await;
    let response = prepare(&server.base, files_map(vec![("0", meta("0", "a.txt", 5))])).await;
    assert_eq!(response.status(), 403);
    wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::RequestRejected { .. })
    })
    .await;
}

/// 走完整 prepare → upload 流程，返回 (server, session, tokens)。
async fn run_trusted_session(
    files: Vec<(&str, FileMetadata)>,
) -> (ReceiveServer, PrepareUploadResponse) {
    let server = spawn_receive(true, Duration::from_secs(10)).await;
    let response = prepare(&server.base, files_map(files)).await;
    assert_eq!(response.status(), 201);
    let payload: PrepareUploadResponse = response.json().await.unwrap();
    (server, payload)
}

async fn upload_text(
    server: &ReceiveServer,
    session: &PrepareUploadResponse,
    file_id: &str,
    body: &str,
) -> reqwest::Response {
    post(
        &server.base,
        &format!(
            "/api/localsend/v2/upload?sessionId={}&fileId={file_id}&token={}",
            session.session_id, session.files[file_id],
        ),
    )
    .body(body.to_string())
    .send()
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_writes_files_and_emits_finished() {
    let (mut server, session) = run_trusted_session(vec![
        ("0", meta("0", "a.txt", 5)),
        ("1", meta("1", "b.txt", 5)),
    ])
    .await;

    assert_eq!(
        upload_text(&server, &session, "0", "hello").await.status(),
        200
    );
    assert_eq!(
        upload_text(&server, &session, "1", "world").await.status(),
        200
    );

    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    let ServiceEvent::ReceiveFinished {
        session_id,
        saved_paths,
        failed,
    } = event
    else {
        unreachable!()
    };
    assert_eq!(session_id, session.session_id);
    assert_eq!(saved_paths.len(), 2);
    assert!(failed.is_empty());
    let names: Vec<String> = saved_paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"a.txt".to_string()));
    assert!(names.contains(&"b.txt".to_string()));
    let a = std::fs::read(server._dir.path().join("a.txt")).unwrap();
    assert_eq!(a, b"hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_dedups_existing_names_with_suffix() {
    let (server, session) = run_trusted_session(vec![("0", meta("0", "a.txt", 3))]).await;
    std::fs::write(server._dir.path().join("a.txt"), b"old").unwrap();
    assert_eq!(
        upload_text(&server, &session, "0", "new").await.status(),
        200
    );
    // 重名规则：写进 "a (1).txt"，不覆盖既有文件（R8）。
    assert_eq!(
        std::fs::read(server._dir.path().join("a.txt")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(server._dir.path().join("a (1).txt")).unwrap(),
        b"new"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_sha256_mismatch_returns_422_and_cleans_up() {
    let mut checksum_meta = meta("0", "sum.txt", 5);
    checksum_meta.sha256 = Some(sha256_hex(b"hello"));
    let (mut server, session) = run_trusted_session(vec![("0", checksum_meta)]).await;
    let response = upload_text(&server, &session, "0", "hellx").await;
    assert_eq!(response.status(), 422);
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    let ServiceEvent::ReceiveFinished {
        saved_paths,
        failed,
        ..
    } = event
    else {
        unreachable!()
    };
    assert!(saved_paths.is_empty());
    assert_eq!(failed[0].0, "sum.txt");
    assert_eq!(failed[0].1, "checksum mismatch");
    // 校验失败的半成品必须删除。
    assert!(!server._dir.path().join("sum.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_rejects_wrong_token_and_unknown_session() {
    let (server, session) = run_trusted_session(vec![("0", meta("0", "a.txt", 5))]).await;
    let bad_token = post(
        &server.base,
        &format!(
            "/api/localsend/v2/upload?sessionId={}&fileId=0&token=deadbeef",
            session.session_id,
        ),
    )
    .body("hello")
    .send()
    .await
    .unwrap();
    assert_eq!(bad_token.status(), 403);
    let unknown = post(
        &server.base,
        &format!(
            "/api/localsend/v2/upload?sessionId=nope&fileId=0&token={}",
            session.files["0"],
        ),
    )
    .body("hello")
    .send()
    .await
    .unwrap();
    assert_eq!(unknown.status(), 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_deletes_partials_and_keeps_done_files() {
    let (mut server, session) = run_trusted_session(vec![
        ("0", meta("0", "done.txt", 5)),
        ("1", meta("1", "partial.bin", 100)),
    ])
    .await;
    assert_eq!(
        upload_text(&server, &session, "0", "hello").await.status(),
        200
    );

    // 用裸 TCP 写 chunked 请求头 + 一个数据块后直接断连：确定性地留下 3 字节半成品，
    // 槽位保持 pending（reqwest 客户端 abort 时 hyper 可能在服务端处理前就重置连接，不可靠）。
    let port: u16 = server.base.rsplit(':').next().unwrap().parse().unwrap();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    use tokio::io::AsyncWriteExt;
    let head = format!(
        "POST /api/localsend/v2/upload?sessionId={}&fileId=1&token={} HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n",
        session.session_id,
        session.files["1"],
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(b"3\r\nabc\r\n").await.unwrap();
    stream.flush().await.unwrap();
    drop(stream); // 不发终止块直接断开 → 服务端 body 读取错误，槽位留在 pending。
    let partial = server._dir.path().join("partial.bin");
    // 客户端 abort 与服务端写盘是异步的：轮询等半成品出现。
    let mut appeared = false;
    for _ in 0..50 {
        if partial.exists() {
            appeared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(appeared, "partial file never appeared");
    assert_eq!(std::fs::read(&partial).unwrap(), b"abc");

    assert_eq!(
        post(
            &server.base,
            &format!("/api/localsend/v2/cancel?sessionId={}", session.session_id)
        )
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    let ServiceEvent::ReceiveFinished {
        saved_paths,
        failed,
        ..
    } = event
    else {
        unreachable!()
    };
    assert_eq!(saved_paths.len(), 1);
    assert!(failed
        .iter()
        .any(|(name, reason)| name == "partial.bin" && reason == "canceled by sender"));
    // cancel 清理语义：半成品删除，已完成文件保留。
    assert!(!partial.exists());
    assert_eq!(
        std::fs::read(server._dir.path().join("done.txt")).unwrap(),
        b"hello"
    );
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

// —— A4：send 端到端 ——

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_to_device_transfers_files_and_directory_zip() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;

    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), b"first").unwrap();
    let dir = source.path().join("bundle");
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("inner").join("deep.txt"), b"deep").unwrap();
    std::fs::write(dir.join("top.txt"), b"top").unwrap();

    let target = DeviceInfo {
        address: Some("127.0.0.1".parse().unwrap()),
        port: server.base.rsplit(':').next().unwrap().parse().unwrap(),
        ..remote_device("phone-fp", "Phone")
    };
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
    let items = vec![source.path().join("one.txt"), dir.clone()];
    crate::send_to_device_with_progress(&target, items, "TestPC", progress_tx)
        .await
        .unwrap();

    // 接收端：一个普通文件 + 一个目录 zip。
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    let ServiceEvent::ReceiveFinished {
        saved_paths,
        failed,
        ..
    } = event
    else {
        unreachable!()
    };
    assert!(failed.is_empty(), "failed: {failed:?}");
    assert_eq!(saved_paths.len(), 2);
    assert_eq!(
        std::fs::read(server._dir.path().join("one.txt")).unwrap(),
        b"first"
    );
    let zip_bytes = std::fs::read(server._dir.path().join("bundle.zip")).unwrap();
    let reader =
        async_zip::tokio::read::seek::ZipFileReader::with_tokio(std::io::Cursor::new(zip_bytes))
            .await
            .unwrap();
    let mut names = Vec::new();
    for index in 0..reader.file().entries().len() {
        names.push(
            reader.file().entries()[index]
                .filename()
                .clone()
                .into_string()
                .unwrap(),
        );
    }
    assert!(names.contains(&"bundle/top.txt".to_string()));
    assert!(names.contains(&"bundle/inner/deep.txt".to_string()));

    // 进度通道收到过事件（通道容量有限，只验证存在性）。
    assert!(progress_rx.recv().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_to_device_requires_discovered_address() {
    let target = remote_device("phone-fp", "Phone");
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"x").unwrap();
    let result = crate::send_to_device(&target, vec![file], "TestPC").await;
    assert!(matches!(result, Err(crate::LocalSendError::AddressUnknown)));
}

// —— A5：二维码下载服务 ——

fn token_of(url: &str) -> String {
    url.rsplit('/').next().unwrap().to_string()
}

fn port_of(url: &str) -> u16 {
    let host_port = url.trim_start_matches("http://").split('/').next().unwrap();
    host_port.rsplit(':').next().unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_serves_index_files_and_zip_then_finishes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), b"AAA").unwrap();
    let nested = dir.path().join("docs");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("readme.md"), b"hello zip").unwrap();

    let handle = QrDownloadServer::start_with_idle_timeout(
        vec![
            QrItem {
                display_name: "alpha.txt".into(),
                source: QrSource::File(dir.path().join("alpha.txt")),
            },
            QrItem {
                display_name: "docs".into(),
                source: QrSource::Dir(nested.clone()),
            },
        ],
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    // 测试环境必须存在局域网候选地址（与此前 local_ip 路径的隐含前提一致）。
    let primary_url = handle
        .urls()
        .first()
        .expect("qr urls must contain at least one candidate address")
        .clone();
    let token = token_of(&primary_url);
    let port = port_of(&primary_url);
    let base = format!("http://127.0.0.1:{port}");
    let mut events = handle.events();

    // token 是唯一鉴权：错误 token 一律 404。
    assert_eq!(
        client()
            .get(format!("{base}/wrong-token"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // 列表页含条目与 zip 链接；首个请求产生 Connected。
    let index = client()
        .get(format!("{base}/{token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    let page = index.text().await.unwrap();
    assert!(page.contains("alpha.txt"));
    assert!(page.contains("/zip"));
    wait_for(&mut events, |e| matches!(e, QrEvent::Connected)).await;

    // 单文件流：内容一致。
    let file = client()
        .get(format!("{base}/{token}/file/0"))
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), 200);
    assert_eq!(file.bytes().await.unwrap().as_ref(), b"AAA");
    // 不存在的条目 404；目录条目没有单文件下载。
    assert_eq!(
        client()
            .get(format!("{base}/{token}/file/9"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client()
            .get(format!("{base}/{token}/file/1"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // zip：async_zip 读回校验条目与内容。
    let zip = client()
        .get(format!("{base}/{token}/zip"))
        .send()
        .await
        .unwrap();
    assert_eq!(zip.status(), 200);
    let zip_bytes = zip.bytes().await.unwrap().to_vec();
    let mut reader =
        async_zip::tokio::read::seek::ZipFileReader::with_tokio(std::io::Cursor::new(zip_bytes))
            .await
            .unwrap();
    // zip 只写文件条目（目录由路径隐含）：alpha.txt + docs/readme.md。
    assert_eq!(reader.file().entries().len(), 2);
    let mut contents = Vec::new();
    for index in 0..reader.file().entries().len() {
        // async_zip 的 entry reader 实现 futures AsyncRead，需 compat 包装成 tokio 侧。
        use tokio_util::compat::FuturesAsyncReadCompatExt;
        let mut entry_reader = reader.reader_with_entry(index).await.unwrap().compat();
        let mut buffer = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut entry_reader, &mut buffer)
            .await
            .unwrap();
        contents.push((
            reader.file().entries()[index]
                .filename()
                .clone()
                .into_string()
                .unwrap(),
            buffer,
        ));
    }
    assert!(contents.contains(&("alpha.txt".to_string(), b"AAA".to_vec())));
    assert!(contents.contains(&("docs/readme.md".to_string(), b"hello zip".to_vec())));

    // 全部条目下载完成 → Finished（监控任务 1s 轮询）。
    wait_for(&mut events, |e| matches!(e, QrEvent::Finished)).await;
    // 传完自动关：端口不再监听（A4 验收口径）。
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_times_out_when_idle_and_shuts_down() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("idle.txt"), b"z").unwrap();
    let handle = QrDownloadServer::start_with_idle_timeout(
        vec![QrItem {
            display_name: "idle.txt".into(),
            source: QrSource::File(dir.path().join("idle.txt")),
        }],
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let port = port_of(handle.urls().first().expect("qr urls non-empty"));
    let mut events = handle.events();
    wait_for(&mut events, |e| matches!(e, QrEvent::TimedOut)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qr_handle_drop_stops_server() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), b"x").unwrap();
    let port = {
        let handle = QrDownloadServer::start_with_idle_timeout(
            vec![QrItem {
                display_name: "x.txt".into(),
                source: QrSource::File(dir.path().join("x.txt")),
            }],
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        port_of(handle.urls().first().expect("qr urls non-empty"))
    };
    // 句柄 drop（R4：关窗即关服务）→ 监听停止。
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}
