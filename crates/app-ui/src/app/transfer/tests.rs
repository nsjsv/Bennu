//! transfer.rs 状态机的单元测试（保持宿主文件 ≤800 行）。

use std::sync::Arc;

use local_send::{DeviceInfo, QrSource};
use tokio_util::sync::CancellationToken;

use super::{
    qr_items_from_paths, DeviceListOwner, DirectSendFailure, DirectSendOutcome, DirectSendPhase,
    DirectSendState, FileBrowser, TransferKind, TransferMessage, TransferSessionPhase,
    TransferSessionState,
};
use crate::app::transfer_service::service_config;
use crate::config::{FavoriteTransferDevice, TransferAcceptMode};
use crate::model::sanitized_application_log_detail;
use crate::view::transfer_qr::qr_image_handle;

pub(crate) fn sample_device(fingerprint: &str) -> DeviceInfo {
    DeviceInfo {
        alias: "Phone".to_owned(),
        version: "2.0".to_owned(),
        fingerprint: fingerprint.to_owned(),
        device_model: None,
        device_type: Some("phone".to_owned()),
        download: None,
        port: local_send::DEFAULT_PORT,
        protocol: "http".to_owned(),
        address: None,
    }
}

#[test]
fn service_config_carries_favorites_with_their_last_address() {
    let mut user_config = crate::config::default_user_config();
    user_config.transfer_favorite_devices = vec![FavoriteTransferDevice {
        fingerprint: "FP-1".to_owned(),
        alias: "Phone".to_owned(),
        custom_alias: false,
        address: Some("192.168.1.9:53317".parse().unwrap()),
        added_at: String::new(),
    }];

    let config = service_config(&user_config);

    assert_eq!(
        config.favorites,
        vec![local_send::Favorite {
            fingerprint: "FP-1".to_owned(),
            address: Some("192.168.1.9:53317".parse().unwrap()),
        }]
    );
}

#[test]
fn service_config_follows_the_receive_switch_and_port() {
    let mut user_config = crate::config::default_user_config();
    user_config.transfer_port = 50000;

    let receiving = service_config(&user_config);
    assert_eq!(receiving.port, 50000);
    assert!(receiving.receiver.is_some());

    // 关掉接收：没有接收端，端口仍用于发现。
    user_config.transfer_receive_enabled = false;
    let hidden = service_config(&user_config);
    assert_eq!(hidden.port, 50000);
    assert!(hidden.receiver.is_none());
}

fn browser_with_port_input(input: &str) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.transfer_port_input = local_send::DEFAULT_PORT.to_string();
    drop(browser.handle_transfer_message(TransferMessage::PortInputChanged(input.to_owned())));
    browser
}

#[test]
fn invalid_port_input_is_flagged_and_changes_nothing() {
    for input in ["80", "1023", "abc", "70000", ""] {
        let mut browser = browser_with_port_input(input);

        drop(browser.handle_transfer_message(TransferMessage::PortCommitted));

        assert!(browser.transfer_port_invalid, "{input:?} must be rejected");
        assert_eq!(browser.user_config.transfer_port, local_send::DEFAULT_PORT);
        // 输入保留，便于用户改正；再编辑即清掉提示。
        assert_eq!(browser.transfer_port_input, input);
        drop(browser.handle_transfer_message(TransferMessage::PortInputChanged("5".to_owned())));
        assert!(!browser.transfer_port_invalid);
    }
}

#[test]
fn valid_port_input_is_saved_normalized() {
    let mut browser = browser_with_port_input(" 50000 ");

    drop(browser.handle_transfer_message(TransferMessage::PortCommitted));

    assert!(!browser.transfer_port_invalid);
    assert_eq!(browser.user_config.transfer_port, 50000);
    assert_eq!(browser.transfer_port_input, "50000");
    assert_eq!(browser.user_config.user_preferences().transfer_port, 50000);
}

fn commit_download_dir(browser: &mut FileBrowser, input: &str) {
    drop(
        browser.handle_transfer_message(TransferMessage::DownloadDirInputChanged(input.to_owned())),
    );
    drop(browser.handle_transfer_message(TransferMessage::DownloadDirCommitted));
}

#[test]
fn relative_download_dir_is_flagged_and_changes_nothing() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

    commit_download_dir(&mut browser, "Downloads/phone");

    assert!(browser.transfer_download_dir_invalid);
    assert_eq!(browser.user_config.transfer_download_dir, None);
    assert_eq!(browser.transfer_download_dir_input, "Downloads/phone");
    drop(browser.handle_transfer_message(TransferMessage::DownloadDirInputChanged("/".to_owned())));
    assert!(!browser.transfer_download_dir_invalid);
}

#[test]
fn absolute_download_dir_is_saved_and_clearing_returns_to_default() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

    commit_download_dir(&mut browser, " /data/phone ");
    assert_eq!(
        browser.user_config.transfer_download_dir,
        Some(std::path::PathBuf::from("/data/phone"))
    );
    assert_eq!(browser.transfer_download_dir_input, "/data/phone");

    commit_download_dir(&mut browser, "  ");
    assert_eq!(browser.user_config.transfer_download_dir, None);
    assert_eq!(browser.transfer_download_dir_input, "");
}

#[test]
fn chosen_download_dir_replaces_the_draft() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    drop(
        browser.handle_transfer_message(TransferMessage::DownloadDirInputChanged(
            "half typed".to_owned(),
        )),
    );

    drop(
        browser.handle_transfer_message(TransferMessage::DownloadDirChosen(Ok(Some(
            std::path::PathBuf::from("/data/chosen"),
        )))),
    );

    assert_eq!(browser.transfer_download_dir_input, "/data/chosen");
    assert!(!browser.transfer_download_dir_invalid);
}

#[test]
fn receive_switch_toggles_the_saved_preference() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    assert!(browser.user_config.transfer_receive_enabled);

    drop(browser.handle_transfer_message(TransferMessage::ReceiveEnabledToggled));
    assert!(!browser.user_config.transfer_receive_enabled);
    assert!(
        !browser
            .user_config
            .user_preferences()
            .transfer_receive_enabled
    );

    drop(browser.handle_transfer_message(TransferMessage::ReceiveEnabledToggled));
    assert!(browser.user_config.transfer_receive_enabled);
}

#[test]
fn receiver_failure_text_names_the_port_in_both_languages() {
    let failure = local_send::ReceiverFailure::PortInUse { port: 53317 };
    assert!(super::receiver_failure_message(&failure).contains("53317"));
    let chinese = tr_in!(
        crate::config::UiLanguage::Chinese,
        "Could not start receiving: port %1 is already in use. Turn off receiving or choose another port.",
        53317
    );
    assert_eq!(
        chinese,
        "接收没开起来：端口 53317 被占用。关掉接收，或换一个端口。"
    );
}

#[test]
fn qr_image_handle_matches_declared_dimensions() {
    let handle = qr_image_handle("http://192.168.1.5:49152/abcdef");
    let iced::widget::image::Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        panic!("qr handle must be rgba");
    };
    assert_eq!(width, height);
    assert_eq!((width * height * 4) as usize, pixels.len());
    // 静区四角必为白色（透明度 255）。
    assert_eq!(&pixels[0..4], &[255, 255, 255, 255]);
}

#[test]
fn qr_items_classify_directories_and_files() {
    let mut dir = std::env::temp_dir();
    dir.push(format!("bennu-transfer-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, b"hello").unwrap();
    let items = qr_items_from_paths(vec![dir.clone(), file]);
    assert!(matches!(items[0].source, QrSource::Dir(_)));
    assert!(matches!(items[1].source, QrSource::File(_)));
    assert_eq!(items[1].display_name, "note.txt");
    std::fs::remove_dir_all(&dir).ok();
}

/// 收集 `action` 期间正式代码发出的直推失败日志（消息正文, error 字段）：
/// 临时装一个只收集的订阅器，正式代码里不放测试钩子。
fn capture_direct_send_failure_logs(action: impl FnOnce()) -> Vec<(String, String)> {
    use std::sync::Mutex;
    use tracing_subscriber::layer::{Context, SubscriberExt};

    #[derive(Default)]
    struct LoggedFields {
        event: Option<String>,
        message: String,
        error: String,
    }
    impl tracing::field::Visit for LoggedFields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "event" {
                self.event = Some(value.to_owned());
            }
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            match field.name() {
                "message" => self.message = format!("{value:?}"),
                "error" => self.error = format!("{value:?}"),
                _ => {}
            }
        }
    }

    struct FailureLogCollector(Arc<Mutex<Vec<(String, String)>>>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for FailureLogCollector {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if event.metadata().target() != "app_ui::transfer" {
                return;
            }
            let mut fields = LoggedFields::default();
            event.record(&mut fields);
            if fields.event.as_deref() == Some("localsend_direct_send_failed") {
                self.0.lock().unwrap().push((fields.message, fields.error));
            }
        }
    }

    let logs = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(FailureLogCollector(Arc::clone(&logs)));
    tracing::subscriber::with_default(subscriber, action);
    let collected = logs.lock().unwrap().clone();
    collected
}

fn qr_session() -> TransferSessionState {
    TransferSessionState {
        phase: TransferSessionPhase::Preparing,
        connected: false,
        bytes_sent: 0,
        speed_bytes_per_second: 0,
        finished: None,
        last_progress: None,
        cancel: CancellationToken::new(),
        paths: vec![std::path::PathBuf::from("/tmp/one.txt")],
    }
}

/// 这台设备最新那次发送的任务 id；没有（任务已不在）时给一个不存在的 id。
fn task_of(browser: &FileBrowser, fingerprint: &str) -> u64 {
    browser
        .transfer_tasks
        .latest_send_to(fingerprint)
        .map_or(u64::MAX, |(id, _)| id)
}

fn finish_direct_send(browser: &mut FileBrowser, fingerprint: &str, outcome: DirectSendOutcome) {
    let task = task_of(browser, fingerprint);
    let _ = browser.handle_transfer_message(TransferMessage::DirectSendFinished {
        task,
        alias: "Phone".to_owned(),
        outcome,
    });
}

fn send_of<'a>(browser: &'a FileBrowser, fingerprint: &str) -> &'a DirectSendState {
    browser.direct_send_to(&sample_device(fingerprint)).unwrap()
}

fn phase_of(browser: &FileBrowser, fingerprint: &str) -> DirectSendPhase {
    send_of(browser, fingerprint).phase.clone()
}

pub(crate) fn start_sending(browser: &mut FileBrowser, fingerprint: &str) -> u64 {
    let panel_open = browser.operation_queue.is_panel_open();
    browser.transfer_tasks.push(
        TransferKind::Send(DirectSendState::sending_to(sample_device(fingerprint))),
        panel_open,
    )
}

/// 一份发送报告：`sent` 个传成的文件 + 给的失败。
pub(crate) fn report(
    sent: usize,
    failures: Vec<local_send::FileFailure>,
) -> local_send::SendReport {
    let sent_files = (0..sent).map(|index| local_send::FileReport {
        file_name: format!("sent-{index}.txt"),
        path: Some(std::path::PathBuf::from(format!("/tmp/sent-{index}.txt"))),
        size: 10,
        outcome: local_send::FileOutcome::Sent,
    });
    let failed_files = failures.into_iter().map(|failure| local_send::FileReport {
        file_name: failure.file_name,
        path: None,
        size: 0,
        outcome: local_send::FileOutcome::Failed(failure.reason),
    });
    local_send::SendReport {
        files: sent_files.chain(failed_files).collect(),
        canceled: false,
    }
}

fn all_sent() -> local_send::SendReport {
    report(1, Vec::new())
}

fn finished(sent: usize, failed: Vec<local_send::FileFailure>) -> DirectSendPhase {
    DirectSendPhase::Finished {
        sent,
        failed,
        details_open: false,
    }
}

fn failed(reason: &str) -> DirectSendOutcome {
    DirectSendOutcome::Failed(DirectSendFailure::Other(reason.to_owned()))
}

#[test]
fn direct_send_results_land_on_their_device_and_failures_are_logged_once() {
    let failure = "failed to transfer one.txt: error sending request for url \
        (http://192.168.1.9:53317/api/localsend/v2/upload?sessionId=s&fileId=0&token=abc123): \
        client error (Connect): tcp connect error: Connection refused (os error 111)";
    let logged_failure = (
        "LocalSend direct send to Phone failed".to_owned(),
        sanitized_application_log_detail(failure),
    );
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    for fingerprint in ["phone-fp", "tablet-fp"] {
        start_sending(&mut browser, fingerprint);
    }

    let phone = task_of(&browser, "phone-fp");
    let logs = capture_direct_send_failure_logs(|| {
        let _ = browser.handle_transfer_message(TransferMessage::DirectSendProgress {
            task: phone,
            progress: local_send::SendProgress {
                file_name: "one.txt".to_owned(),
                file_index: 1,
                total_files: 3,
                bytes_sent: 5,
                file_size: 10,
                bytes_done: 15,
                bytes_total: 30,
            },
        });
        finish_direct_send(&mut browser, "phone-fp", failed(failure));
        finish_direct_send(
            &mut browser,
            "tablet-fp",
            DirectSendOutcome::Sent(all_sent()),
        );
        // 设备状态已不在（不该发生）时失败照样进日志：日志是长期记录。
        finish_direct_send(&mut browser, "gone-fp", failed(failure));
    });

    assert_eq!(send_of(&browser, "phone-fp").file_index, 1);
    assert_eq!(
        phase_of(&browser, "phone-fp"),
        DirectSendPhase::Failed(DirectSendFailure::Other(failure.to_owned()))
    );
    assert_eq!(send_of(&browser, "tablet-fp").file_index, 0);
    assert_eq!(phase_of(&browser, "tablet-fp"), finished(1, Vec::new()));
    assert!(browser.direct_send_to(&sample_device("gone-fp")).is_none());
    // 成功不记日志。
    assert_eq!(logs, vec![logged_failure; 2]);
    // 上传 URL 里的会话令牌必须先脱敏再进日志。
    assert!(!logs[0].1.contains("abc123"));
}

#[test]
fn a_device_that_is_sending_cannot_start_another_send() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let phone = sample_device("phone-fp");
    assert!(browser.can_send_to(&phone));

    start_sending(&mut browser, "phone-fp");
    assert!(!browser.can_send_to(&phone));
    assert!(browser.can_send_to(&sample_device("tablet-fp")));

    finish_direct_send(
        &mut browser,
        "phone-fp",
        DirectSendOutcome::Sent(all_sent()),
    );
    assert!(browser.can_send_to(&phone));
}

#[test]
fn a_pin_request_opens_the_prompt_until_it_is_canceled() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let phone = sample_device("phone-fp");
    start_sending(&mut browser, "phone-fp");

    let logs = capture_direct_send_failure_logs(|| {
        finish_direct_send(&mut browser, "phone-fp", DirectSendOutcome::PinRequired);
        assert_eq!(
            phase_of(&browser, "phone-fp"),
            DirectSendPhase::NeedsPin {
                input: String::new(),
                wrong: false
            }
        );
        // 等 PIN 时主"发送"不可点：输完用同一批文件重发。
        assert!(!browser.can_send_to(&phone));
        let _ = browser.handle_transfer_message(TransferMessage::DirectSendPinChanged {
            fingerprint: "phone-fp".to_owned(),
            value: "4821".to_owned(),
        });
        assert_eq!(
            phase_of(&browser, "phone-fp"),
            DirectSendPhase::NeedsPin {
                input: "4821".to_owned(),
                wrong: false
            }
        );

        // 重发后对方说 PIN 不对：清空输入、提示再输。
        finish_direct_send(&mut browser, "phone-fp", DirectSendOutcome::WrongPin);
        assert_eq!(
            phase_of(&browser, "phone-fp"),
            DirectSendPhase::NeedsPin {
                input: String::new(),
                wrong: true
            }
        );
        // 空的不发。
        let _ = browser.handle_transfer_message(TransferMessage::DirectSendPinSubmitted(
            "phone-fp".to_owned(),
        ));
        assert!(matches!(
            phase_of(&browser, "phone-fp"),
            DirectSendPhase::NeedsPin { .. }
        ));

        let _ = browser.handle_transfer_message(TransferMessage::DirectSendCancelPressed(
            "phone-fp".to_owned(),
        ));
    });

    assert_eq!(phase_of(&browser, "phone-fp"), DirectSendPhase::Canceled);
    assert!(browser.can_send_to(&phone));
    // 要 PIN、PIN 不对、取消都不是失败，不进日志。
    assert!(logs.is_empty());
}

#[test]
fn being_declined_or_blocked_ends_the_send_and_is_logged() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    start_sending(&mut browser, "phone-fp");
    start_sending(&mut browser, "tablet-fp");

    let logs = capture_direct_send_failure_logs(|| {
        finish_direct_send(
            &mut browser,
            "phone-fp",
            DirectSendOutcome::Failed(DirectSendFailure::Declined),
        );
        finish_direct_send(
            &mut browser,
            "tablet-fp",
            DirectSendOutcome::Failed(DirectSendFailure::TooManyPinAttempts),
        );
    });

    assert_eq!(
        phase_of(&browser, "phone-fp"),
        DirectSendPhase::Failed(DirectSendFailure::Declined)
    );
    assert!(browser.can_send_to(&sample_device("tablet-fp")));
    let reasons: Vec<_> = logs.into_iter().map(|(_, reason)| reason).collect();
    assert_eq!(
        reasons,
        ["the device declined the files", "too many wrong PINs"]
    );
}

#[test]
fn shutdown_closes_the_transfer_window_and_cancels_its_qr_session() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let session = qr_session();
    let cancel = session.cancel.clone();
    browser.transfer_session = Some(session);
    browser.transfer_window = Some(iced::window::Id::unique());

    drop(browser.close_auxiliary_window(browser.main_window));

    assert_eq!(browser.transfer_window, None);
    assert!(browser.transfer_session.is_none());
    assert!(
        cancel.is_cancelled(),
        "the QR download service must be shut down"
    );
}

fn send_settings(browser: &mut FileBrowser, message: TransferMessage) {
    drop(browser.handle_transfer_message(message));
}

#[test]
fn a_receive_pin_takes_effect_only_once_a_non_empty_one_is_saved() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

    // 打开只露出输入框。
    send_settings(&mut browser, TransferMessage::ReceivePinToggled);
    assert_eq!(browser.transfer_pin_input.as_deref(), Some(""));
    assert_eq!(browser.user_config.transfer_receive_pin, None);

    send_settings(&mut browser, TransferMessage::ReceivePinCommitted);
    assert!(browser.transfer_pin_invalid);
    assert_eq!(browser.user_config.transfer_receive_pin, None);

    send_settings(
        &mut browser,
        TransferMessage::ReceivePinInputChanged(" 4821 ".to_owned()),
    );
    assert!(!browser.transfer_pin_invalid);
    send_settings(&mut browser, TransferMessage::ReceivePinCommitted);
    assert_eq!(
        browser.user_config.transfer_receive_pin.as_deref(),
        Some("4821")
    );
    assert_eq!(browser.transfer_pin_input.as_deref(), Some("4821"));

    // 关掉 = 不要 PIN，并忘掉它。
    send_settings(&mut browser, TransferMessage::ReceivePinToggled);
    assert_eq!(browser.user_config.transfer_receive_pin, None);
    assert_eq!(browser.transfer_pin_input, None);
    send_settings(&mut browser, TransferMessage::ReceivePinToggled);
    assert_eq!(browser.transfer_pin_input.as_deref(), Some(""));
}

#[test]
fn choosing_an_accept_mode_changes_what_the_receiver_is_given() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    assert_eq!(
        browser.user_config.transfer_accept_mode,
        TransferAcceptMode::AcceptFavorites
    );

    send_settings(
        &mut browser,
        TransferMessage::AcceptModeSelected(TransferAcceptMode::AskEveryone),
    );
    browser.user_config.transfer_receive_pin = Some("4821".to_owned());

    let receiver = service_config(&browser.user_config)
        .receiver
        .expect("receiving is on by default");
    assert_eq!(receiver.accept, local_send::AcceptPolicy::AskEveryone);
    assert_eq!(receiver.pin.as_deref(), Some("4821"));
}

#[test]
fn a_send_with_failed_files_shows_them_on_demand_and_logs_them_once() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    start_sending(&mut browser, "phone-fp");
    let failure = local_send::FileFailure {
        file_name: "照片/a.jpg".to_owned(),
        reason: "Permission denied (os error 13)".to_owned(),
    };

    let logs = capture_direct_send_failure_logs(|| {
        finish_direct_send(
            &mut browser,
            "phone-fp",
            DirectSendOutcome::Sent(report(2, vec![failure.clone()])),
        );
    });

    assert_eq!(
        phase_of(&browser, "phone-fp"),
        finished(2, vec![failure.clone()])
    );
    let reasons: Vec<_> = logs.into_iter().map(|(_, reason)| reason).collect();
    assert_eq!(reasons, ["照片/a.jpg: Permission denied (os error 13)"]);
    let toggle = TransferMessage::DirectSendDetailsToggled("phone-fp".to_owned());
    send_settings(&mut browser, toggle.clone());
    assert!(matches!(
        phase_of(&browser, "phone-fp"),
        DirectSendPhase::Finished {
            details_open: true,
            ..
        }
    ));
    send_settings(&mut browser, toggle);
    assert_eq!(phase_of(&browser, "phone-fp"), finished(2, vec![failure]));

    // 只有空文件夹：报"没有可发的"。
    start_sending(&mut browser, "tablet-fp");
    finish_direct_send(
        &mut browser,
        "tablet-fp",
        DirectSendOutcome::Failed(DirectSendFailure::NothingToSend),
    );
    assert_eq!(
        phase_of(&browser, "tablet-fp"),
        DirectSendPhase::Failed(DirectSendFailure::NothingToSend)
    );
}

#[test]
fn each_window_keeps_its_own_checked_devices_until_it_closes() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let check = |owner, fingerprint: &str, checked| TransferMessage::DeviceChecked {
        owner,
        fingerprint: fingerprint.to_owned(),
        checked,
    };
    send_settings(
        &mut browser,
        check(DeviceListOwner::LocalSendWindow, "phone", true),
    );
    send_settings(
        &mut browser,
        check(DeviceListOwner::LocalSendWindow, "tablet", true),
    );
    send_settings(
        &mut browser,
        check(DeviceListOwner::LocalSendWindow, "tablet", false),
    );
    send_settings(
        &mut browser,
        check(DeviceListOwner::TransferWindow, "laptop", true),
    );

    let checked = |browser: &FileBrowser, owner| {
        let mut checked: Vec<_> = browser.checked_devices.of(owner).iter().cloned().collect();
        checked.sort();
        checked
    };
    assert_eq!(
        checked(&browser, DeviceListOwner::LocalSendWindow),
        ["phone"]
    );
    assert_eq!(
        checked(&browser, DeviceListOwner::TransferWindow),
        ["laptop"]
    );

    browser.local_send_window = Some(iced::window::Id::unique());
    drop(browser.close_local_send_window());
    assert!(checked(&browser, DeviceListOwner::LocalSendWindow).is_empty());
    assert_eq!(
        checked(&browser, DeviceListOwner::TransferWindow),
        ["laptop"]
    );

    send_settings(
        &mut browser,
        TransferMessage::CheckedCleared(DeviceListOwner::TransferWindow),
    );
    assert!(checked(&browser, DeviceListOwner::TransferWindow).is_empty());
}
