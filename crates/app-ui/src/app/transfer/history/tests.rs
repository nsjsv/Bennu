//! 传输记录：哪些情况记几条、上限、读入合并、清空、点开。

use local_send::{FileFailure, FileReport, ReceivedFile, ServiceEvent};

use super::*;
use crate::app::transfer::tests::{report, sample_device, start_sending};
use crate::app::transfer::{DirectSendFailure, DirectSendOutcome, DirectSendState, TransferKind};

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

/// (方向, 设备, 名字或文字, 结果)：新的在前。
fn summary(browser: &FileBrowser) -> Vec<(Direction, String, String, HistoryResult)> {
    browser
        .transfer_history
        .records
        .iter()
        .map(|record| {
            let item = match &record.item {
                HistoryItem::File { name, more, .. } if *more > 0 => format!("{name} +{more}"),
                HistoryItem::File { name, .. } => name.clone(),
                HistoryItem::Text(text) => format!("\"{text}\""),
            };
            (
                record.direction,
                record.device.clone(),
                item,
                record.result.clone(),
            )
        })
        .collect()
}

fn finish(browser: &mut FileBrowser, id: u64, outcome: DirectSendOutcome) {
    drop(browser.accept_direct_send_finished(id, "Phone", outcome));
}

#[test]
fn an_accepted_send_records_each_file_and_canceled_ones_as_canceled() {
    let mut browser = browser();
    let id = start_sending(&mut browser, "phone");
    let mut sent = report(
        1,
        vec![FileFailure {
            file_name: "照片/b.jpg".to_owned(),
            reason: "HTTP 500".to_owned(),
        }],
    );
    sent.files.push(FileReport {
        file_name: "照片/c.jpg".to_owned(),
        path: Some("/tmp/c.jpg".into()),
        size: 3,
        outcome: local_send::FileOutcome::Canceled,
    });
    sent.canceled = true;

    finish(&mut browser, id, DirectSendOutcome::Sent(sent));

    let sent = |name: &str, result| (Direction::Sent, "Phone".to_owned(), name.to_owned(), result);
    assert_eq!(
        summary(&browser),
        [
            sent("sent-0.txt", HistoryResult::Done),
            sent("照片/b.jpg", HistoryResult::Failed("HTTP 500".to_owned())),
            sent("照片/c.jpg", HistoryResult::Canceled),
        ]
    );
    // 传成的那个记着源文件位置与大小：点了能打开。
    assert!(matches!(
        &browser.transfer_history.records[0].item,
        HistoryItem::File { path: Some(path), size: Some(10), .. } if path.ends_with("sent-0.txt")
    ));
    assert_eq!(
        browser
            .direct_send_to(&sample_device("phone"))
            .unwrap()
            .phase,
        crate::app::transfer::DirectSendPhase::Canceled
    );
}

#[test]
fn a_send_ended_before_acceptance_is_one_record() {
    let mut browser = browser();
    for outcome in [
        DirectSendOutcome::Failed(DirectSendFailure::Declined),
        DirectSendOutcome::Failed(DirectSendFailure::TooManyPinAttempts),
        DirectSendOutcome::Canceled,
        DirectSendOutcome::Failed(DirectSendFailure::Other("connection refused".to_owned())),
    ] {
        let id = start_sending(&mut browser, "phone");
        finish(&mut browser, id, outcome);
    }
    // 要 PIN 不是结束：不记。
    let id = start_sending(&mut browser, "tablet");
    finish(&mut browser, id, DirectSendOutcome::PinRequired);

    let whole = |result| {
        (
            Direction::Sent,
            "Phone".to_owned(),
            "one.txt".to_owned(),
            result,
        )
    };
    assert_eq!(
        summary(&browser),
        [
            whole(HistoryResult::Failed("connection refused".to_owned())),
            whole(HistoryResult::Canceled),
            whole(HistoryResult::TooManyPinAttempts),
            whole(HistoryResult::Declined),
        ]
    );
}

#[test]
fn canceling_while_checking_records_the_whole_send_once() {
    let mut browser = browser();
    let mut state = DirectSendState::checking_for(sample_device("phone"), 7);
    state.item_count = 3;
    let id = browser
        .transfer_tasks
        .push(TransferKind::Send(state), false);

    drop(browser.cancel_transfer_task(id));
    drop(browser.cancel_transfer_task(id));

    assert_eq!(
        summary(&browser),
        [(
            Direction::Sent,
            "Phone".to_owned(),
            "one.txt +2".to_owned(),
            HistoryResult::Canceled
        )]
    );
}

#[test]
fn text_is_recorded_both_ways() {
    let mut browser = browser();
    let id = browser.begin_text_for_test("phone");
    finish(
        &mut browser,
        id,
        DirectSendOutcome::Sent(report(0, Vec::new())),
    );
    drop(
        browser.handle_transfer_message(TransferMessage::ServiceEvent(
            ServiceEvent::MessageReceived {
                device: sample_device("tablet"),
                text: "收到".to_owned(),
            },
        )),
    );

    assert_eq!(
        summary(&browser),
        [
            (
                Direction::Received,
                "Phone".to_owned(),
                "\"收到\"".to_owned(),
                HistoryResult::Done
            ),
            (
                Direction::Sent,
                "Phone".to_owned(),
                "\"hi\"".to_owned(),
                HistoryResult::Done
            ),
        ]
    );
}

#[test]
fn a_receive_records_each_file_with_where_it_landed() {
    let mut browser = browser();
    drop(
        browser.handle_transfer_message(TransferMessage::ServiceEvent(
            ServiceEvent::RequestAccepted {
                session_id: "s".to_owned(),
                device: sample_device("phone"),
                files: Vec::new(),
            },
        )),
    );
    let file = |name: &str, outcome| ReceivedFile {
        file_name: name.to_owned(),
        size: 5,
        outcome,
    };

    drop(
        browser.handle_transfer_message(TransferMessage::ServiceEvent(
            ServiceEvent::ReceiveFinished {
                session_id: "s".to_owned(),
                report: ReceiveReport {
                    files: vec![
                        file("a.jpg", ReceivedOutcome::Saved("/dl/a (1).jpg".into())),
                        file("b.jpg", ReceivedOutcome::Failed("write failed".to_owned())),
                        file("c.jpg", ReceivedOutcome::Canceled),
                    ],
                },
            },
        )),
    );

    let received = |name: &str, result| {
        (
            Direction::Received,
            "Phone".to_owned(),
            name.to_owned(),
            result,
        )
    };
    assert_eq!(
        summary(&browser),
        [
            received("a.jpg", HistoryResult::Done),
            received("b.jpg", HistoryResult::Failed("write failed".to_owned())),
            received("c.jpg", HistoryResult::Canceled),
        ]
    );
    // 记的是落盘位置（重名改过的）。
    assert!(matches!(
        &browser.transfer_history.records[0].item,
        HistoryItem::File { path: Some(path), .. } if path.ends_with("a (1).jpg")
    ));
}

fn text_record(text: &str) -> NewRecord {
    NewRecord {
        direction: Direction::Received,
        device: "Phone".to_owned(),
        item: HistoryItem::Text(text.to_owned()),
        result: HistoryResult::Done,
    }
}

#[test]
fn only_the_latest_records_are_kept_and_unsaved_ones_wait_for_the_database() {
    let mut browser = browser();

    for index in 0..(HISTORY_LIMIT + 50) {
        drop(browser.record_history(vec![text_record(&index.to_string())]));
    }

    let records = &browser.transfer_history.records;
    assert_eq!(records.len(), HISTORY_LIMIT);
    assert_eq!(records[0].item, HistoryItem::Text("249".to_owned()));
    // 测试里没有状态库：都等着随读入一起写。
    assert_eq!(browser.transfer_history.unsaved.len(), HISTORY_LIMIT + 50);
}

#[test]
fn loading_merges_with_records_made_before_it_without_duplicates() {
    let mut browser = browser();
    drop(browser.record_history(vec![text_record("new")]));
    let made_before_load = browser.transfer_history.records[0].clone();
    let older = HistoryRecord {
        id: "older".to_owned(),
        at: made_before_load.at - Duration::from_secs(60),
        ..made_before_load.clone()
    };

    browser.accept_history_loaded(Ok(vec![made_before_load.clone(), older.clone()]));

    assert_eq!(browser.transfer_history.records, [made_before_load, older]);
}

#[test]
fn clearing_empties_the_list() {
    let mut browser = browser();
    drop(browser.record_history(vec![text_record("a")]));

    drop(browser.handle_transfer_message(TransferMessage::HistoryClearPressed));

    assert!(browser.transfer_history.records.is_empty());
}

fn record_with(browser: &mut FileBrowser, record: NewRecord) -> String {
    drop(browser.record_history(vec![record]));
    browser.transfer_history.records[0].id.clone()
}

#[test]
fn opening_a_text_record_shows_it_again_titled_by_direction() {
    let mut browser = browser();
    let id = record_with(
        &mut browser,
        NewRecord {
            direction: Direction::Sent,
            ..text_record("今晚 8 点")
        },
    );

    drop(browser.handle_transfer_message(TransferMessage::HistoryRecordOpened(id)));

    let shown = browser.incoming_messages.front().unwrap();
    assert_eq!(shown.title, MessageTitle::Sent("Phone".to_owned()));
    assert_eq!(shown.text, "今晚 8 点");
}

fn file_record(path: Option<PathBuf>, more: u64) -> NewRecord {
    NewRecord {
        direction: Direction::Received,
        device: "Phone".to_owned(),
        item: HistoryItem::File {
            name: "a.txt".to_owned(),
            path,
            size: Some(1),
            more,
        },
        result: HistoryResult::Done,
    }
}

#[test]
fn opening_a_file_record_selects_it_in_a_new_tab_or_says_it_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let mut browser = browser();
    let tabs_before = browser.pane().tabs.len();
    let present = record_with(&mut browser, file_record(Some(file.clone()), 0));

    drop(browser.handle_transfer_message(TransferMessage::HistoryRecordOpened(present)));

    assert_eq!(browser.pane().tabs.len(), tabs_before + 1);
    assert_eq!(browser.tab().current_dir, dir.path());
    assert!(browser.tab().selected_paths.contains(&file));

    let gone = record_with(
        &mut browser,
        file_record(Some(dir.path().join("gone.txt")), 0),
    );
    drop(browser.handle_transfer_message(TransferMessage::HistoryRecordOpened(gone)));
    assert!(browser
        .current_error()
        .is_some_and(|error| error.contains("gone.txt")));
    assert_eq!(browser.pane().tabs.len(), tabs_before + 1);

    // 整次没传成的一条：没东西可开。
    let whole = record_with(&mut browser, file_record(None, 3));
    drop(browser.handle_transfer_message(TransferMessage::HistoryRecordOpened(whole)));
    assert_eq!(browser.pane().tabs.len(), tabs_before + 1);
}
