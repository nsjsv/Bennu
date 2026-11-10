//! 取消与进度：发送端撤回/中途取消、接收端本机取消、进度带总量、收下时报谁发了什么。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use super::{
    files_map, free_port, meta, post, prepared, remote_device, run_trusted_session,
    start_receiver_with, wait_for, wait_until_serving, TEST_TIMEOUT,
};
use crate::{AcceptPolicy, CancellationToken, LocalSendError, SendProgress, ServiceEvent};

fn two_files(dir: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    let first = dir.path().join("a.txt");
    let second = dir.path().join("b.txt");
    std::fs::write(&first, b"hello").unwrap();
    std::fs::write(&second, b"world!!").unwrap();
    vec![first, second]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_progress_reports_each_file_start_and_the_whole_batch() {
    let mut server = super::spawn_receive(false, Duration::from_secs(10)).await;
    let source = tempfile::tempdir().unwrap();
    let files = prepared(two_files(&source)).await;
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    server.favor(&fingerprint);
    let (progress, mut progress_rx) = mpsc::channel(64);

    sender
        .send(
            &server.target(),
            &files,
            None,
            progress,
            &CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut events: Vec<SendProgress> = Vec::new();
    while let Ok(event) = progress_rx.try_recv() {
        events.push(event);
    }
    let summary: Vec<_> = events
        .iter()
        .map(|event| (event.file_index, event.bytes_done, event.bytes_total))
        .collect();
    // 每个文件开始报一次 0，结束报一次满；整批 = 5 + 7 字节。
    assert_eq!(summary, [(0, 0, 12), (0, 5, 12), (1, 5, 12), (1, 12, 12)]);
    wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_while_the_device_decides_withdraws_the_request() {
    let server = start_receiver_with(
        "TestPC",
        free_port().await,
        crate::receive::port_already_released(),
        Duration::from_secs(30),
        AcceptPolicy::AskEveryone,
        None,
    );
    let mut events = server.events.resubscribe();
    wait_until_serving(&server.sender.client, server.port, "TestPC").await;
    let source = tempfile::tempdir().unwrap();
    let files = prepared(two_files(&source)).await;
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let cancel = CancellationToken::new();
    let target = server.target();
    let sending = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            sender
                .send(&target, &files, None, mpsc::channel(4).0, &cancel)
                .await
        })
    };
    let ServiceEvent::IncomingRequest { session_id, .. } = wait_for(&mut events, |event| {
        matches!(event, ServiceEvent::IncomingRequest { .. })
    })
    .await
    else {
        unreachable!()
    };

    cancel.cancel();

    assert!(matches!(
        sending.await.unwrap(),
        Err(LocalSendError::Canceled)
    ));
    // 接收端不用等 30 秒：确认弹窗马上收起。
    let ServiceEvent::RequestRejected {
        session_id: withdrawn,
    } = tokio::time::timeout(Duration::from_secs(5), async {
        wait_for(&mut events, |event| {
            matches!(event, ServiceEvent::RequestRejected { .. })
        })
        .await
    })
    .await
    .expect("the withdrawn request should be dropped at once")
    else {
        unreachable!()
    };
    assert_eq!(withdrawn, session_id);
}

/// 假接收端：upload 读到请求体后一直不回（卡在传输中）；记 cancel 次数。
async fn stalling_receiver() -> (u16, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let cancels = Arc::new(AtomicUsize::new(0));
    let uploading = Arc::new(tokio::sync::Notify::new());
    let (cancel_count, started) = (Arc::clone(&cancels), Arc::clone(&uploading));
    let router = axum::Router::new()
        .route(
            "/api/localsend/v2/prepare-upload",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"sessionId": "s", "files": {"0": "t", "1": "t"}}))
            }),
        )
        .route(
            "/api/localsend/v2/upload",
            axum::routing::post(move || async move {
                started.notify_one();
                futures_util::future::pending::<()>().await;
            }),
        )
        .route(
            "/api/localsend/v2/cancel",
            axum::routing::post(move || async move {
                cancel_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (port, cancels, uploading)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_mid_transfer_stops_and_tells_the_device() {
    let (port, cancels, uploading) = stalling_receiver().await;
    let source = tempfile::tempdir().unwrap();
    let files = prepared(two_files(&source)).await;
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let target = crate::DeviceInfo {
        address: Some("127.0.0.1".parse().unwrap()),
        port,
        protocol: "http".into(),
        ..remote_device("FAKE", "Fake")
    };
    let cancel = CancellationToken::new();
    let sending = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            sender
                .send(&target, &files, None, mpsc::channel(4).0, &cancel)
                .await
        })
    };
    tokio::time::timeout(TEST_TIMEOUT, uploading.notified())
        .await
        .unwrap();

    cancel.cancel();

    assert!(matches!(
        tokio::time::timeout(TEST_TIMEOUT, sending)
            .await
            .unwrap()
            .unwrap(),
        Err(LocalSendError::Canceled)
    ));
    assert_eq!(cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_a_receive_keeps_finished_files_and_stops_the_one_being_written() {
    let (mut server, session) = run_trusted_session(vec![
        ("0", meta("0", "done.txt", 5)),
        ("1", meta("1", "partial.bin", 100)),
    ])
    .await;
    let upload = |file_id: &str| {
        format!(
            "/api/localsend/v2/upload?sessionId={}&fileId={file_id}&token={}",
            session.session_id, session.files[file_id],
        )
    };
    assert_eq!(
        post(&server, &upload("0"))
            .body("hello")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let stalled_body = futures_util::stream::iter([Ok::<_, std::io::Error>(
        axum::body::Bytes::from_static(b"abc"),
    )])
    .chain(futures_util::stream::pending());
    let stalled = tokio::spawn(
        post(&server, &upload("1"))
            .body(reqwest::Body::wrap_stream(stalled_body))
            .send(),
    );
    let partial = server._dir.path().join("partial.bin");
    for _ in 0..100 {
        if std::fs::read(&partial).is_ok_and(|bytes| bytes == b"abc") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        server
            .state
            .end_session(&session.session_id, "canceled on this computer")
            .await
    );

    // 正在写的那个上传马上停下（回给对方的是错误，不是成功）。
    let response = tokio::time::timeout(TEST_TIMEOUT, stalled)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 409);
    let ServiceEvent::ReceiveFinished {
        saved_paths,
        failed,
        ..
    } = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(saved_paths, [server._dir.path().join("done.txt")]);
    assert_eq!(
        failed,
        [(
            "partial.bin".to_owned(),
            "canceled on this computer".to_owned()
        )]
    );
    assert!(!partial.exists());
    // 对方后面的文件发不进来了。
    assert_eq!(
        post(&server, &upload("1"))
            .body("x")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_accepted_session_tells_who_sent_what_and_reports_progress_per_file() {
    let mut server = super::spawn_receive(true, Duration::from_secs(10)).await;

    let response = server
        .sender
        .client
        .post(format!("{}/api/localsend/v2/prepare-upload", server.base))
        .json(&crate::PrepareUploadRequest {
            info: remote_device("sender-fp", "Sender"),
            files: files_map(vec![("7", meta("7", "a.txt", 5))]),
        })
        .send()
        .await
        .unwrap();
    let session: crate::PrepareUploadResponse = response.json().await.unwrap();

    let ServiceEvent::RequestAccepted { device, files, .. } =
        wait_for(&mut server.events, |event| {
            matches!(event, ServiceEvent::RequestAccepted { .. })
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(device.fingerprint, server.sender.fingerprint);
    assert_eq!(files[0].file_name, "a.txt");

    post(
        &server,
        &format!(
            "/api/localsend/v2/upload?sessionId={}&fileId=7&token={}",
            session.session_id, session.files["7"],
        ),
    )
    .body("hello")
    .send()
    .await
    .unwrap();
    let ServiceEvent::ReceiveProgress {
        file_id, received, ..
    } = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveProgress { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!((file_id.as_str(), received), ("7", 5));
}
