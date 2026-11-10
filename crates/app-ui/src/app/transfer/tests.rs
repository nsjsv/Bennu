//! transfer.rs 状态机的单元测试（保持宿主文件 ≤800 行）。

use std::sync::Arc;

use local_send::{DeviceInfo, QrSource};
use tokio_util::sync::CancellationToken;

use super::{
    qr_items_from_paths, DirectSendState, FileBrowser, TransferMessage, TransferSessionPhase,
    TransferSessionState,
};
use crate::app::transfer_service::service_config;
use crate::config::FavoriteTransferDevice;
use crate::model::sanitized_application_log_detail;
use crate::view::transfer_qr::qr_image_handle;

fn sample_device(fingerprint: &str) -> DeviceInfo {
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

fn finish_direct_send(browser: &mut FileBrowser, fingerprint: &str, outcome: Result<(), &str>) {
    let _ = browser.handle_transfer_message(TransferMessage::DirectSendFinished {
        fingerprint: fingerprint.to_owned(),
        alias: "Phone".to_owned(),
        outcome: outcome.map_err(str::to_owned),
    });
}

fn outcome_of(browser: &FileBrowser, fingerprint: &str) -> Option<Result<(), String>> {
    browser.direct_sends[fingerprint].outcome.clone()
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
        browser
            .direct_sends
            .insert(fingerprint.to_owned(), DirectSendState::started());
    }

    let logs = capture_direct_send_failure_logs(|| {
        let _ = browser.handle_transfer_message(TransferMessage::DirectSendProgress {
            fingerprint: "phone-fp".to_owned(),
            progress: local_send::SendProgress {
                file_name: "one.txt".to_owned(),
                file_index: 1,
                total_files: 3,
                bytes_sent: 5,
                file_size: 10,
            },
        });
        finish_direct_send(&mut browser, "phone-fp", Err(failure));
        finish_direct_send(&mut browser, "tablet-fp", Ok(()));
        // 设备状态已不在（不该发生）时失败照样进日志：日志是长期记录。
        finish_direct_send(&mut browser, "gone-fp", Err(failure));
    });

    assert_eq!(browser.direct_sends["phone-fp"].file_index, 1);
    assert_eq!(
        outcome_of(&browser, "phone-fp"),
        Some(Err(failure.to_owned()))
    );
    assert_eq!(browser.direct_sends["tablet-fp"].file_index, 0);
    assert_eq!(outcome_of(&browser, "tablet-fp"), Some(Ok(())));
    assert!(!browser.direct_sends.contains_key("gone-fp"));
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

    browser
        .direct_sends
        .insert("phone-fp".to_owned(), DirectSendState::started());
    assert!(!browser.can_send_to(&phone));
    assert!(browser.can_send_to(&sample_device("tablet-fp")));

    finish_direct_send(&mut browser, "phone-fp", Ok(()));
    assert!(browser.can_send_to(&phone));
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
