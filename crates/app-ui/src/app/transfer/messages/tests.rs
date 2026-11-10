//! 收到的文字：排队、显示名、复制、链接判定、关掉看下一条、后台才通知。

use super::*;
use crate::config::FavoriteTransferDevice;
use local_send::ServiceEvent;

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

fn sender(fingerprint: &str, protocol: &str) -> DeviceInfo {
    DeviceInfo {
        alias: "Phone".to_owned(),
        version: "2.1".to_owned(),
        fingerprint: fingerprint.to_owned(),
        device_model: None,
        device_type: None,
        download: None,
        port: local_send::DEFAULT_PORT,
        protocol: protocol.to_owned(),
        address: None,
    }
}

fn receive(browser: &mut FileBrowser, device: DeviceInfo, text: &str) -> Task<Message> {
    browser.handle_transfer_message(TransferMessage::ServiceEvent(
        ServiceEvent::MessageReceived {
            device,
            text: text.to_owned(),
        },
    ))
}

#[test]
fn messages_queue_up_and_closing_one_shows_the_next() {
    let mut browser = browser();
    drop(receive(&mut browser, sender("A", "https"), "first"));
    drop(receive(&mut browser, sender("B", "https"), "second"));

    let texts: Vec<_> = browser
        .incoming_messages
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(texts, ["first", "second"]);

    drop(browser.handle_transfer_message(TransferMessage::IncomingMessageClosed));
    assert_eq!(browser.incoming_messages.front().unwrap().text, "second");
    // Esc / 点外面：同样关掉当前这条。
    drop(browser.dismiss_floating());
    assert!(browser.incoming_messages.is_empty());
}

#[test]
fn a_favorite_is_named_by_its_favorite_name_but_a_plain_device_never_is() {
    let mut browser = browser();
    browser.user_config.transfer_favorite_devices = vec![FavoriteTransferDevice {
        fingerprint: "F".to_owned(),
        alias: "Living room".to_owned(),
        custom_alias: true,
        address: None,
        added_at: String::new(),
    }];

    drop(receive(&mut browser, sender("F", "https"), "hi"));
    // 明文设备自称同一指纹：认不了，用它自报的名字。
    drop(receive(&mut browser, sender("F", "http"), "hi"));

    let names: Vec<_> = browser
        .incoming_messages
        .iter()
        .map(|message| message.title.clone())
        .collect();
    assert_eq!(
        names,
        [
            MessageTitle::Received("Living room".to_owned()),
            MessageTitle::Received("Phone".to_owned())
        ]
    );
}

#[test]
fn copying_marks_the_shown_message() {
    let mut browser = browser();
    drop(receive(&mut browser, sender("A", "https"), "first"));
    drop(receive(&mut browser, sender("B", "https"), "second"));

    let copy = browser.handle_transfer_message(TransferMessage::IncomingMessageCopied);

    assert_eq!(copy.units(), 1);
    let copied: Vec<_> = browser
        .incoming_messages
        .iter()
        .map(|message| message.copied)
        .collect();
    assert_eq!(copied, [true, false]);
}

#[test]
fn only_a_whole_http_link_gets_an_open_button() {
    assert_eq!(
        message_link("  https://meet.example.com/x?a=1 \n"),
        Some("https://meet.example.com/x?a=1")
    );
    assert_eq!(
        message_link("http://192.168.1.5:8080"),
        Some("http://192.168.1.5:8080")
    );
    assert_eq!(message_link("see https://example.com"), None);
    assert_eq!(message_link("file:///etc/passwd"), None);
    assert_eq!(message_link("smb://nas/share"), None);
    assert_eq!(message_link("example.com"), None);
}

#[test]
fn a_notification_is_sent_only_while_bennu_is_in_the_background() {
    let mut browser = browser();
    assert!(browser.system_focused_window.is_none());

    // 后台：弹窗（主窗口没有要清理的输入，空任务）+ 一条通知。
    assert_eq!(receive(&mut browser, sender("A", "https"), "hi").units(), 1);

    browser.system_focused_window = Some(browser.main_window);
    drop(browser.handle_transfer_message(TransferMessage::IncomingMessageClosed));
    assert_eq!(receive(&mut browser, sender("A", "https"), "hi").units(), 0);
}
