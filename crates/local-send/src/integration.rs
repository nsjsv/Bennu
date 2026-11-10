//! 集成测试：对真实 HTTPS 接收端跑 A3/A4/A5 验收路径，发送方带自己的设备证书。
//! 全部封在 127.0.0.1 随机端口与临时目录内，不触碰真实组播网络。

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use std::time::Duration;

use tokio::sync::{broadcast, watch, Mutex};

use crate::device::{DeviceInfo, FileMetadata, PrepareUploadRequest, PrepareUploadResponse};
use crate::discovery::DeviceTable;
use crate::identity::DeviceIdentity;
use crate::receive::{port_already_released, spawn_server, PortReleased, ReceiveState};
use crate::{AcceptPolicy, Favorite, ReceiverConfig, ServiceEvent};

mod cancel;
mod find_device;
mod link;
mod mtls;
mod pin;
mod qr;
mod send;
mod text;

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn remote_device(fingerprint: &str, alias: &str) -> DeviceInfo {
    DeviceInfo {
        alias: alias.into(),
        version: "2.0".into(),
        fingerprint: fingerprint.into(),
        device_model: None,
        device_type: None,
        download: None,
        port: crate::DEFAULT_PORT,
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
        preview: None,
    }
}

fn files_map(entries: Vec<(&str, FileMetadata)>) -> HashMap<String, FileMetadata> {
    entries
        .into_iter()
        .map(|(id, meta)| (id.to_string(), meta))
        .collect()
}

/// 一台带新设备身份的对端：服务唯一的出站客户端与它的证书指纹（接收端按它认人）。
pub(crate) struct TestPeer {
    pub(crate) client: reqwest::Client,
    pub(crate) fingerprint: String,
}

pub(crate) fn test_peer() -> TestPeer {
    let dir = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap();
    TestPeer {
        client: crate::outbound_tls::OutboundTls::new(&identity)
            .unwrap()
            .client(None)
            .unwrap(),
        fingerprint: identity.fingerprint().to_owned(),
    }
}

/// 真实接收端（TLS + 强制客户端证书）与一台发送方。
pub(crate) struct ReceiveServer {
    pub(crate) base: String,
    pub(crate) port: u16,
    pub(crate) state: Arc<ReceiveState>,
    pub(crate) events: broadcast::Receiver<ServiceEvent>,
    /// 置位即优雅关停；drop 同样会关停，测试期间必须持有。
    pub(crate) shutdown: watch::Sender<bool>,
    pub(crate) released: PortReleased,
    /// 接收端自己的证书指纹（直推时对端声明的 fingerprint）。
    pub(crate) fingerprint: String,
    /// 收藏快照：app-ui 改收藏时做的就是换掉它。
    pub(crate) favorites: watch::Sender<Vec<Favorite>>,
    pub(crate) sender: TestPeer,
    /// 下载目录，随句柄一起释放：服务任务持有的 download_dir 在测试期间始终有效。
    pub(crate) _dir: tempfile::TempDir,
}

impl ReceiveServer {
    /// 让这台设备成为收藏（自动收）。
    pub(crate) fn favor(&self, fingerprint: &str) {
        self.favorites.send_replace(vec![favorite(fingerprint)]);
    }

    /// 直推目标：本接收端。
    fn target(&self) -> DeviceInfo {
        DeviceInfo {
            address: Some("127.0.0.1".parse().unwrap()),
            port: self.port,
            protocol: "https".into(),
            ..remote_device(&self.fingerprint, "Receiver")
        }
    }
}

pub(crate) fn favorite(fingerprint: &str) -> Favorite {
    Favorite {
        fingerprint: fingerprint.to_owned(),
        address: None,
    }
}

pub(crate) fn receiver_config(
    download_dir: &std::path::Path,
    accept: AcceptPolicy,
    pin: Option<&str>,
) -> ReceiverConfig {
    ReceiverConfig {
        download_dir: download_dir.to_path_buf(),
        accept,
        pin: pin.map(str::to_owned),
    }
}

/// 启动接收端但不等它就绪（端口可能被占）；没有收藏设备、收藏自动收、不要 PIN。
pub(crate) fn start_receiver(
    alias: &str,
    port: u16,
    previous_released: PortReleased,
    confirm_timeout: Duration,
) -> ReceiveServer {
    start_receiver_with(
        alias,
        port,
        previous_released,
        confirm_timeout,
        AcceptPolicy::AcceptFavorites,
        None,
    )
}

pub(crate) fn start_receiver_with(
    alias: &str,
    port: u16,
    previous_released: PortReleased,
    confirm_timeout: Duration,
    accept: AcceptPolicy,
    pin: Option<&str>,
) -> ReceiveServer {
    let dir = tempfile::tempdir().unwrap();
    let identity_dir = tempfile::tempdir().unwrap();
    let identity =
        DeviceIdentity::load_or_create(&identity_dir.path().join("identity.pem")).unwrap();
    let (events_tx, events) = broadcast::channel(128);
    let (favorites, favorites_rx) = watch::channel(Vec::new());
    let state = Arc::new(ReceiveState::new(
        crate::own_device_info(alias.to_owned(), None, port, &identity),
        receiver_config(dir.path(), accept, pin),
        favorites_rx,
        Arc::new(Mutex::new(DeviceTable::new(
            identity.fingerprint().to_owned(),
        ))),
        events_tx,
        confirm_timeout,
    ));
    let (shutdown, shutdown_rx) = watch::channel(false);
    let released = spawn_server(
        Arc::clone(&state),
        port,
        crate::receive::server_config(&identity).unwrap(),
        previous_released,
        shutdown_rx,
    );
    ReceiveServer {
        base: format!("https://127.0.0.1:{port}"),
        port,
        state,
        events,
        shutdown,
        released,
        fingerprint: identity.fingerprint().to_owned(),
        favorites,
        sender: test_peer(),
        _dir: dir,
    }
}

pub(crate) async fn free_port() -> u16 {
    let probe = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    probe.local_addr().unwrap().port()
}

/// 端口上回应 info 的是名为 `alias` 的接收端，才算它在服务。
pub(crate) async fn wait_until_serving(client: &reqwest::Client, port: u16, alias: &str) {
    let url = format!("https://127.0.0.1:{port}/api/localsend/v2/info");
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Ok(response) = client.get(&url).send().await {
                if let Ok(device) = response.json::<DeviceInfo>().await {
                    if device.alias == alias {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("receiver never started serving");
}

/// `sender_is_favorite`：测试自带的发送方（`server.sender`）是不是收藏设备。
async fn spawn_receive(sender_is_favorite: bool, confirm_timeout: Duration) -> ReceiveServer {
    let server = start_receiver(
        "TestPC",
        free_port().await,
        port_already_released(),
        confirm_timeout,
    );
    if sender_is_favorite {
        server.favor(&server.sender.fingerprint.clone());
    }
    wait_until_serving(&server.sender.client, server.port, "TestPC").await;
    server
}

/// 二维码下载服务是明文 HTTP，用普通客户端。
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .unwrap()
}

async fn prepare(
    server: &ReceiveServer,
    files: HashMap<String, FileMetadata>,
) -> reqwest::Response {
    prepare_with(server.sender.client.clone(), server.base.clone(), files).await
}

/// 发送方自报一个与证书无关的 fingerprint：接收端必须按证书认人。拿所有权，
/// 便于在挂起等确认的用例里 spawn。
async fn prepare_with(
    client: reqwest::Client,
    base: String,
    files: HashMap<String, FileMetadata>,
) -> reqwest::Response {
    client
        .post(format!("{base}/api/localsend/v2/prepare-upload"))
        .json(&PrepareUploadRequest {
            info: remote_device("sender-fp", "Sender"),
            files,
        })
        .send()
        .await
        .unwrap()
}

fn post(server: &ReceiveServer, path: &str) -> reqwest::RequestBuilder {
    server.sender.client.post(format!("{}{path}", server.base))
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
    // 自报与证书一致才登记；大小写不论（官方转大写后比较）。
    let claimed = server.sender.fingerprint.to_ascii_lowercase();
    let response = post(&server, "/api/localsend/v2/register")
        .json(&remote_device(&claimed, "Phone"))
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
    assert_eq!(device.fingerprint, server.sender.fingerprint);
    assert_eq!(device.address, Some("127.0.0.1".parse().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_prepare_upload_returns_session_and_tokens() {
    let server = spawn_receive(true, Duration::from_secs(10)).await;
    let response = prepare(
        &server,
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
async fn a_request_from_a_non_favorite_waits_for_confirmation() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let task = tokio::spawn(prepare_with(
        server.sender.client.clone(),
        server.base.clone(),
        files_map(vec![("0", meta("0", "a.txt", 5))]),
    ));

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
    // 自报的 "sender-fp" 不作数：确认框里的设备（以及加进收藏的身份）是证书指纹；
    // 地址是连接来源，加进收藏时要记住。
    assert_eq!(device.fingerprint, server.sender.fingerprint);
    assert_eq!(device.address, Some("127.0.0.1".parse().unwrap()));
    assert_eq!(files.len(), 1);

    assert!(server.state.resolve_pending(&session_id, true).await);
    let response = task.await.unwrap();
    assert_eq!(response.status(), 201);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_favorites_takes_effect_on_the_next_request() {
    let mut server = spawn_receive(false, Duration::from_millis(300)).await;
    let single = || files_map(vec![("0", meta("0", "a.txt", 5))]);

    // 加进收藏（接收确认里勾"加入收藏"、点 ☆）：下一个请求直接收，不用重启。
    server.favor(&server.sender.fingerprint.clone());
    assert_eq!(prepare(&server, single()).await.status(), 201);

    // 取消收藏：又要确认（这里没人确认，超时 403）。
    server.favorites.send_replace(Vec::new());
    assert_eq!(prepare(&server, single()).await.status(), 403);
    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::IncomingRequest { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_prepare_upload_rejects_and_stale_resolve_fails() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let task = tokio::spawn(prepare_with(
        server.sender.client.clone(),
        server.base.clone(),
        files_map(vec![("0", meta("0", "a.txt", 5))]),
    ));
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::IncomingRequest { .. })
    })
    .await;
    let ServiceEvent::IncomingRequest { session_id, .. } = event else {
        unreachable!()
    };
    assert!(server.state.resolve_pending(&session_id, false).await);
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
    assert!(!server.state.resolve_pending(&session_id, true).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_prepare_upload_times_out_with_403() {
    let mut server = spawn_receive(false, Duration::from_millis(200)).await;
    let response = prepare(&server, files_map(vec![("0", meta("0", "a.txt", 5))])).await;
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
    let response = prepare(&server, files_map(files)).await;
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
        server,
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
    let ServiceEvent::ReceiveFinished { session_id, report } = event else {
        unreachable!()
    };
    let (saved_paths, failed) = (report.saved_paths(), report.failures());
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
async fn a_checksum_mismatch_may_be_resent_until_the_third_attempt() {
    let mut checksum_meta = meta("0", "sum.txt", 5);
    checksum_meta.sha256 = Some(sha256_hex(b"hello"));
    let (mut server, session) = run_trusted_session(vec![("0", checksum_meta)]).await;

    // 前两次不对：422，文件退回待收，会话没结束。
    for _ in 0..2 {
        let response = upload_text(&server, &session, "0", "hellx").await;
        assert_eq!(response.status(), 422);
    }
    assert!(!matches!(
        server.events.try_recv(),
        Ok(ServiceEvent::ReceiveFinished { .. })
    ));
    // 第三次还不对：判失败、删半成品。
    let response = upload_text(&server, &session, "0", "hellx").await;
    assert_eq!(response.status(), 422);
    let event = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    let ServiceEvent::ReceiveFinished { report, .. } = event else {
        unreachable!()
    };
    let (saved_paths, failed) = (report.saved_paths(), report.failures());
    assert!(saved_paths.is_empty());
    assert_eq!(
        failed,
        [("sum.txt".to_owned(), "checksum mismatch".to_owned())]
    );
    assert!(!server._dir.path().join("sum.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resent_file_lands_in_the_same_place_after_a_mismatch() {
    let mut checksum_meta = meta("0", "sum.txt", 5);
    checksum_meta.sha256 = Some(sha256_hex(b"hello"));
    let (server, session) = run_trusted_session(vec![("0", checksum_meta)]).await;

    assert_eq!(
        upload_text(&server, &session, "0", "hellx").await.status(),
        422
    );
    assert_eq!(
        upload_text(&server, &session, "0", "hello").await.status(),
        200
    );

    assert_eq!(
        std::fs::read(server._dir.path().join("sum.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(std::fs::read_dir(server._dir.path()).unwrap().count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_arrives_with_its_structure_beside_an_existing_one() {
    let (mut server, session) = run_trusted_session(vec![
        ("0", meta("0", "照片/a.txt", 1)),
        ("1", meta("1", "照片/2024/b.txt", 1)),
    ])
    .await;
    std::fs::create_dir(server._dir.path().join("照片")).unwrap();

    assert_eq!(upload_text(&server, &session, "0", "a").await.status(), 200);
    assert_eq!(upload_text(&server, &session, "1", "b").await.status(), 200);

    let renamed = server._dir.path().join("照片 (1)");
    assert_eq!(std::fs::read(renamed.join("a.txt")).unwrap(), b"a");
    assert_eq!(
        std::fs::read(renamed.join("2024").join("b.txt")).unwrap(),
        b"b"
    );
    wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_name_climbing_out_of_the_receive_directory_is_refused() {
    let (server, session) = run_trusted_session(vec![("0", meta("0", "../evil.txt", 1))]).await;

    assert_eq!(upload_text(&server, &session, "0", "x").await.status(), 400);

    assert!(!server
        ._dir
        .path()
        .parent()
        .unwrap()
        .join("evil.txt")
        .exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_rejects_wrong_token_and_unknown_session() {
    let (server, session) = run_trusted_session(vec![("0", meta("0", "a.txt", 5))]).await;
    let bad_token = post(
        &server,
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
        &server,
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

    // 先发 3 字节、之后既不发完也不断开：半成品确定落盘、槽位保持 pending。直接断开
    // 不可靠——对端 RST 时服务端可能还没读走缓冲里的数据。
    let stalled_body = futures_util::stream::iter([Ok::<_, std::io::Error>(
        axum::body::Bytes::from_static(b"abc"),
    )])
    .chain(futures_util::stream::pending());
    let stalled_upload = tokio::spawn(
        post(
            &server,
            &format!(
                "/api/localsend/v2/upload?sessionId={}&fileId=1&token={}",
                session.session_id, session.files["1"],
            ),
        )
        .body(reqwest::Body::wrap_stream(stalled_body))
        .send(),
    );
    let partial = server._dir.path().join("partial.bin");
    // 发送与服务端写盘是异步的：轮询等半成品出现。
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
    stalled_upload.abort();

    assert_eq!(
        post(
            &server,
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
    let ServiceEvent::ReceiveFinished { report, .. } = event else {
        unreachable!()
    };
    assert_eq!(report.saved_paths().len(), 1);
    assert!(report.failures().is_empty());
    assert!(report
        .files
        .iter()
        .any(|file| file.file_name == "partial.bin"
            && file.outcome == crate::ReceivedOutcome::Canceled));
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

pub(crate) async fn prepared(items: Vec<std::path::PathBuf>) -> crate::PreparedFiles {
    crate::prepare_files(
        items,
        tokio::sync::mpsc::channel(1).0,
        crate::CancellationToken::new(),
    )
    .await
    .unwrap()
}
