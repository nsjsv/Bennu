//! 批次准备与取消：一批只准备一次、好了分给每台在等它的设备；取消只停这一台，等这批
//! 的都取消了才停下准备。

use super::*;
use crate::app::transfer::tests::sample_device;

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

fn add_batch(browser: &mut FileBrowser, batch: u64) -> CancellationToken {
    let cancel = CancellationToken::new();
    browser.transfer_tasks.batches.insert(
        batch,
        Batch {
            progress: PrepareProgress {
                bytes_done: 0,
                bytes_total: 0,
            },
            cancel: cancel.clone(),
        },
    );
    cancel
}

fn checking(browser: &mut FileBrowser, batch: u64, fingerprint: &str) -> u64 {
    browser.transfer_tasks.push(
        TransferKind::Send(DirectSendState::checking_for(
            sample_device(fingerprint),
            batch,
        )),
        false,
    )
}

fn send<'a>(browser: &'a FileBrowser, fingerprint: &str) -> &'a DirectSendState {
    browser.direct_send_to(&sample_device(fingerprint)).unwrap()
}

#[test]
fn one_batch_is_prepared_once_for_every_device_waiting_for_it() {
    let mut browser = browser();
    for (batch, fingerprint) in [(7, "phone"), (7, "tablet"), (8, "laptop")] {
        checking(&mut browser, batch, fingerprint);
    }
    add_batch(&mut browser, 7);
    add_batch(&mut browser, 8);
    // 准备中的设备不能再点发送。
    assert!(!browser.can_send_to(&sample_device("phone")));

    browser.accept_batch_progress(
        7,
        PrepareProgress {
            bytes_done: 50,
            bytes_total: 200,
        },
    );
    assert_eq!(browser.checking_percent(send(&browser, "phone")), 25);

    let files = Arc::new(PreparedFiles::unreadable(String::new()));
    drop(browser.accept_batch_prepared(7, Some(Arc::clone(&files))));

    assert!(!browser.transfer_tasks.batches.contains_key(&7));
    for fingerprint in ["phone", "tablet"] {
        let Payload::Files(ready) = &send(&browser, fingerprint).payload else {
            panic!("{fingerprint} should have the prepared files");
        };
        // 同一份：不是每台各算一遍。
        assert!(Arc::ptr_eq(ready, &files));
    }
    assert_eq!(send(&browser, "laptop").batch(), Some(8));
}

#[test]
fn canceling_one_device_stops_the_preparation_only_when_nobody_waits_for_it() {
    let mut browser = browser();
    let phone = checking(&mut browser, 7, "phone");
    let tablet = checking(&mut browser, 7, "tablet");
    let preparation = add_batch(&mut browser, 7);

    drop(browser.cancel_transfer_task(phone));
    assert_eq!(send(&browser, "phone").phase, DirectSendPhase::Canceled);
    assert!(!preparation.is_cancelled(), "the tablet still waits");
    assert!(browser.can_send_to(&sample_device("phone")));

    drop(browser.cancel_transfer_task(tablet));
    assert!(preparation.is_cancelled());

    // 准备被取消：结果是 None，没人再开始发。
    drop(browser.accept_batch_prepared(7, None));
    assert_eq!(send(&browser, "tablet").phase, DirectSendPhase::Canceled);
}

#[test]
fn canceling_a_running_send_waits_for_it_to_stop() {
    let mut browser = browser();
    let id = crate::app::transfer::tests::start_sending(&mut browser, "phone");
    let token = browser.transfer_tasks.send_mut(id).unwrap().cancel.clone();

    drop(browser.cancel_transfer_task(id));

    assert!(token.is_cancelled());
    assert_eq!(send(&browser, "phone").phase, DirectSendPhase::Canceling);
    assert!(!browser.can_send_to(&sample_device("phone")));
    drop(browser.accept_direct_send_finished(id, "Phone", DirectSendOutcome::Canceled));
    assert_eq!(send(&browser, "phone").phase, DirectSendPhase::Canceled);
}

#[test]
fn progress_tracks_the_whole_batch_and_its_speed() {
    let mut browser = browser();
    let id = crate::app::transfer::tests::start_sending(&mut browser, "phone");
    let progress = |done| local_send::SendProgress {
        file_name: "a".to_owned(),
        file_index: 0,
        total_files: 2,
        bytes_sent: done,
        file_size: 100,
        bytes_done: done,
        bytes_total: 300,
    };

    browser.accept_direct_send_progress(id, progress(0));
    browser.accept_direct_send_progress(id, progress(60));

    let state = send(&browser, "phone");
    assert_eq!((state.bytes_done, state.bytes_total), (60, 300));
    let task = browser
        .transfer_tasks
        .panel_tasks()
        .find(|task| task.id == id)
        .unwrap();
    assert_eq!(browser.transfer_tasks.fraction(task), Some(0.2));
}
