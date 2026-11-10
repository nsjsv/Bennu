//! 发送端：校验不符整文件重传、一个文件失败不影响别的、对方没要的文件。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{prepared, remote_device, spawn_receive, wait_for};
use crate::{LocalSendError, SendReport, ServiceEvent};

/// 假接收端（明文 HTTP）：prepare-upload 只给 `tokens` 里的文件发令牌；upload 前
/// `mismatches` 次回 422，之后 200；记下收到的 upload 与 cancel 次数。
struct FakeReceiver {
    port: u16,
    uploads: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
}

async fn fake_receiver(tokens: &'static [&'static str], mismatches: usize) -> FakeReceiver {
    let uploads = Arc::new(AtomicUsize::new(0));
    let cancels = Arc::new(AtomicUsize::new(0));
    let (upload_count, cancel_count) = (Arc::clone(&uploads), Arc::clone(&cancels));
    let router = axum::Router::new()
        .route(
            "/api/localsend/v2/prepare-upload",
            axum::routing::post(move || async move {
                let files: serde_json::Map<String, serde_json::Value> = tokens
                    .iter()
                    .map(|id| ((*id).to_owned(), serde_json::json!(format!("token-{id}"))))
                    .collect();
                axum::Json(serde_json::json!({"sessionId": "s", "files": files}))
            }),
        )
        .route(
            "/api/localsend/v2/upload",
            axum::routing::post(move |body: axum::body::Bytes| async move {
                drop(body);
                let attempt = upload_count.fetch_add(1, Ordering::SeqCst);
                if attempt < mismatches {
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    axum::http::StatusCode::OK
                }
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
    FakeReceiver {
        port,
        uploads,
        cancels,
    }
}

async fn send_to(
    receiver: &FakeReceiver,
    names: &[&str],
) -> (Result<SendReport, LocalSendError>, tempfile::TempDir) {
    let source = tempfile::tempdir().unwrap();
    let items = names
        .iter()
        .map(|name| {
            let path = source.path().join(name);
            std::fs::write(&path, name.as_bytes()).unwrap();
            path
        })
        .collect();
    let files = prepared(items).await;
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let target = crate::DeviceInfo {
        address: Some("127.0.0.1".parse().unwrap()),
        port: receiver.port,
        protocol: "http".into(),
        ..remote_device("FAKE", "Fake")
    };
    let report = sender
        .send(
            &target,
            &files,
            None,
            tokio::sync::mpsc::channel(4).0,
            &crate::CancellationToken::new(),
        )
        .await;
    (report, source)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checksum_mismatch_resends_the_whole_file() {
    let receiver = fake_receiver(&["0"], 2).await;

    let (report, _source) = send_to(&receiver, &["a.txt"]).await;

    let report = report.unwrap();
    assert_eq!((report.sent(), report.failures()), (1, Vec::new()));
    assert_eq!(receiver.uploads.load(Ordering::SeqCst), 3);
    assert_eq!(receiver.cancels.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_is_given_up_after_three_mismatches_and_the_session_is_canceled() {
    let receiver = fake_receiver(&["0"], usize::MAX).await;

    let (report, _source) = send_to(&receiver, &["a.txt"]).await;

    let report = report.unwrap();
    assert_eq!(report.sent(), 0);
    assert_eq!(
        report.failures()[0].reason,
        "the received file did not match its checksum 3 times"
    );
    assert_eq!(receiver.uploads.load(Ordering::SeqCst), 3);
    assert_eq!(receiver.cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_the_device_did_not_take_are_reported_and_the_rest_are_sent() {
    let receiver = fake_receiver(&["1"], 0).await;

    let (report, _source) = send_to(&receiver, &["a.txt", "b.txt"]).await;

    let report = report.unwrap();
    assert_eq!(report.sent(), 1);
    assert_eq!(report.failures().len(), 1);
    assert_eq!(report.failures()[0].file_name, "a.txt");
    // 没宣告失败的文件：对方的会话本来就不等它，不用取消。
    assert_eq!(receiver.cancels.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_file_failing_mid_send_does_not_stop_the_others() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let source = tempfile::tempdir().unwrap();
    let (first, second) = (source.path().join("a.txt"), source.path().join("b.txt"));
    std::fs::write(&first, b"a").unwrap();
    std::fs::write(&second, b"b").unwrap();
    let files = prepared(vec![first.clone(), second]).await;
    // 校验算完后文件没了：上传时打不开。
    std::fs::remove_file(&first).unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    server.favor(&fingerprint);

    let report = sender
        .send(
            &server.target(),
            &files,
            None,
            tokio::sync::mpsc::channel(4).0,
            &crate::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(report.sent(), 1);
    assert_eq!(report.failures()[0].file_name, "a.txt");
    // 有宣告过的文件没传：取消让对方收尾，收到的那个留着。
    let ServiceEvent::ReceiveFinished { report, .. } = wait_for(&mut server.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(report.saved_paths(), [server._dir.path().join("b.txt")]);
    assert!(report
        .files
        .iter()
        .any(|file| file.file_name == "a.txt" && file.outcome == crate::ReceivedOutcome::Canceled));
}

// —— 端到端：真实接收端 ——

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_is_sent_file_by_file_and_arrives_with_its_structure() {
    let mut server = spawn_receive(false, Duration::from_secs(10)).await;
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), b"first").unwrap();
    let dir = source.path().join("bundle");
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("inner").join("deep.txt"), b"deep").unwrap();
    std::fs::write(dir.join("top.txt"), b"top").unwrap();
    let identity_dir = tempfile::tempdir().unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(identity_dir.path());
    server.favor(&fingerprint);
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
    let files = prepared(vec![source.path().join("one.txt"), dir]).await;

    let report = sender
        .send(
            &server.target(),
            &files,
            None,
            progress_tx,
            &crate::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!((report.sent(), report.failures()), (3, Vec::new()));
    assert!(!report.canceled);
    let ServiceEvent::ReceiveFinished { report, .. } = wait_for(&mut server.events, |e| {
        matches!(e, ServiceEvent::ReceiveFinished { .. })
    })
    .await
    else {
        unreachable!()
    };
    let failed = report.failures();
    assert!(failed.is_empty(), "failed: {failed:?}");
    let received = server._dir.path();
    assert_eq!(std::fs::read(received.join("one.txt")).unwrap(), b"first");
    assert_eq!(
        std::fs::read(received.join("bundle/top.txt")).unwrap(),
        b"top"
    );
    assert_eq!(
        std::fs::read(received.join("bundle/inner/deep.txt")).unwrap(),
        b"deep"
    );
    // 进度通道收到过事件（通道容量有限，只验证存在性）。
    assert!(progress_rx.recv().await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_empty_folders_are_nothing_to_send_and_unreadable_files_never_reach_the_device() {
    let target = remote_device("phone-fp", "Phone");
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("empty")).unwrap();
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let (progress, _) = tokio::sync::mpsc::channel(1);

    let empty = prepared(vec![source.path().join("empty")]).await;
    assert!(matches!(
        sender
            .send(
                &target,
                &empty,
                None,
                progress.clone(),
                &crate::CancellationToken::new()
            )
            .await,
        Err(LocalSendError::NothingToSend)
    ));

    // 都读不了：不连对方（这台设备连地址都没有），直接报告全失败。
    let missing = prepared(vec![source.path().join("missing.txt")]).await;
    let report = sender
        .send(
            &target,
            &missing,
            None,
            progress,
            &crate::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.sent(), 0);
    assert_eq!(report.failures()[0].file_name, "missing.txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_to_device_requires_discovered_address() {
    let target = remote_device("phone-fp", "Phone");
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"x").unwrap();
    let (sender, _) = crate::send::sender_with_new_identity(source.path());
    let (progress, _) = tokio::sync::mpsc::channel(1);
    let result = sender
        .send(
            &target,
            &prepared(vec![file]).await,
            None,
            progress,
            &crate::CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(LocalSendError::AddressUnknown)));
}
