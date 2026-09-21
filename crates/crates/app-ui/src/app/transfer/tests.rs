//! transfer.rs 状态机的单元测试（保持宿主文件 ≤800 行）。

use std::collections::HashSet;
use std::sync::Arc;

use local_send::{DeviceInfo, QrSource};

use super::{qr_items_from_paths, trusted_fingerprint_set};
use crate::app::transfer_service::{service_config, LOCAL_SEND_PORT};
use crate::config::TrustedTransferDevice;
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
