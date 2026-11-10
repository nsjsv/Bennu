//! 收藏设备：只收证书认过的设备、跟上地址与名字、改名、按 IP 添加、接收确认。

use super::*;
use crate::app::transfer::IncomingTransferConfirmation;
use local_send::ServiceEvent;

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

/// 服务发出的设备：`https` = 证书认过（见 `DeviceInfo::is_certified`）。
fn device(fingerprint: &str, protocol: &str, address: &str) -> DeviceInfo {
    let address: SocketAddr = address.parse().unwrap();
    DeviceInfo {
        alias: "Phone".to_owned(),
        version: "2.1".to_owned(),
        fingerprint: fingerprint.to_owned(),
        device_model: None,
        device_type: Some("mobile".to_owned()),
        download: None,
        port: address.port(),
        protocol: protocol.to_owned(),
        address: Some(address.ip()),
    }
}

fn favorite(fingerprint: &str, alias: &str, address: Option<&str>) -> FavoriteTransferDevice {
    FavoriteTransferDevice {
        fingerprint: fingerprint.to_owned(),
        alias: alias.to_owned(),
        custom_alias: false,
        address: address.map(|address| address.parse().unwrap()),
        added_at: String::new(),
    }
}

fn send(browser: &mut FileBrowser, message: TransferMessage) {
    drop(browser.handle_transfer_message(message));
}

fn seen(browser: &mut FileBrowser, device: DeviceInfo) {
    send(
        browser,
        TransferMessage::ServiceEvent(ServiceEvent::DeviceSeen(device)),
    );
}

fn favorites(browser: &FileBrowser) -> &[FavoriteTransferDevice] {
    &browser.user_config.transfer_favorite_devices
}

#[test]
fn favorites_claim_only_certified_devices_with_their_fingerprint() {
    let favorites = [favorite("F1", "Phone", None), favorite("F2", "NAS", None)];
    let devices = [
        device("F1", "https", "192.168.1.5:53317"),
        // 明文设备自称 F2：认不了它是不是那台，留在"附近"。
        device("F2", "http", "192.168.1.66:53317"),
        device("OTHER", "https", "192.168.1.7:53317"),
    ];

    let rows = favorite_rows_among(&favorites, &devices);
    let unclaimed = unclaimed_among(&favorites, &devices);

    assert_eq!(rows[0].online.map(|d| d.address), Some(devices[0].address));
    assert!(rows[1].online.is_none());
    let unclaimed: Vec<_> = unclaimed
        .iter()
        .map(|device| (device.fingerprint.as_str(), device.protocol.as_str()))
        .collect();
    assert_eq!(unclaimed, [("F2", "http"), ("OTHER", "https")]);
}

#[test]
fn starring_adds_a_certified_device_with_its_address_but_never_a_plain_one() {
    let mut browser = browser();

    send(
        &mut browser,
        TransferMessage::FavoritePressed(device("PLAIN", "http", "192.168.1.66:53317")),
    );
    assert!(favorites(&browser).is_empty());

    send(
        &mut browser,
        TransferMessage::FavoritePressed(device("F1", "https", "192.168.1.5:53318")),
    );
    assert_eq!(
        favorites(&browser),
        [FavoriteTransferDevice {
            added_at: favorites(&browser)[0].added_at.clone(),
            ..favorite("F1", "Phone", Some("192.168.1.5:53318"))
        }]
    );

    // ★：取消收藏。
    send(
        &mut browser,
        TransferMessage::UnfavoritePressed("F1".to_owned()),
    );
    assert!(favorites(&browser).is_empty());
}

#[test]
fn a_seen_favorite_follows_its_address_and_name_unless_renamed() {
    let mut browser = browser();
    browser.user_config.transfer_favorite_devices =
        vec![favorite("F1", "Old name", Some("192.168.1.5:53317"))];

    let mut moved = device("F1", "https", "192.168.1.23:53317");
    moved.alias = "New name".to_owned();
    seen(&mut browser, moved);
    assert_eq!(
        favorites(&browser)[0],
        favorite("F1", "New name", Some("192.168.1.23:53317"))
    );

    // 名字是用户起的：地址照样跟，名字不跟。
    browser.user_config.transfer_favorite_devices[0].alias = "Mine".to_owned();
    browser.user_config.transfer_favorite_devices[0].custom_alias = true;
    let mut renamed = device("F1", "https", "192.168.1.30:53317");
    renamed.alias = "Newer".to_owned();
    seen(&mut browser, renamed);
    assert_eq!(favorites(&browser)[0].alias, "Mine");
    assert_eq!(
        favorites(&browser)[0].address,
        Some("192.168.1.30:53317".parse().unwrap())
    );

    // 明文设备自称 F1：冒充者改不了收藏记住的地址。
    seen(&mut browser, device("F1", "http", "192.168.1.66:53317"));
    assert_eq!(
        favorites(&browser)[0].address,
        Some("192.168.1.30:53317".parse().unwrap())
    );
}

#[test]
fn renaming_sets_a_custom_name_and_clearing_it_follows_the_device_again() {
    let mut browser = browser();
    browser.user_config.transfer_favorite_devices = vec![favorite("F1", "Phone", None)];
    let rename = |browser: &mut FileBrowser, value: &str| {
        send(
            browser,
            TransferMessage::FavoriteAliasInputChanged {
                fingerprint: "F1".to_owned(),
                value: value.to_owned(),
            },
        );
        send(
            browser,
            TransferMessage::FavoriteAliasCommitted("F1".to_owned()),
        );
    };

    rename(&mut browser, "  Living room ");
    assert_eq!(favorites(&browser)[0].alias, "Living room");
    assert!(favorites(&browser)[0].custom_alias);
    assert!(browser.favorite_alias_inputs.is_empty());

    // 不在线时清空：名字先不变，下次搜到就换成对方的名字。
    rename(&mut browser, "");
    assert!(!favorites(&browser)[0].custom_alias);
    assert_eq!(favorites(&browser)[0].alias, "Living room");
    seen(&mut browser, device("F1", "https", "192.168.1.5:53317"));
    assert_eq!(favorites(&browser)[0].alias, "Phone");
}

fn open_form(browser: &mut FileBrowser, ip: &str, port: &str) {
    let form = |message| TransferMessage::AddressForm(message);
    send(browser, form(AddressFormMessage::Opened));
    send(browser, form(AddressFormMessage::IpChanged(ip.to_owned())));
    send(
        browser,
        form(AddressFormMessage::PortChanged(port.to_owned())),
    );
}

fn form_state(browser: &FileBrowser) -> &FavoriteAddressForm {
    browser.favorite_address_form.as_ref().unwrap()
}

#[test]
fn the_address_form_starts_with_this_computer_port_and_rejects_bad_input() {
    let mut browser = browser();
    browser.user_config.transfer_port = 53400;
    send(
        &mut browser,
        TransferMessage::AddressForm(AddressFormMessage::Opened),
    );
    assert_eq!(form_state(&browser).port, "53400");

    for (ip, port) in [
        ("abc", "53317"),
        ("192.168.1", "53317"),
        ("::1", "53317"),
        ("192.168.1.20", "0"),
        ("192.168.1.20", "70000"),
        ("192.168.1.20", "port"),
    ] {
        open_form(&mut browser, ip, port);
        send(
            &mut browser,
            TransferMessage::AddressForm(AddressFormMessage::Submitted),
        );
        assert!(form_state(&browser).error.is_some(), "{ip}:{port}");
        assert_eq!(form_state(&browser).connecting, None, "{ip}:{port}");
    }
}

fn connecting_form(browser: &mut FileBrowser, name: &str, address: &str) -> SocketAddr {
    let address: SocketAddr = address.parse().unwrap();
    browser.favorite_address_form = Some(FavoriteAddressForm {
        name: name.to_owned(),
        ip: address.ip().to_string(),
        port: address.port().to_string(),
        connecting: Some(address),
        error: None,
    });
    address
}

fn found(
    browser: &mut FileBrowser,
    address: SocketAddr,
    outcome: Result<DeviceInfo, FindDeviceError>,
) {
    send(
        browser,
        TransferMessage::AddressForm(AddressFormMessage::Found { address, outcome }),
    );
}

#[test]
fn the_address_form_ignores_results_it_is_not_waiting_for() {
    let mut browser = browser();
    let waiting = connecting_form(&mut browser, "", "192.168.1.20:53317");
    // 连接中不能改输入：结果要对上用户看到的地址。
    send(
        &mut browser,
        TransferMessage::AddressForm(AddressFormMessage::IpChanged("10.0.0.1".to_owned())),
    );
    assert_eq!(form_state(&browser).ip, "192.168.1.20");

    let other: SocketAddr = "192.168.1.21:53317".parse().unwrap();
    found(
        &mut browser,
        other,
        Ok(device("F1", "https", "192.168.1.21:53317")),
    );
    assert!(favorites(&browser).is_empty());
    assert_eq!(form_state(&browser).connecting, Some(waiting));

    // 取消后才回来的结果：丢掉。
    send(
        &mut browser,
        TransferMessage::AddressForm(AddressFormMessage::Canceled),
    );
    found(
        &mut browser,
        waiting,
        Ok(device("F1", "https", "192.168.1.20:53317")),
    );
    assert!(favorites(&browser).is_empty());
    assert!(browser.favorite_address_form.is_none());
}

#[test]
fn a_found_device_becomes_a_favorite_under_the_given_name() {
    let mut browser = browser();
    let address = connecting_form(&mut browser, " Living room ", "192.168.1.20:53317");

    found(
        &mut browser,
        address,
        Ok(device("F1", "https", "192.168.1.20:53317")),
    );

    assert!(browser.favorite_address_form.is_none());
    assert_eq!(
        favorites(&browser),
        [FavoriteTransferDevice {
            custom_alias: true,
            added_at: favorites(&browser)[0].added_at.clone(),
            ..favorite("F1", "Living room", Some("192.168.1.20:53317"))
        }]
    );
}

#[test]
fn adding_a_favorite_again_by_ip_updates_its_address() {
    let mut browser = browser();
    browser.user_config.transfer_favorite_devices =
        vec![favorite("F1", "Phone", Some("192.168.1.5:53317"))];
    let address = connecting_form(&mut browser, "", "100.64.0.7:53317");

    found(
        &mut browser,
        address,
        Ok(device("F1", "https", "100.64.0.7:53317")),
    );

    assert_eq!(
        favorites(&browser),
        [favorite("F1", "Phone", Some("100.64.0.7:53317"))]
    );
}

#[test]
fn a_failed_connection_keeps_the_form_open_with_the_reason() {
    for (error, text) in [
        (
            FindDeviceError::NotEncrypted,
            tr!("This device has encryption turned off, so it cannot be a favorite."),
        ),
        (FindDeviceError::ThisDevice, tr!("This is this computer.")),
        (
            FindDeviceError::Unreachable("connection refused".to_owned()),
            tr!("Could not connect: %1", "connection refused"),
        ),
    ] {
        let mut browser = browser();
        let address = connecting_form(&mut browser, "", "192.168.1.20:53317");

        found(&mut browser, address, Err(error));

        assert_eq!(form_state(&browser).error.as_deref(), Some(text.as_str()));
        assert_eq!(form_state(&browser).connecting, None);
        assert!(favorites(&browser).is_empty());
    }
}

fn confirm_incoming(sender: DeviceInfo, add_to_favorites: bool, accept: bool) -> FileBrowser {
    let mut browser = browser();
    browser.incoming_transfer = Some(IncomingTransferConfirmation {
        device: sender,
        session_id: "session".to_owned(),
        files: Vec::new(),
        add_to_favorites,
    });
    send(&mut browser, TransferMessage::IncomingConfirmed { accept });
    browser
}

#[test]
fn accepting_with_add_to_favorites_keeps_the_sender_and_its_address() {
    let sender = device("F1", "https", "192.168.1.5:53317");

    let accepted = confirm_incoming(sender.clone(), true, true);
    assert_eq!(
        favorites(&accepted)[0].address,
        Some("192.168.1.5:53317".parse().unwrap())
    );

    assert!(favorites(&confirm_incoming(sender.clone(), true, false)).is_empty());
    assert!(favorites(&confirm_incoming(sender, false, true)).is_empty());
    let plain = device("F2", "http", "192.168.1.6:53317");
    assert!(favorites(&confirm_incoming(plain, true, true)).is_empty());
}

#[test]
fn closing_the_local_send_window_clears_the_address_form() {
    let mut browser = browser();
    browser.local_send_window = Some(iced::window::Id::unique());
    connecting_form(&mut browser, "", "192.168.1.20:53317");

    drop(browser.close_local_send_window());

    assert!(browser.favorite_address_form.is_none());
}
