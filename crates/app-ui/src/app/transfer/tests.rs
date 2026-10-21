//! transfer.rs 状态机的单元测试（保持宿主文件 ≤800 行）。

use std::collections::HashSet;
use std::sync::Arc;

use local_send::{DeviceInfo, QrSource};
use tokio_util::sync::CancellationToken;

use super::{
    qr_items_from_paths, trusted_fingerprint_set, DirectSendState, FileBrowser, TransferMessage,
    TransferSessionPhase, TransferSessionState,
};
use crate::app::transfer_service::{service_config, LOCAL_SEND_PORT};
use crate::config::TrustedTransferDevice;
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
        port: LOCAL_SEND_PORT,
        protocol: "http".to_owned(),
        address: None,
    }
}

#[test]
fn trust_predicate_reads_shared_snapshot_and_fails_closed() {
    let trusted = Arc::new(parking_lot::RwLock::new(HashSet::from(["fp-1".to_owned()])));
    let config = service_config(&crate::config::default_user_config(), Arc::clone(&trusted));
    let is_trusted = config.is_trusted;
    assert!(is_trusted(&sample_device("fp-1")));
    assert!(!is_trusted(&sample_device("fp-2")));
    // 空 fingerprint 永不信任（fail closed）。
    assert!(!is_trusted(&sample_device("")));
    trusted.write().insert("fp-2".to_owned());
    assert!(is_trusted(&sample_device("fp-2")));
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

#[test]
fn trusted_fingerprint_set_skips_empty_identities() {
    let set = trusted_fingerprint_set(&[
        TrustedTransferDevice {
            fingerprint: String::new(),
            alias: "broken".to_owned(),
            added_at: String::new(),
        },
        TrustedTransferDevice {
            fingerprint: "fp-9".to_owned(),
            alias: "Phone".to_owned(),
            added_at: String::new(),
        },
    ]);
    assert_eq!(set, HashSet::from(["fp-9".to_owned()]));
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

fn session_showing(fingerprint: &str) -> TransferSessionState {
    TransferSessionState {
        phase: TransferSessionPhase::Preparing,
        connected: false,
        bytes_sent: 0,
        speed_bytes_per_second: 0,
        finished: None,
        last_progress: None,
        cancel: CancellationToken::new(),
        direct_send: Some(DirectSendState {
            fingerprint: fingerprint.to_owned(),
            alias: String::new(),
            file_name: String::new(),
            file_index: 0,
            total_files: 0,
            bytes_sent: 0,
            file_size: 0,
            outcome: None,
        }),
    }
}

fn finish_direct_send(browser: &mut FileBrowser, fingerprint: &str, outcome: Result<(), &str>) {
    let _ = browser.handle_transfer_message(TransferMessage::DirectSendFinished {
        fingerprint: fingerprint.to_owned(),
        alias: "Phone".to_owned(),
        outcome: outcome.map_err(str::to_owned),
    });
}

fn displayed_outcome(browser: &FileBrowser) -> Option<Result<(), String>> {
    browser
        .transfer_session
        .as_ref()
        .and_then(|session| session.direct_send.as_ref())
        .and_then(|state| state.outcome.clone())
}

#[test]
fn direct_send_failure_is_logged_once_whatever_the_display_slot_shows() {
    let failure = "failed to transfer one.txt: error sending request for url \
        (http://192.168.1.9:53317/api/localsend/v2/upload?sessionId=s&fileId=0&token=abc123): \
        client error (Connect): tcp connect error: Connection refused (os error 111)";
    let logged_failure = (
        "LocalSend direct send to Phone failed".to_owned(),
        sanitized_application_log_detail(failure),
    );
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

    let logs = capture_direct_send_failure_logs(|| {
        // 窗口已关：没有传输会话可写。
        finish_direct_send(&mut browser, "phone-fp", Err(failure));
        // 显示槽已被另一台设备的直推顶替。
        browser.transfer_session = Some(session_showing("tablet-fp"));
        finish_direct_send(&mut browser, "phone-fp", Err(failure));
        assert_eq!(displayed_outcome(&browser), None);
        // 显示槽正是这次直推：失败照常写进窗口状态。
        browser.transfer_session = Some(session_showing("phone-fp"));
        finish_direct_send(&mut browser, "phone-fp", Err(failure));
        assert_eq!(displayed_outcome(&browser), Some(Err(failure.to_owned())));
        // 成功不记日志。
        browser.transfer_session = Some(session_showing("phone-fp"));
        finish_direct_send(&mut browser, "phone-fp", Ok(()));
        assert_eq!(displayed_outcome(&browser), Some(Ok(())));
    });

    assert_eq!(logs, vec![logged_failure; 3]);
    // 上传 URL 里的会话令牌必须先脱敏再进日志。
    assert!(!logs[0].1.contains("abc123"));
}
