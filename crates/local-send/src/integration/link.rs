//! 通过链接接收：手机网页（明文 HTTP，没有证书）按随机码上传到接收目录。

use std::path::Path;
use std::time::Duration;

use super::{client, next_event, wait_for};
use crate::{
    LinkControl, LinkEvent, LinkReceiveConfig, LinkReceiveHandle, LinkReceiveServer,
    PrepareUploadResponse, ReceiveReport, ReceivedOutcome,
};

struct Link {
    handle: LinkReceiveHandle,
    /// `http://127.0.0.1:端口/随机码`
    base: String,
    events: tokio::sync::broadcast::Receiver<LinkEvent>,
}

async fn start_link(download_dir: &Path, idle_timeout: Duration) -> Link {
    let handle = LinkReceiveServer::start_with_idle_timeout(
        LinkReceiveConfig {
            computer_name: "TestPC".into(),
            download_dir: download_dir.to_path_buf(),
        },
        idle_timeout,
    )
    .await
    .unwrap();
    let url = handle.urls().first().expect("a LAN address").clone();
    let (host_port, token) = url.trim_start_matches("http://").split_once('/').unwrap();
    let port = host_port.rsplit(':').next().unwrap();
    let events = handle.events();
    Link {
        base: format!("http://127.0.0.1:{port}/{token}"),
        handle,
        events,
    }
}

fn control(link: &Link) -> LinkControl {
    link.handle.control()
}

/// 网页发的 prepare-upload：只有文件，没有设备信息。
async fn prepare(base: &str, files: &[(&str, usize)]) -> reqwest::Response {
    let files: serde_json::Map<String, serde_json::Value> = files
        .iter()
        .enumerate()
        .map(|(index, (name, size))| {
            (
                index.to_string(),
                serde_json::json!({
                    "id": index.to_string(),
                    "fileName": name,
                    "size": size,
                    "fileType": "application/octet-stream",
                }),
            )
        })
        .collect();
    client()
        .post(format!("{base}/prepare-upload"))
        .json(&serde_json::json!({ "files": files }))
        .send()
        .await
        .unwrap()
}

async fn open_send(base: &str, files: &[(&str, usize)]) -> PrepareUploadResponse {
    let response = prepare(base, files).await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

async fn upload(base: &str, send: &PrepareUploadResponse, file_id: &str, body: &[u8]) -> u16 {
    client()
        .post(format!("{base}/upload"))
        .query(&[
            ("sessionId", send.session_id.as_str()),
            ("fileId", file_id),
            ("token", send.files[file_id].as_str()),
        ])
        .body(body.to_vec())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn finished(events: &mut tokio::sync::broadcast::Receiver<LinkEvent>) -> ReceiveReport {
    let LinkEvent::Finished { report, .. } =
        wait_for(events, |event| matches!(event, LinkEvent::Finished { .. })).await
    else {
        unreachable!()
    };
    report
}

fn outcome_of<'a>(report: &'a ReceiveReport, name: &str) -> &'a ReceivedOutcome {
    &report
        .files
        .iter()
        .find(|file| file.file_name == name)
        .unwrap()
        .outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_sends_files_into_the_download_directory_through_the_link() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"old").unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;
    let wrong = link.base.rsplit_once('/').unwrap().0.to_owned() + "/wrong";

    assert_eq!(
        client().get(&wrong).send().await.unwrap().status(),
        404,
        "only the link with the random code opens the page"
    );
    let page = client().get(&link.base).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("TestPC"));

    let send = open_send(&link.base, &[("a.txt", 3), ("b.bin", 2)]).await;
    let LinkEvent::Accepted { address, files, .. } = next_event(&mut link.events).await else {
        panic!("the computer learns about the send before any file arrives");
    };
    assert_eq!(address.to_string(), "127.0.0.1");
    assert_eq!(files.len(), 2);
    assert_eq!(upload(&link.base, &send, "0", b"new").await, 200);
    assert_eq!(upload(&link.base, &send, "1", b"hi").await, 200);

    let report = finished(&mut link.events).await;
    assert_eq!(
        outcome_of(&report, "a.txt"),
        &ReceivedOutcome::Saved(dir.path().join("a (1).txt"))
    );
    assert_eq!(std::fs::read(dir.path().join("a (1).txt")).unwrap(), b"new");
    assert_eq!(std::fs::read(dir.path().join("b.bin")).unwrap(), b"hi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_phones_send_at_the_same_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;

    let first = open_send(&link.base, &[("one.txt", 1)]).await;
    let second = open_send(&link.base, &[("two.txt", 1)]).await;
    assert_ne!(first.session_id, second.session_id);
    assert_eq!(upload(&link.base, &second, "0", b"2").await, 200);
    assert_eq!(upload(&link.base, &first, "0", b"1").await, 200);

    let reports = [
        finished(&mut link.events).await,
        finished(&mut link.events).await,
    ];
    let saved: Vec<_> = reports
        .iter()
        .flat_map(ReceiveReport::saved_paths)
        .collect();
    assert_eq!(saved.len(), 2);
    assert_eq!(std::fs::read(dir.path().join("one.txt")).unwrap(), b"1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_computer_can_cancel_a_phone_send() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;
    let send = open_send(&link.base, &[("kept.txt", 1), ("left.txt", 1)]).await;
    assert_eq!(upload(&link.base, &send, "0", b"k").await, 200);

    control(&link).cancel_session(&send.session_id);

    let report = finished(&mut link.events).await;
    assert!(matches!(
        outcome_of(&report, "kept.txt"),
        ReceivedOutcome::Saved(_)
    ));
    assert_eq!(outcome_of(&report, "left.txt"), &ReceivedOutcome::Canceled);
    // 网页的下一个文件被拒：会话已经没了，网页据此提示"电脑取消了"。
    assert_eq!(upload(&link.base, &send, "1", b"l").await, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_phone_that_started_a_send_can_cancel_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;
    let send = open_send(&link.base, &[("a.txt", 1)]).await;
    let cancel = |base: &str| {
        client()
            .post(format!("{base}/cancel"))
            .query(&[("sessionId", send.session_id.as_str())])
            .send()
    };
    // 从局域网地址连进来，来源 IP 就不是 127.0.0.1：算另一部手机。
    let other_phone = link.handle.urls().first().unwrap().clone();

    assert_eq!(cancel(&other_phone).await.unwrap().status(), 403);
    assert_eq!(cancel(&link.base).await.unwrap().status(), 200);

    let report = finished(&mut link.events).await;
    assert_eq!(outcome_of(&report, "a.txt"), &ReceivedOutcome::Canceled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_stopping_open_sends_finish_but_nothing_new_starts() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;
    let send = open_send(&link.base, &[("a.txt", 1)]).await;

    control(&link).stop();
    // 停下由后台处理：等到网页打不开为止。
    let mut page_status = 200;
    for _ in 0..50 {
        page_status = client()
            .get(&link.base)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16();
        if page_status == 410 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(page_status, 410);
    assert_eq!(prepare(&link.base, &[("b.txt", 1)]).await.status(), 410);
    assert_eq!(upload(&link.base, &send, "0", b"a").await, 200);
    assert!(finished(&mut link.events).await.saved_paths().len() == 1);
    wait_for(&mut link.events, |event| matches!(event, LinkEvent::Closed)).await;
    assert!(
        client().get(&link.base).send().await.is_err(),
        "the server is gone once the last send finished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_with_nothing_open_closes_right_away() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_secs(60)).await;

    control(&link).stop();

    assert!(matches!(
        next_event(&mut link.events).await,
        LinkEvent::Closed
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_link_ends_what_is_left_and_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut link = start_link(dir.path(), Duration::from_millis(300)).await;
    open_send(&link.base, &[("a.txt", 1)]).await;

    let report = finished(&mut link.events).await;

    assert_eq!(outcome_of(&report, "a.txt"), &ReceivedOutcome::Canceled);
    assert!(matches!(
        next_event(&mut link.events).await,
        LinkEvent::TimedOut
    ));
    assert!(client().get(&link.base).send().await.is_err());
}
