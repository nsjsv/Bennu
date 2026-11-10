//! 批次准备：一批只准备一次，好了分给每台在等它的设备。

use super::*;
use crate::app::transfer::tests::sample_device;

fn waiting_for(batch: u64, fingerprint: &str) -> DirectSendState {
    DirectSendState {
        file_index: 0,
        total_files: 0,
        bytes_sent: 0,
        file_size: 0,
        phase: DirectSendPhase::Checking,
        target: sample_device(fingerprint),
        files: SendFiles::Preparing(batch),
    }
}

#[test]
fn one_batch_is_prepared_once_for_every_device_waiting_for_it() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    for (batch, fingerprint) in [(7, "phone"), (7, "tablet"), (8, "laptop")] {
        browser
            .direct_sends
            .states
            .insert(fingerprint.to_owned(), waiting_for(batch, fingerprint));
    }
    for batch in [7, 8] {
        browser.direct_sends.batches.insert(
            batch,
            PrepareProgress {
                bytes_done: 0,
                bytes_total: 0,
            },
        );
    }
    // 准备中的设备不能再点发送。
    assert!(!browser.can_send_to(&sample_device("phone")));

    browser.accept_batch_progress(
        7,
        PrepareProgress {
            bytes_done: 50,
            bytes_total: 200,
        },
    );
    assert_eq!(
        browser.checking_percent(&browser.direct_sends.states["phone"]),
        25
    );

    let files = Arc::new(PreparedFiles::unreadable(String::new()));
    drop(browser.accept_batch_prepared(7, Arc::clone(&files)));

    assert!(!browser.direct_sends.batches.contains_key(&7));
    for fingerprint in ["phone", "tablet"] {
        let SendFiles::Ready(ready) = &browser.direct_sends.states[fingerprint].files else {
            panic!("{fingerprint} should have the prepared files");
        };
        // 同一份：不是每台各算一遍。
        assert!(Arc::ptr_eq(ready, &files));
    }
    assert!(matches!(
        browser.direct_sends.states["laptop"].files,
        SendFiles::Preparing(8)
    ));
}
