//! 通过链接接收：出码的几个阶段、停止与关窗、旧链接的消息、手机发来的接收进任务面板
//! 与记录并能取消（真服务）。

use std::time::Duration;

use local_send::{LinkReceiveHandle, ReceivedOutcome};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::app::transfer::{Direction, HistoryResult, ReceivePhase, TransferKind};

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

fn send(browser: &mut FileBrowser, message: LinkReceiveMessage) {
    drop(browser.handle_transfer_message(TransferMessage::LinkReceive(message)));
}

fn phase(browser: &FileBrowser) -> Option<&LinkReceivePhase> {
    browser
        .link_receiving
        .current
        .as_ref()
        .map(|current| &current.phase)
}

fn chosen(browser: &FileBrowser) -> Option<usize> {
    match phase(browser) {
        Some(LinkReceivePhase::Ready(links)) => links.chosen.as_ref().map(|(index, _)| *index),
        other => panic!("not showing addresses: {other:?}"),
    }
}

fn started(link: u64) -> LinkReceiveMessage {
    LinkReceiveMessage::Started {
        link,
        urls: vec![
            "http://192.168.1.5:4000/a".into(),
            "http://10.0.0.2:4000/a".into(),
        ],
    }
}

#[test]
fn receiving_from_a_phone_lists_addresses_then_shows_the_chosen_code() {
    let mut browser = browser();

    send(&mut browser, LinkReceiveMessage::StartPressed);
    assert!(matches!(phase(&browser), Some(LinkReceivePhase::Starting)));
    send(&mut browser, started(0));
    assert_eq!(chosen(&browser), None);
    send(&mut browser, LinkReceiveMessage::AddressChosen(1));
    assert_eq!(chosen(&browser), Some(1));
    send(&mut browser, LinkReceiveMessage::AddressListRequested);
    assert_eq!(chosen(&browser), None);
}

#[test]
fn pressing_start_again_while_the_link_is_up_keeps_it() {
    let mut browser = browser();
    send(&mut browser, LinkReceiveMessage::StartPressed);
    send(&mut browser, started(0));

    send(&mut browser, LinkReceiveMessage::StartPressed);

    assert_eq!(browser.link_receiving.next_id, 1);
    assert!(matches!(phase(&browser), Some(LinkReceivePhase::Ready(_))));
}

#[test]
fn stopping_or_closing_the_window_takes_the_link_down() {
    let mut browser = browser();
    send(&mut browser, LinkReceiveMessage::StartPressed);
    let stop = browser
        .link_receiving
        .current
        .as_ref()
        .unwrap()
        .stop
        .clone();

    send(&mut browser, LinkReceiveMessage::StopPressed);

    assert!(phase(&browser).is_none());
    assert!(stop.is_cancelled());

    send(&mut browser, LinkReceiveMessage::StartPressed);
    let stop = browser
        .link_receiving
        .current
        .as_ref()
        .unwrap()
        .stop
        .clone();
    drop(browser.close_local_send_window());

    assert!(phase(&browser).is_none());
    assert!(stop.is_cancelled());
}

#[test]
fn a_stopped_link_cannot_change_the_current_one() {
    let mut browser = browser();
    send(&mut browser, LinkReceiveMessage::StartPressed);
    send(&mut browser, LinkReceiveMessage::StopPressed);
    send(&mut browser, LinkReceiveMessage::StartPressed);
    let control = real_link().control();

    send(&mut browser, started(0));
    send(
        &mut browser,
        LinkReceiveMessage::Event {
            link: 0,
            control: control.clone(),
            event: LinkEvent::TimedOut,
        },
    );
    assert!(matches!(phase(&browser), Some(LinkReceivePhase::Starting)));

    send(
        &mut browser,
        LinkReceiveMessage::Event {
            link: 1,
            control,
            event: LinkEvent::TimedOut,
        },
    );
    assert!(matches!(phase(&browser), Some(LinkReceivePhase::TimedOut)));
}

#[test]
fn a_failed_start_says_why_and_can_be_tried_again() {
    let mut browser = browser();
    send(&mut browser, LinkReceiveMessage::StartPressed);

    send(
        &mut browser,
        LinkReceiveMessage::Failed {
            link: 0,
            error: "no usable LAN address".into(),
        },
    );
    assert!(matches!(phase(&browser), Some(LinkReceivePhase::Failed(_))));
    send(&mut browser, LinkReceiveMessage::StartPressed);

    assert!(matches!(phase(&browser), Some(LinkReceivePhase::Starting)));
    assert_eq!(browser.link_receiving.next_id, 2);
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

thread_local! {
    static RUNTIME: tokio::runtime::Runtime = runtime();
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

fn real_link() -> LinkReceiveHandle {
    let directory = tempfile::tempdir().unwrap().keep();
    block_on(LinkReceiveServer::start(LinkReceiveConfig {
        computer_name: "PC".into(),
        download_dir: directory,
    }))
    .unwrap()
}

/// 手机网页那样开一次发送（明文 HTTP）。
async fn phone_opens_a_send(handle: &LinkReceiveHandle) {
    let url = handle.urls()[0].trim_start_matches("http://").to_owned();
    let (host_port, token) = url.split_once('/').unwrap();
    let address = format!("127.0.0.1:{}", host_port.rsplit(':').next().unwrap());
    let body = r#"{"files":{"0":{"id":"0","fileName":"a.jpg","size":3}}}"#;
    let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
    let request = format!(
        "POST /{token}/prepare-upload HTTP/1.1\r\nHost: {address}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}

async fn next(events: &mut broadcast::Receiver<LinkEvent>) -> LinkEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn a_phone_send_is_a_receive_task_that_cancels_through_its_link() {
    let mut browser = browser();
    let handle = real_link();
    let mut events = handle.events();
    let deliver = |browser: &mut FileBrowser, event| {
        send(
            browser,
            LinkReceiveMessage::Event {
                link: 0,
                control: handle.control(),
                event,
            },
        );
    };

    let accepted = block_on(async {
        phone_opens_a_send(&handle).await;
        next(&mut events).await
    });
    deliver(&mut browser, accepted);
    let task = browser.transfer_tasks.panel_tasks().next().unwrap();
    let (id, TransferKind::Receive(receive)) = (task.id, &task.kind) else {
        panic!("a phone send is a receive task");
    };
    assert_eq!(receive.sender, tr!("Browser %1", "127.0.0.1"));
    assert!(matches!(receive.source, ReceiveSource::Link(_)));

    drop(browser.cancel_transfer_task(id));
    let finished = block_on(next(&mut events));
    let LinkEvent::Finished { report, .. } = &finished else {
        panic!("canceling ends the session on the link: {finished:?}");
    };
    assert_eq!(report.files[0].outcome, ReceivedOutcome::Canceled);
    deliver(&mut browser, finished);

    let task = browser.transfer_tasks.panel_tasks().next().unwrap();
    assert!(matches!(
        &task.kind,
        TransferKind::Receive(receive) if receive.phase == ReceivePhase::Canceled { saved: 0 }
    ));
    let record = &browser.transfer_history.records[0];
    assert_eq!(
        (record.direction, record.device.as_str(), &record.result),
        (
            Direction::Received,
            tr!("Browser %1", "127.0.0.1").as_str(),
            &HistoryResult::Canceled
        )
    );
}
