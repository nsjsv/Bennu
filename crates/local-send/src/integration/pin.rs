//! PIN 与接收确认三档：接收端照官方 `pin.rs` 拦人，发送端把拒绝原因分清楚。

use std::time::Duration;

use reqwest::StatusCode;
use tokio::sync::{broadcast, mpsc};

use super::{
    files_map, free_port, meta, remote_device, start_receiver_with, wait_for, wait_until_serving,
    ReceiveServer,
};
use crate::device::PrepareUploadRequest;
use crate::{AcceptPolicy, LocalSendError, ServiceEvent};

async fn spawn(
    accept: AcceptPolicy,
    pin: Option<&str>,
    confirm_timeout: Duration,
) -> ReceiveServer {
    let server = start_receiver_with(
        "TestPC",
        free_port().await,
        crate::receive::port_already_released(),
        confirm_timeout,
        accept,
        pin,
    );
    wait_until_serving(&server.sender.client, server.port, "TestPC").await;
    server
}

/// 收藏了自带发送方、要 PIN 的接收端：过了 PIN 就直接收。
async fn spawn_with_pin(pin: &str) -> ReceiveServer {
    let server = spawn(
        AcceptPolicy::AcceptFavorites,
        Some(pin),
        Duration::from_secs(10),
    )
    .await;
    server.favor(&server.sender.fingerprint.clone());
    server
}

async fn prepare(server: &ReceiveServer, pin: Option<&str>) -> StatusCode {
    let mut request = server
        .sender
        .client
        .post(format!("{}/api/localsend/v2/prepare-upload", server.base));
    if let Some(pin) = pin {
        request = request.query(&[("pin", pin)]);
    }
    request
        .json(&PrepareUploadRequest {
            info: remote_device("sender-fp", "Sender"),
            files: files_map(vec![("0", meta("0", "a.txt", 5))]),
        })
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pin_is_required_from_everyone_including_favorites() {
    let mut server = spawn_with_pin("4821").await;

    assert_eq!(prepare(&server, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        prepare(&server, Some("1111")).await,
        StatusCode::UNAUTHORIZED
    );
    // 被 PIN 挡下的请求不弹确认、不进会话。
    assert!(matches!(
        server.events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(prepare(&server, Some("4821")).await, StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_wrong_pins_block_the_address_even_with_the_right_one() {
    let server = spawn_with_pin("4821").await;

    for _ in 0..3 {
        assert_eq!(
            prepare(&server, Some("0000")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    assert_eq!(
        prepare(&server, Some("4821")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_right_pin_clears_earlier_mistakes_and_a_missing_one_is_not_a_mistake() {
    let server = spawn_with_pin("4821").await;

    for _ in 0..5 {
        assert_eq!(prepare(&server, None).await, StatusCode::UNAUTHORIZED);
    }
    for _ in 0..2 {
        assert_eq!(
            prepare(&server, Some("0000")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(prepare(&server, Some("4821")).await, StatusCode::CREATED);
    for _ in 0..2 {
        assert_eq!(
            prepare(&server, Some("0000")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    assert_eq!(prepare(&server, Some("4821")).await, StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asking_everyone_asks_favorites_too() {
    let mut server = spawn(AcceptPolicy::AskEveryone, None, Duration::from_millis(200)).await;
    server.favor(&server.sender.fingerprint.clone());

    // 没人确认：超时拒绝。
    assert_eq!(prepare(&server, None).await, StatusCode::FORBIDDEN);
    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::IncomingRequest { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepting_everyone_takes_files_from_strangers() {
    let server = spawn(
        AcceptPolicy::AcceptEveryone,
        None,
        Duration::from_millis(200),
    )
    .await;

    assert_eq!(prepare(&server, None).await, StatusCode::CREATED);
}

async fn send_one_file(
    server: &ReceiveServer,
    pin: Option<&str>,
) -> (Result<crate::SendReport, LocalSendError>, tempfile::TempDir) {
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"first").unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    server.favor(&fingerprint);
    let (progress, _) = mpsc::channel(4);
    let result = sender
        .send(
            &server.target(),
            &super::prepared(vec![file]).await,
            pin,
            progress,
            &crate::CancellationToken::new(),
        )
        .await;
    (result, source)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sender_tells_a_missing_pin_from_a_wrong_one_and_sends_with_the_right_one() {
    // 空格、&、中文：PIN 必须原样到达。
    let pin = "a&b c中";
    let mut server = spawn(
        AcceptPolicy::AcceptFavorites,
        Some(pin),
        Duration::from_secs(10),
    )
    .await;

    assert!(matches!(
        send_one_file(&server, None).await.0,
        Err(LocalSendError::PinRequired)
    ));
    assert!(matches!(
        send_one_file(&server, Some("abc")).await.0,
        Err(LocalSendError::WrongPin)
    ));
    send_one_file(&server, Some(pin)).await.0.unwrap();
    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sender_reports_being_blocked_and_being_declined() {
    let server = spawn(
        AcceptPolicy::AcceptFavorites,
        Some("4821"),
        Duration::from_secs(10),
    )
    .await;
    for _ in 0..3 {
        let _ = send_one_file(&server, Some("0000")).await;
    }
    assert!(matches!(
        send_one_file(&server, Some("4821")).await.0,
        Err(LocalSendError::TooManyPinAttempts)
    ));

    let asking = spawn(AcceptPolicy::AskEveryone, None, Duration::from_millis(200)).await;
    assert!(matches!(
        send_one_file(&asking, None).await.0,
        Err(LocalSendError::Declined)
    ));
}
