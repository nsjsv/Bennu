//! 任务列表：面板里哪些行、未读、清除、进度环、接收任务的进度与取消。

use local_send::ServiceEvent;

use super::*;
use crate::app::transfer::tests::{sample_device, start_sending};
use crate::app::transfer::{DirectSendOutcome, IncomingTransferConfirmation, TransferMessage};
use crate::app::FileBrowser;

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

fn event(browser: &mut FileBrowser, event: ServiceEvent) {
    drop(browser.handle_transfer_message(TransferMessage::ServiceEvent(event)));
}

fn file(id: &str, size: u64) -> FileMetadata {
    FileMetadata {
        id: id.to_owned(),
        file_name: format!("{id}.jpg"),
        size,
        file_type: None,
        sha256: None,
        preview: None,
    }
}

/// a.jpg 收下了，b.jpg 是 `second`。
fn received(second: local_send::ReceivedOutcome) -> local_send::ReceiveReport {
    local_send::ReceiveReport {
        files: vec![
            local_send::ReceivedFile {
                file_name: "a.jpg".to_owned(),
                size: 100,
                outcome: local_send::ReceivedOutcome::Saved("/tmp/a.jpg".into()),
            },
            local_send::ReceivedFile {
                file_name: "b.jpg".to_owned(),
                size: 300,
                outcome: second,
            },
        ],
    }
}

fn start_receiving(browser: &mut FileBrowser, session: &str) {
    event(
        browser,
        ServiceEvent::RequestAccepted {
            session_id: session.to_owned(),
            device: sample_device("phone"),
            files: vec![file("a", 100), file("b", 300)],
        },
    );
}

fn receive<'a>(browser: &'a FileBrowser, session: &str) -> &'a ReceiveTask {
    browser
        .transfer_tasks
        .panel_tasks()
        .find_map(|task| match &task.kind {
            TransferKind::Receive(receive) if receive.session_id == session => Some(receive),
            _ => None,
        })
        .unwrap()
}

fn panel_ids(browser: &FileBrowser) -> Vec<u64> {
    browser
        .transfer_tasks
        .panel_tasks()
        .map(|task| task.id)
        .collect()
}

#[test]
fn a_receive_shows_progress_per_file_and_its_result() {
    let mut browser = browser();
    start_receiving(&mut browser, "s");

    let progress = |file_id: &str, received| ServiceEvent::ReceiveProgress {
        session_id: "s".to_owned(),
        file_id: file_id.to_owned(),
        received,
    };
    event(&mut browser, progress("a", 100));
    event(&mut browser, progress("b", 50));
    let task = receive(&browser, "s");
    assert_eq!((task.bytes_done(), task.bytes_total()), (150, 400));
    assert_eq!(task.files_done(), 1);
    assert_eq!(task.sender, "Phone");

    event(
        &mut browser,
        ServiceEvent::ReceiveFinished {
            session_id: "s".to_owned(),
            report: received(local_send::ReceivedOutcome::Failed(
                "write failed".to_owned(),
            )),
        },
    );
    assert_eq!(
        receive(&browser, "s").phase,
        ReceivePhase::Finished {
            saved: 1,
            failed: vec![("b.jpg".to_owned(), "write failed".to_owned())]
        }
    );
    assert!(browser.transfer_tasks.has_unread_failed());
}

#[test]
fn canceling_a_receive_shows_it_as_canceled_once_the_session_ends() {
    let mut browser = browser();
    start_receiving(&mut browser, "s");
    let id = panel_ids(&browser)[0];

    drop(browser.handle_transfer_message(TransferMessage::TaskCancelRequested(id)));
    assert_eq!(receive(&browser, "s").phase, ReceivePhase::Canceling);

    event(
        &mut browser,
        ServiceEvent::ReceiveFinished {
            session_id: "s".to_owned(),
            report: received(local_send::ReceivedOutcome::Canceled),
        },
    );
    assert_eq!(
        receive(&browser, "s").phase,
        ReceivePhase::Canceled { saved: 1 }
    );
    assert!(!browser.transfer_tasks.has_unread_failed());
}

#[test]
fn a_new_send_keeps_the_earlier_one_in_the_panel_but_replaces_an_old_text_send() {
    let mut browser = browser();
    let first = start_sending(&mut browser, "phone");
    drop(browser.accept_direct_send_finished(
        first,
        "Phone",
        DirectSendOutcome::Sent(crate::app::transfer::tests::report(1, Vec::new())),
    ));
    let second = start_sending(&mut browser, "phone");
    assert_eq!(panel_ids(&browser), [first, second]);
    // 设备行显示最新的那次。
    assert_eq!(
        browser
            .transfer_tasks
            .latest_send_to("phone")
            .map(|(id, _)| id),
        Some(second)
    );

    // 文字发送不进面板，被下一次发送取代。
    drop(browser.accept_direct_send_finished(second, "Phone", DirectSendOutcome::Canceled));
    let text = browser.begin_text_for_test("phone");
    assert_eq!(panel_ids(&browser), [first, second]);
    drop(browser.accept_direct_send_finished(text, "Phone", DirectSendOutcome::Canceled));
    let third = start_sending(&mut browser, "phone");
    assert!(browser
        .transfer_tasks
        .latest_send_to("phone")
        .is_some_and(|(id, _)| id == third));
    assert!(browser.transfer_tasks.send_mut(text).is_none());
}

#[test]
fn unread_clear_and_the_indicator_cover_localsend_rows() {
    let mut browser = browser();
    let done = start_sending(&mut browser, "phone");
    let running = start_sending(&mut browser, "tablet");
    browser.accept_direct_send_progress(
        running,
        local_send::SendProgress {
            file_name: "a".to_owned(),
            file_index: 0,
            total_files: 1,
            bytes_sent: 25,
            file_size: 100,
            bytes_done: 25,
            bytes_total: 100,
        },
    );
    drop(browser.accept_direct_send_finished(
        done,
        "Phone",
        DirectSendOutcome::Sent(crate::app::transfer::tests::report(1, Vec::new())),
    ));
    assert_eq!(browser.transfer_tasks.unread_count(), 2);
    assert_eq!(browser.transfer_tasks.active_fraction(), Some(0.25));
    assert!(!browser.transfer_tasks.has_indeterminate());

    // 打开面板 = 都看过了。
    drop(browser.update(crate::model::Message::FileOperationIndicatorPressed));
    assert_eq!(browser.transfer_tasks.unread_count(), 0);

    // 清除只清已结束的；清除已结束同时作用于 LocalSend 行。
    browser.transfer_tasks.clear(running);
    assert_eq!(panel_ids(&browser), [done, running]);
    drop(browser.update(crate::model::Message::FileOperationClearFinishedRequested));
    assert_eq!(panel_ids(&browser), [running]);
}

#[test]
fn waiting_for_acceptance_animates_the_indicator() {
    let mut browser = browser();
    start_sending(&mut browser, "phone");

    assert!(browser.transfer_tasks.has_active());
    assert_eq!(browser.transfer_tasks.active_fraction(), None);
    assert!(browser.transfer_tasks.has_indeterminate());
}

#[test]
fn a_withdrawn_request_closes_its_confirmation() {
    let mut browser = browser();
    browser.incoming_transfer = Some(IncomingTransferConfirmation {
        device: sample_device("phone"),
        session_id: "s".to_owned(),
        files: Vec::new(),
        add_to_favorites: false,
    });

    event(
        &mut browser,
        ServiceEvent::RequestRejected {
            session_id: "other".to_owned(),
        },
    );
    assert!(browser.incoming_transfer.is_some());
    event(
        &mut browser,
        ServiceEvent::RequestRejected {
            session_id: "s".to_owned(),
        },
    );
    assert!(browser.incoming_transfer.is_none());
}

#[test]
fn speed_is_measured_between_samples_at_least_half_a_second_apart() {
    let mut meter = SpeedMeter::default();
    let start = Instant::now();

    meter.sample(0, start);
    meter.sample(1_000, start + Duration::from_millis(100));
    assert_eq!(meter.bytes_per_second, 0);
    meter.sample(2_000, start + Duration::from_secs(1));
    assert_eq!(meter.bytes_per_second, 2_000);
    // 重传让字节倒退：重新开始量，不报负数。
    meter.sample(10, start + Duration::from_secs(2));
    assert_eq!(meter.bytes_per_second, 2_000);
}
