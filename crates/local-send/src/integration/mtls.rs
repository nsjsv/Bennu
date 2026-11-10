//! 双向 TLS：真实接收端照新版官方 App（Rust 核心）强制客户端证书、按证书指纹认人；
//! 本机出站请求（直推、register）出示的证书就是自报 fingerprint 对应的那张；本机
//! 认对方也只看它的服务端证书。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use super::{
    files_map, free_port, meta, post, prepare, spawn_receive, test_peer, wait_for, TEST_TIMEOUT,
};
use crate::device::{DeviceInfo, PrepareUploadResponse};
use crate::identity::DeviceIdentity;
use crate::ServiceEvent;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_rejects_clients_without_a_certificate() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let anonymous = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap();

    let outcome = anonymous
        .post(format!("{}/api/localsend/v2/register", server.base))
        .json(&super::remote_device("anonymous", "Anonymous"))
        .send()
        .await;

    assert!(outcome.is_err(), "{outcome:?}");
    // 握手就被拒，请求进不了 HTTP 层。
    assert!(matches!(
        server.events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_never_handshakes_does_not_block_others() {
    let server = spawn_receive(true, Duration::from_secs(10)).await;
    let _stalled = tokio::net::TcpStream::connect(("127.0.0.1", server.port))
        .await
        .unwrap();

    // 新客户端：发送方的客户端里有等就绪时留下的连接，复用它就不用新握手了。
    let info = test_peer()
        .client
        .get(format!("{}/api/localsend/v2/info", server.base))
        .send();

    // 握手超时是 10 秒：5 秒内拿到回应说明没被前一个连接挡住。
    let response = tokio::time::timeout(Duration::from_secs(5), info)
        .await
        .expect("blocked by a stalled handshake")
        .unwrap();
    assert!(response.status().is_success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_with_a_fingerprint_its_certificate_does_not_prove_is_ignored() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;

    let response = post(&server, "/api/localsend/v2/register")
        .json(&super::remote_device("SOMEONE-ELSE", "Impostor"))
        .send()
        .await
        .unwrap();

    // 照常回 200 + 本机信息（同官方），只是不登记。
    assert_eq!(response.status(), 200);
    assert!(matches!(
        server.events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_certificate_cannot_use_a_session_token() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let response = prepare(&server, files_map(vec![("0", meta("0", "a.txt", 5))])).await;
    let session: PrepareUploadResponse = response.json().await.unwrap();
    let upload_path = format!(
        "/api/localsend/v2/upload?sessionId={}&fileId=0&token={}",
        session.session_id, session.files["0"],
    );
    let cancel_path = format!("/api/localsend/v2/cancel?sessionId={}", session.session_id);
    let intruder = test_peer();

    for path in [&upload_path, &cancel_path] {
        let status = intruder
            .client
            .post(format!("{}{path}", server.base))
            .body("hello")
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 403, "{path}");
    }

    // 发起方自己照常能传。
    let status = post(&server, &upload_path)
        .body("hello")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_send_presents_the_identity_its_fingerprint_claims() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"first").unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    // 只收藏发送方的身份：自动收下说明接收端按握手时的证书认出了它。
    server.favor(&fingerprint);
    let (progress, _) = mpsc::channel(1);

    sender
        .send(&server.target(), vec![file], progress)
        .await
        .unwrap();

    let event = wait_for(&mut server.events, |event| {
        matches!(
            event,
            ServiceEvent::IncomingRequest { .. } | ServiceEvent::ReceiveFinished { .. }
        )
    })
    .await;
    assert!(
        matches!(event, ServiceEvent::ReceiveFinished { .. }),
        "{event:?}"
    );
    assert_eq!(
        std::fs::read(server._dir.path().join("one.txt")).unwrap(),
        b"first"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_presents_the_identity_its_fingerprint_claims() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let dir = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap();
    let own = crate::own_device_info("Peer".to_owned(), None, free_port().await, &identity);

    let response = crate::discovery::post_register(
        &crate::outbound_tls::OutboundTls::new(&identity)
            .unwrap()
            .client(None)
            .unwrap(),
        &format!("{}/api/localsend/v2/register", server.base),
        &own,
        TEST_TIMEOUT,
    )
    .await
    .unwrap();

    assert!(response.status().is_success());
    // 接收端只登记自报与证书一致的 register：登记上了就证明出示的是这张证书。
    let ServiceEvent::DeviceSeen(device) = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::DeviceSeen(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(device.fingerprint, identity.fingerprint());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_send_refuses_a_device_whose_certificate_is_not_the_chosen_one() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("secret-name.txt");
    std::fs::write(&file, b"secret").unwrap();
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let (progress, _) = mpsc::channel(1);
    // 选的是另一台设备，这个地址上却是 server：比如对方换了 IP，地址被别的设备占了。
    let chosen = DeviceInfo {
        fingerprint: test_peer().fingerprint,
        ..server.target()
    };

    let error = sender
        .send(&chosen, vec![file], progress)
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not belong to the device you chose"),
        "{error}"
    );
    // 握手就断了：连文件清单都没到接收端。
    assert!(matches!(
        server.events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probing_an_encrypted_device_trusts_its_certificate_not_its_claim() {
    let identity_dir = tempfile::tempdir().unwrap();
    let identity =
        DeviceIdentity::load_or_create(&identity_dir.path().join("identity.pem")).unwrap();
    let port = free_port().await;
    let liar = DeviceInfo {
        fingerprint: "SOMEONE-ELSE".into(),
        ..crate::own_device_info("Liar".to_owned(), None, port, &identity)
    };
    let download = tempfile::tempdir().unwrap();
    let state = Arc::new(crate::receive::ReceiveState::new(
        liar,
        download.path().to_path_buf(),
        tokio::sync::watch::channel(Vec::new()).1,
        Arc::new(tokio::sync::Mutex::new(
            crate::discovery::DeviceTable::default(),
        )),
        broadcast::channel(16).0,
        Duration::from_secs(10),
    ));
    let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let _released = crate::receive::spawn_server(
        state,
        port,
        crate::receive::server_config(&identity).unwrap(),
        crate::receive::port_already_released(),
        shutdown_rx,
    );
    let discovery = crate::discovery::tests::discovery(port, crate::discovery::Presence::Hidden);
    super::wait_until_serving(&discovery.client, port, "Liar").await;

    let device = crate::discovery::probe_target(
        &discovery,
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        "https",
        TEST_TIMEOUT,
    )
    .await
    .unwrap();

    assert_eq!(device.fingerprint, identity.fingerprint());
    assert!(device.is_certified());
}
