//! 发现：探测认人、广播只当线索、露面规则、设备表老化。

use super::*;

/// 假对端：记录被打到的路由；响应体照官方 App 省略 port/protocol。
pub(super) struct FakePeer {
    pub(super) port: u16,
    pub(super) hits: Arc<parking_lot::Mutex<Vec<&'static str>>>,
}

pub(super) async fn fake_peer() -> FakePeer {
    let hits = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let body = serde_json::json!({
        "alias": "Phone",
        "version": "2.1",
        "fingerprint": "PEER",
        "deviceType": "mobile",
    });
    let (register_hits, register_body) = (Arc::clone(&hits), body.clone());
    let (info_hits, info_body) = (Arc::clone(&hits), body);
    let router = axum::Router::new()
        .route(
            REGISTER_PATH,
            axum::routing::post(move |_: axum::Json<DeviceInfo>| async move {
                register_hits.lock().push("register");
                axum::Json(register_body)
            }),
        )
        .route(
            INFO_PATH,
            axum::routing::get(move || async move {
                info_hits.lock().push("info");
                axum::Json(info_body)
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    FakePeer { port, hits }
}

/// 与服务同样的出站客户端（不钉指纹、出示新身份），只连回环。
pub(crate) fn discovery(port: u16, presence: Presence) -> Discovery {
    let dir = tempfile::tempdir().unwrap();
    let identity =
        crate::identity::DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap();
    Discovery {
        own_fingerprint: "OWN".into(),
        port,
        presence,
        devices: Arc::new(Mutex::new(DeviceTable::default())),
        events: broadcast::channel(16).0,
        client: crate::outbound_tls::OutboundTls::new(&identity)
            .unwrap()
            .client(None)
            .unwrap(),
    }
}

fn localhost(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

fn announce_from(port: u16, announce: bool) -> Announce {
    Announce {
        device: DeviceInfo {
            port,
            ..device("CLAIMED")
        },
        announce: Some(announce),
    }
}

async fn table_fingerprints(discovery: &Discovery) -> Vec<String> {
    discovery
        .devices
        .lock()
        .await
        .clear()
        .into_iter()
        .map(|device| device.fingerprint)
        .collect()
}

#[tokio::test]
async fn hidden_probe_only_queries_info() {
    let peer = fake_peer().await;
    let discovery = discovery(peer.port, Presence::Hidden);

    let device = probe_target(&discovery, localhost(peer.port), "http", TIMEOUT)
        .await
        .unwrap();

    assert_eq!(
        *peer.hits.lock(),
        vec!["info"],
        "hidden must not register itself"
    );
    assert_eq!(device.fingerprint, "PEER");
}

#[tokio::test]
async fn visible_probe_registers_and_records_the_probed_port_and_scheme() {
    let peer = fake_peer().await;
    let discovery = discovery(peer.port, Presence::Visible(device("OWN")));

    let device = probe_target(&discovery, localhost(peer.port), "http", TIMEOUT)
        .await
        .unwrap();

    assert_eq!(*peer.hits.lock(), vec!["register"]);
    // 对端响应省略 port/protocol：反序列化默认值（53317/https）不能当真。
    assert_eq!(device.port, peer.port);
    assert_eq!(device.protocol, "http");
}

#[tokio::test]
async fn probe_reports_its_own_device() {
    let peer = fake_peer().await;
    let mut discovery = discovery(peer.port, Presence::Hidden);
    discovery.own_fingerprint = "PEER".into();

    let probed = probe_target(&discovery, localhost(peer.port), "http", TIMEOUT).await;

    assert_eq!(probed, Err(ProbeError::ThisDevice));
}

#[tokio::test]
async fn an_announce_is_recorded_only_after_reaching_the_device() {
    let peer = fake_peer().await;
    let discovery = Arc::new(discovery(peer.port, Presence::Hidden));
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let source = localhost(socket.local_addr().unwrap().port());

    accept_announce(
        Arc::clone(&discovery),
        announce_from(peer.port, false),
        source,
        Arc::clone(&socket),
    )
    .await;

    // 不露面时只查询；登记的是连上的那台（明文 HTTP 只能信它自报的 PEER）。
    assert_eq!(*peer.hits.lock(), vec!["info"]);
    assert_eq!(table_fingerprints(&discovery).await, ["PEER"]);

    let silent = free_tcp_port().await;
    accept_announce(
        Arc::clone(&discovery),
        announce_from(silent, false),
        source,
        Arc::clone(&socket),
    )
    .await;
    assert!(table_fingerprints(&discovery).await.is_empty());
}

#[tokio::test]
async fn an_unreachable_announcer_gets_a_udp_reply_only_when_visible() {
    let silent = free_tcp_port().await;
    let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let source = listener.local_addr().unwrap();
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());

    accept_announce(
        Arc::new(discovery(silent, Presence::Hidden)),
        announce_from(silent, true),
        source,
        Arc::clone(&socket),
    )
    .await;
    let mut buffer = [0u8; 4096];
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.recv_from(&mut buffer))
            .await
            .is_err(),
        "hidden must not answer"
    );

    accept_announce(
        Arc::new(discovery(silent, Presence::Visible(device("OWN")))),
        announce_from(silent, true),
        source,
        socket,
    )
    .await;
    let (size, _) = tokio::time::timeout(TIMEOUT, listener.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let reply = parse_announce(&buffer[..size], "other").unwrap();
    assert_eq!(reply.device.fingerprint, "OWN");
    assert_eq!(reply.announce, Some(false));
}

const TIMEOUT: Duration = Duration::from_secs(5);

/// 绑定后立即释放：一个确定没人监听的本机端口。
async fn free_tcp_port() -> u16 {
    tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn multicast_listener_shares_its_port_with_another_reusing_socket() {
    let port = std::net::UdpSocket::bind("0.0.0.0:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let first = bind_multicast_listener(port).expect("first bind");
    // 同机的官方 LocalSend 1.18 同样开着复用：第二个监听必须绑得上。
    let second = bind_multicast_listener(port).expect("second bind on the same port");
    assert_eq!(first.local_addr().unwrap().port(), port);
    assert_eq!(second.local_addr().unwrap().port(), port);
}

fn device(fingerprint: &str) -> DeviceInfo {
    DeviceInfo {
        alias: "A".into(),
        version: "2.0".into(),
        fingerprint: fingerprint.into(),
        device_model: None,
        device_type: None,
        download: None,
        port: crate::DEFAULT_PORT,
        protocol: "http".into(),
        address: None,
    }
}

#[test]
fn announce_json_roundtrip() {
    let payload = encode_announce(&device("fp"), true).unwrap();
    let parsed = parse_announce(&payload, "other").unwrap();
    assert_eq!(parsed.device.fingerprint, "fp");
    assert_eq!(parsed.announce, Some(true));
}

#[test]
fn lan_candidates_exclude_virtual_interfaces_and_reserved_ranges() {
    // 真实网卡私网地址是候选。
    assert!(is_lan_candidate("wlan0", "192.168.1.5".parse().unwrap()));
    assert!(is_lan_candidate("enp3s0", "10.0.0.2".parse().unwrap()));
    assert!(is_lan_candidate("eth0", "172.16.0.1".parse().unwrap()));
    // Clash/mihomo fake-ip TUN（benchmark 段 198.18/15）必须排除。
    assert!(!is_lan_candidate("utun5", "198.18.0.1".parse().unwrap()));
    assert!(!is_lan_candidate("wlan0", "198.18.0.1".parse().unwrap()));
    // CGNAT（100.64/10，Tailscale 等）放开：对端接入同一隧道时真实可达，
    // 由用户在地址列表自行取舍；链路本地仍排除。
    assert!(is_lan_candidate(
        "tailscale0",
        "100.64.1.1".parse().unwrap()
    ));
    assert!(!is_lan_candidate("wlan0", "169.254.3.4".parse().unwrap()));
    // 虚拟接口名排除（docker/网桥/veth/隧道）。
    assert!(!is_lan_candidate("docker0", "172.17.0.1".parse().unwrap()));
    assert!(!is_lan_candidate(
        "virbr0",
        "192.168.122.1".parse().unwrap()
    ));
    assert!(!is_lan_candidate(
        "br-600092",
        "172.19.0.1".parse().unwrap()
    ));
    assert!(!is_lan_candidate(
        "veth9c3e8fa",
        "172.17.0.2".parse().unwrap()
    ));
    assert!(!is_lan_candidate("wg0", "10.99.0.1".parse().unwrap()));
    // 环回/多播/未指定排除。
    assert!(!is_lan_candidate("lo", "127.0.0.1".parse().unwrap()));
    assert!(!is_lan_candidate("wlan0", "239.1.1.1".parse().unwrap()));
    assert!(!is_lan_candidate("wlan0", "0.0.0.0".parse().unwrap()));
}

#[test]
fn lan_candidates_sort_private_first() {
    let mut candidates = vec![
        ("eth0".to_owned(), "8.8.8.8".parse().unwrap()),
        ("wlan0".to_owned(), "192.168.1.5".parse().unwrap()),
    ];
    sort_lan_candidates(&mut candidates);
    // 私网候选排前：QR URL 必须取到手机可达的地址。
    assert_eq!(candidates[0].1, "192.168.1.5".parse::<Ipv4Addr>().unwrap());
}

// 自发自收必须丢弃，否则设备表会被自己污染。
#[test]
fn own_announce_is_ignored() {
    let payload = encode_announce(&device("fp"), true).unwrap();
    assert!(parse_announce(&payload, "fp").is_none());
}

#[test]
fn invalid_payload_is_dropped() {
    assert!(parse_announce(b"not json", "fp").is_none());
}

#[test]
fn device_table_dedups_and_reports_changes() {
    let mut table = DeviceTable::default();
    let now = Instant::now();
    let first = device("fp");
    let seen = table.upsert(first.clone(), "10.0.0.2".parse().unwrap(), now);
    assert_eq!(seen.unwrap().address, Some("10.0.0.2".parse().unwrap()));
    // 相同信息重复上报不再触发 DeviceSeen。
    assert!(table
        .upsert(first.clone(), "10.0.0.2".parse().unwrap(), now)
        .is_none());
    // 别名变化视为信息变化。
    let mut renamed = first.clone();
    renamed.alias = "B".into();
    assert!(table
        .upsert(renamed, "10.0.0.2".parse().unwrap(), now)
        .is_some());
}

#[test]
fn device_table_prunes_only_expired_entries() {
    let mut table = DeviceTable::default();
    let now = Instant::now();
    table.upsert(device("old"), "10.0.0.2".parse().unwrap(), now);
    table.upsert(
        device("new"),
        "10.0.0.3".parse().unwrap(),
        now + Duration::from_secs(20),
    );
    let removed = table.prune(
        now + DEVICE_LIFETIME + Duration::from_secs(1),
        DEVICE_LIFETIME,
    );
    // new 在 now+20s 刷新，寿命到 now+50s；old 在 now，寿命到 now+30s。
    assert_eq!(
        removed
            .iter()
            .map(|d| d.fingerprint.as_str())
            .collect::<Vec<_>>(),
        ["old"]
    );
    assert!(table
        .upsert(device("x"), "10.0.0.4".parse().unwrap(), now)
        .is_some());
}
