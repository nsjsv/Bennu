//! 文字消息：发送端照官方的写法发，接收端认出来回 204、不走接收确认。

use std::time::Duration;

use reqwest::StatusCode;
use tokio::sync::broadcast;

use super::{
    files_map, free_port, meta, remote_device, start_receiver_with, wait_for, wait_until_serving,
    ReceiveServer,
};
use crate::device::{FileMetadata, PrepareUploadRequest};
use crate::{AcceptPolicy, LocalSendError, ServiceEvent};

async fn spawn(accept: AcceptPolicy, pin: Option<&str>) -> ReceiveServer {
    let server = start_receiver_with(
        "TestPC",
        free_port().await,
        crate::receive::port_already_released(),
        Duration::from_millis(200),
        accept,
        pin,
    );
    wait_until_serving(&server.sender.client, server.port, "TestPC").await;
    server
}

fn message(text: &str, file_type: &str) -> FileMetadata {
    FileMetadata {
        file_type: Some(file_type.to_owned()),
        preview: Some(text.to_owned()),
        ..meta("0", "note.txt", text.len() as u64)
    }
}

async fn prepare(server: &ReceiveServer, files: Vec<(&str, FileMetadata)>) -> StatusCode {
    server
        .sender
        .client
        .post(format!("{}/api/localsend/v2/prepare-upload", server.base))
        .json(&PrepareUploadRequest {
            info: remote_device("sender-fp", "Sender"),
            files: files_map(files),
        })
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_is_shown_without_asking_even_when_everything_is_confirmed() {
    let mut server = spawn(AcceptPolicy::AskEveryone, None).await;

    let status = prepare(
        &server,
        vec![("0", message("你好 https://x.y", "text/plain"))],
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    let ServiceEvent::MessageReceived { device, text } = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::MessageReceived { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(text, "你好 https://x.y");
    // 按证书认发送方，不信自报的 "sender-fp"。
    assert_eq!(device.fingerprint, server.sender.fingerprint);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_that_only_look_like_messages_go_through_confirmation() {
    let mut server = spawn(AcceptPolicy::AskEveryone, None).await;

    // 不是 text 类型、没带 preview、多个文件：都是普通文件，要确认（这里没人确认，超时 403）。
    let not_text = message("x", "image/png");
    let no_preview = FileMetadata {
        preview: None,
        ..message("x", "text/plain")
    };
    let two = vec![
        ("0", message("a", "text/plain")),
        ("1", message("b", "text/plain")),
    ];
    for files in [vec![("0", not_text)], vec![("0", no_preview)], two] {
        assert_eq!(prepare(&server, files).await, StatusCode::FORBIDDEN);
    }
    // 旧版官方写 `text` 而不是 MIME：照样是文字。
    assert_eq!(
        prepare(&server, vec![("0", message("legacy", "text"))]).await,
        StatusCode::NO_CONTENT
    );
    let mut messages = 0;
    while let Ok(event) = server.events.try_recv() {
        messages += usize::from(matches!(event, ServiceEvent::MessageReceived { .. }));
    }
    assert_eq!(messages, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_still_needs_the_pin() {
    let mut server = spawn(AcceptPolicy::AcceptEveryone, Some("4821")).await;

    let status = prepare(&server, vec![("0", message("hi", "text/plain"))]).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(matches!(
        server.events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sender_delivers_a_message_and_handles_pins_like_files() {
    let mut server = spawn(AcceptPolicy::AskEveryone, Some("4821")).await;
    let identity = tempfile::tempdir().unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(identity.path());

    assert!(matches!(
        sender
            .send_text(
                &server.target(),
                "hi",
                None,
                &crate::CancellationToken::new()
            )
            .await,
        Err(LocalSendError::PinRequired)
    ));
    sender
        .send_text(
            &server.target(),
            "第一行\n第二行",
            Some("4821"),
            &crate::CancellationToken::new(),
        )
        .await
        .unwrap();

    let ServiceEvent::MessageReceived { device, text } = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::MessageReceived { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(text, "第一行\n第二行");
    assert_eq!(device.fingerprint, fingerprint);
}

/// 把文字当文件收的接收端（给了令牌）：文字内容作为那个文件传过去。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_that_wants_the_message_as_a_file_gets_its_content() {
    let received = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let saved = std::sync::Arc::clone(&received);
    let router = axum::Router::new()
        .route(
            "/api/localsend/v2/prepare-upload",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"sessionId": "s", "files": {"0": "t"}}))
            }),
        )
        .route(
            "/api/localsend/v2/upload",
            axum::routing::post(move |body: axum::body::Bytes| async move {
                saved.lock().push(body.to_vec());
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let identity = tempfile::tempdir().unwrap();
    let (sender, _) = crate::send::sender_with_new_identity(identity.path());
    let target = crate::DeviceInfo {
        address: Some("127.0.0.1".parse().unwrap()),
        port,
        protocol: "http".into(),
        ..remote_device("FAKE", "Fake")
    };

    sender
        .send_text(&target, "hello", None, &crate::CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(*received.lock(), [b"hello".to_vec()]);
}
