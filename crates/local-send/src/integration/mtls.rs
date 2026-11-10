//! 双向 TLS：真实接收端照新版官方 App（Rust 核心）强制客户端证书、按证书指纹认人；
//! 本机出站请求（直推、register）出示的证书就是自报 fingerprint 对应的那张。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use super::{
    files_map, free_port, meta, post, prepare, spawn_receive, spawn_receive_with, test_peer,
    wait_for, TEST_TIMEOUT,
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
    let seen_by_trust = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen_by_trust);
    let mut server = spawn_receive_with(
        Arc::new(move |device: &DeviceInfo| {
            recorder.lock().push(device.fingerprint.clone());
            true
        }),
        Duration::from_secs(10),
    )
    .await;
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"first").unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    let (progress, _) = mpsc::channel(1);

    sender
        .send(&server.target(), vec![file], progress)
        .await
        .unwrap();

    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    assert_eq!(
        std::fs::read(server._dir.path().join("one.txt")).unwrap(),
        b"first"
    );
    // 接收端按握手时的证书认人：信任判定看到的就是发送方身份的指纹。
    assert_eq!(*seen_by_trust.lock(), vec![fingerprint]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_presents_the_identity_its_fingerprint_claims() {
    let mut server = spawn_receive(true, Duration::from_secs(10)).await;
    let dir = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap();
    let own = crate::own_device_info("Peer".to_owned(), None, free_port().await, &identity);

    let response = crate::discovery::post_register(
        &crate::peer_client(&identity).unwrap(),
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
