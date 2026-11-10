//! 按地址找设备（"按 IP 添加"）：只认开了加密的设备，身份取自证书。

use std::net::SocketAddr;
use std::time::Duration;

use super::{free_port, spawn_receive, wait_for};
use crate::discovery::tests::{discovery, fake_peer};
use crate::discovery::{find_device, Presence};
use crate::{FindDeviceError, ServiceEvent};

fn localhost(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encrypted_device_is_found_by_its_certificate_and_listed_at_once() {
    let server = spawn_receive(false, Duration::from_secs(10)).await;
    let discovery = discovery(server.port, Presence::Hidden);
    let mut events = discovery.events.subscribe();

    let device = find_device(&discovery, localhost(server.port))
        .await
        .unwrap();

    assert_eq!(device.fingerprint, server.fingerprint);
    assert_eq!(device.address, Some("127.0.0.1".parse().unwrap()));
    assert_eq!(device.port, server.port);
    assert!(device.is_certified());
    // 和扫描到的一样进设备表：列表里马上有它。
    let ServiceEvent::DeviceSeen(listed) = wait_for(&mut events, |event| {
        matches!(event, ServiceEvent::DeviceSeen(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(listed.fingerprint, server.fingerprint);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_http_device_is_reported_as_not_encrypted() {
    let peer = fake_peer().await;
    let discovery = discovery(peer.port, Presence::Hidden);

    let found = find_device(&discovery, localhost(peer.port)).await;

    assert_eq!(found, Err(FindDeviceError::NotEncrypted));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn this_computer_and_silent_addresses_are_told_apart() {
    let server = spawn_receive(false, Duration::from_secs(10)).await;
    let mut own = discovery(server.port, Presence::Hidden);
    own.own_fingerprint = server.fingerprint.clone();

    assert_eq!(
        find_device(&own, localhost(server.port)).await,
        Err(FindDeviceError::ThisDevice)
    );

    let silent = free_port().await;
    let found = find_device(&discovery(silent, Presence::Hidden), localhost(silent)).await;
    assert!(
        matches!(&found, Err(FindDeviceError::Unreachable(reason)) if !reason.is_empty()),
        "{found:?}"
    );
}
