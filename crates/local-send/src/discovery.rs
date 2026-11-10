//! 发现：UDP 组播 announce 与回应、legacy HTTP 扫描、设备表老化。
//!
//! 组播组 `224.0.0.167`，端口与接收端口相同（默认 53317）；对端收到
//! `announce:true` 后优先用 HTTP `POST /api/localsend/v2/register` 回应，失败退化为
//! UDP 回 `announce:false`。本机是否露面（announce、回应、扫描时登记自己）由
//! [`Presence`] 决定：接收关着时只看不露面。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Mutex};

use crate::device::{Announce, DeviceInfo};
use crate::error::LocalSendError;
use crate::ServiceEvent;

mod sweep;

/// LocalSend v2 固定组播地址。
pub(crate) const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);

/// 设备表老化阈值（N5：~30s 未刷新剔除）。
pub(crate) const DEVICE_LIFETIME: Duration = Duration::from_secs(30);
/// announce 周期（N5：~5s）。
pub(crate) const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// register 回应的 HTTP 超时：局域网单跳请求，超过即退化 UDP。
const REGISTER_TIMEOUT: Duration = Duration::from_secs(2);
/// 单个扫描请求超时：局域网单跳，超时即视为不存在。
const SWEEP_TIMEOUT: Duration = Duration::from_millis(500);
/// register 路由路径（listener 回应与主动扫描共用）。
const REGISTER_PATH: &str = "/api/localsend/v2/register";
/// info 路由路径：不露面时扫描只查询、不登记。
const INFO_PATH: &str = "/api/localsend/v2/info";

/// 设备表条目：设备信息 + 来源地址（直推目标）+ 最近活跃时刻。
#[derive(Debug, Clone)]
pub(crate) struct DeviceEntry {
    pub device: DeviceInfo,
    pub address: IpAddr,
    pub last_seen: Instant,
}

/// 共享设备表：discovery 监听、receive 的 register 端点与老化任务共同持有。
#[derive(Default)]
pub(crate) struct DeviceTable {
    entries: HashMap<String, DeviceEntry>,
}

impl DeviceTable {
    /// 按 fingerprint 插入或刷新；返回 `Some(device)` 表示首次发现或信息变化（需发 `DeviceSeen`）。
    pub(crate) fn upsert(
        &mut self,
        device: DeviceInfo,
        address: IpAddr,
        now: Instant,
    ) -> Option<DeviceInfo> {
        let changed = match self.entries.get_mut(&device.fingerprint) {
            Some(entry) => {
                let changed = entry.device != device || entry.address != address;
                entry.last_seen = now;
                if changed {
                    entry.device = device.clone();
                    entry.address = address;
                }
                changed
            }
            None => {
                self.entries.insert(
                    device.fingerprint.clone(),
                    DeviceEntry {
                        device: device.clone(),
                        address,
                        last_seen: now,
                    },
                );
                true
            }
        };
        changed.then(|| {
            let mut device = device;
            device.address = Some(address);
            device
        })
    }

    /// 手动刷新：清空设备表并返回被清设备（逐个广播 `DeviceLost`）。
    pub(crate) fn clear(&mut self) -> Vec<DeviceInfo> {
        self.entries
            .drain()
            .map(|(_, entry)| entry.device)
            .collect()
    }

    /// 剔除超时设备，返回被剔除设备（需发 `DeviceLost`）。
    pub(crate) fn prune(&mut self, now: Instant, lifetime: Duration) -> Vec<DeviceInfo> {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.last_seen) > lifetime)
            .map(|(fingerprint, _)| fingerprint.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|fingerprint| self.entries.remove(&fingerprint))
            .map(|entry| entry.device)
            .collect()
    }
}

/// 解析 announce 报文；自发自收（fingerprint 相同）返回 `None`。
pub(crate) fn parse_announce(payload: &[u8], own_fingerprint: &str) -> Option<Announce> {
    let announce: Announce = serde_json::from_slice(payload).ok()?;
    (announce.device.fingerprint != own_fingerprint).then_some(announce)
}

/// 生成 announce 报文 JSON。
pub(crate) fn encode_announce(device: &DeviceInfo, announce: bool) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&Announce {
        device: device.clone(),
        announce: Some(announce),
    })
}

/// 常见虚拟/隧道接口名片段：这些接口上的地址对扫码/直推设备不可达。
const VIRTUAL_INTERFACE_HINTS: [&str; 10] = [
    "docker", "virbr", "br-", "veth", "tun", "tap", "wg", "zt", "vboxnet", "vmnet",
];

/// 判断（接口名, IPv4）是否为局域网候选：排除环回、链路本地（169.254/16）、
/// benchmark（198.18/15，Clash 等 fake-ip TUN 虚拟网卡）、多播/广播/未指定
/// 地址，以及 docker/网桥/veth 等虚拟接口。CGNAT（100.64/10，Tailscale）
/// 保留：对端接入同一隧道时真实可达，用户可在地址列表自行取舍。
pub(crate) fn is_lan_candidate(name: &str, ip: Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified() {
        return false;
    }
    // 链路本地（169.254/16）；benchmark 段（198.18/15，Clash 等 fake-ip TUN
    // 网卡）手写判断，std 的 is_benchmarking 尚未稳定。
    if ip.is_link_local() || (ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19)) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    !VIRTUAL_INTERFACE_HINTS
        .iter()
        .any(|hint| lower.contains(hint))
}

/// 私网地址优先，同组按地址、接口名稳定排序（QR URL 取第一个）。
pub(crate) fn sort_lan_candidates(candidates: &mut [(String, Ipv4Addr)]) {
    candidates.sort_by(|a, b| (!a.1.is_private(), a.1, &a.0).cmp(&(!b.1.is_private(), b.1, &b.0)));
}

/// 枚举局域网候选（接口名, IPv4）：默认路由可能被 TUN/VPN 虚拟网卡接管，
/// 不能按默认路由取 IP，必须过滤虚拟接口与保留网段。
pub(crate) fn lan_candidates() -> Result<Vec<(String, Ipv4Addr)>, LocalSendError> {
    let interfaces = local_ip_address::list_afinet_netifas()
        .map_err(|error| LocalSendError::NoLocalInterface(error.to_string()))?;
    let mut candidates: Vec<(String, Ipv4Addr)> = interfaces
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(v4) if is_lan_candidate(&name, v4) => Some((name, v4)),
            _ => None,
        })
        .collect();
    sort_lan_candidates(&mut candidates);
    candidates.dedup_by(|a, b| a.1 == b.1);
    Ok(candidates)
}

/// 局域网候选地址列表（逐接口组播 announce 用）：多网卡各自可达不同网段。
pub(crate) fn local_ipv4s() -> Result<Vec<Ipv4Addr>, LocalSendError> {
    Ok(lan_candidates()?.into_iter().map(|(_, ip)| ip).collect())
}

/// 本机在网络上的露面方式。接收开着才宣告自己、回应 announce、扫描时登记自己；
/// 关着只看不露面：别人发现不了本机，本机照样能发现别人并直推。依赖露面方式的
/// 三处行为都在这里决定，发现任务里不各自判断。
pub(crate) enum Presence {
    Visible(DeviceInfo),
    Hidden,
}

impl Presence {
    /// announce 循环：只在露面时运行。
    fn spawn_announcer(&self, port: u16, mut shutdown: watch::Receiver<bool>) {
        let Presence::Visible(own_device) = self else {
            return;
        };
        let own_device = own_device.clone();
        tokio::spawn(async move {
            loop {
                if announce_to_all(&own_device, port).await.is_err() {
                    // 单次发送失败（如临时的接口变化）只影响本轮，等待下一轮即可。
                    tracing::debug!(event = "localsend_announce_failed", "announce round failed");
                }
                tokio::select! {
                    _ = tokio::time::sleep(ANNOUNCE_INTERVAL) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// 回应对端的 `announce:true`：露面时登记自己，不露面时不回应。
    async fn answer(
        &self,
        remote: &DeviceInfo,
        source: std::net::SocketAddr,
        client: &reqwest::Client,
        socket: &UdpSocket,
    ) {
        if let Presence::Visible(own_device) = self {
            reply_register(own_device, remote, source, client, socket).await;
        }
    }

    /// 扫描探测：露面时 POST register（顺带让对方认识本机），不露面时只 GET info。
    async fn probe(
        &self,
        client: &reqwest::Client,
        base: &str,
    ) -> reqwest::Result<reqwest::Response> {
        match self {
            Presence::Visible(own_device) => {
                post_register(
                    client,
                    &format!("{base}{REGISTER_PATH}"),
                    own_device,
                    SWEEP_TIMEOUT,
                )
                .await
            }
            Presence::Hidden => {
                client
                    .get(format!("{base}{INFO_PATH}"))
                    .timeout(SWEEP_TIMEOUT)
                    .send()
                    .await
            }
        }
    }
}

/// 发现任务共用的上下文。
pub(crate) struct Discovery {
    /// 本机身份指纹：过滤听到、扫到的自己。
    pub(crate) own_fingerprint: String,
    /// 组播端口，也是扫描探测对端的端口（同官方：端口一致才互相发现）。
    pub(crate) port: u16,
    pub(crate) presence: Presence,
    pub(crate) devices: Arc<Mutex<DeviceTable>>,
    pub(crate) events: broadcast::Sender<ServiceEvent>,
    pub(crate) client: reqwest::Client,
}

/// 驱动全部发现任务：announce 循环（露面时）、组播监听、扫描与设备老化；收到
/// shutdown 后全部退出。
pub(crate) fn spawn(
    discovery: Arc<Discovery>,
    sweeper_notify: Arc<tokio::sync::Notify>,
    shutdown: watch::Receiver<bool>,
) {
    discovery
        .presence
        .spawn_announcer(discovery.port, shutdown.clone());

    // 老化：周期剔除 30s 未刷新的设备并广播 DeviceLost。
    let aging = Arc::clone(&discovery);
    let mut aging_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(ANNOUNCE_INTERVAL) => {}
                _ = aging_shutdown.changed() => return,
            }
            let removed = aging
                .devices
                .lock()
                .await
                .prune(Instant::now(), DEVICE_LIFETIME);
            for device in removed {
                let _ = aging.events.send(ServiceEvent::DeviceLost(device));
            }
        }
    });

    // legacy HTTP 扫描兜底：组播被网络吞掉时仍能发现常驻 LocalSend 设备。
    tokio::spawn(sweep::run_http_sweeper(
        Arc::clone(&discovery),
        sweeper_notify,
        shutdown.clone(),
    ));

    // 组播监听：登记对端 announce，按露面方式决定是否回应。
    let listener = tokio::spawn(run_listener(discovery, shutdown.clone()));
    let mut listener_shutdown = shutdown;
    tokio::spawn(async move {
        let _ = listener_shutdown.changed().await;
        listener.abort();
    });
}

/// 组播监听套接字。同机的官方 LocalSend（1.18 起同样设置复用）也监听这个端口：
/// 双方都开复用才能共存，组播报文两边都收得到。
fn bind_multicast_listener(port: u16) -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.bind(&std::net::SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

/// 听不到组播只影响"发现别人"：本机 announce 走每个接口单独的发送套接字，不受影响；
/// 扫描每轮兜底，用户也无可操作的事，所以只记警告日志、不上报界面。
async fn run_listener(discovery: Arc<Discovery>, shutdown: watch::Receiver<bool>) {
    let socket = match bind_multicast_listener(discovery.port) {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!(event = "localsend_udp_bind_failed", %error, port = discovery.port, "udp bind failed, relying on http sweep");
            return;
        }
    };
    // 逐候选接口加入组播组：默认路由可能被 TUN/VPN 虚拟网卡接管，
    // UNSPECIFIED 会让内核选错出口导致收不到手机的 announce。
    let mut joined = false;
    for (_, ip) in lan_candidates().unwrap_or_default() {
        match socket.join_multicast_v4(MULTICAST_GROUP, ip) {
            Ok(()) => joined = true,
            Err(error) => {
                tracing::debug!(event = "localsend_multicast_join_failed", %ip, %error, "join on interface failed");
            }
        }
    }
    if !joined {
        tracing::warn!(
            event = "localsend_multicast_join_failed",
            "join multicast failed, relying on http sweep"
        );
        return;
    }

    let mut buffer = [0u8; 4096];
    let mut shutdown = shutdown;
    loop {
        let (size, source) = tokio::select! {
            received = socket.recv_from(&mut buffer) => match received {
                Ok(received) => received,
                Err(error) => {
                    tracing::debug!(event = "localsend_udp_recv_failed", %error, "udp recv failed");
                    continue;
                }
            },
            _ = shutdown.changed() => return,
        };
        let Some(announce) = parse_announce(&buffer[..size], &discovery.own_fingerprint) else {
            continue;
        };
        record_device(
            &discovery.devices,
            &discovery.events,
            announce.device.clone(),
            source.ip(),
        )
        .await;
        // 对端请求被发现（announce:true）→ 按露面方式回应。
        if announce.announce == Some(true) {
            discovery
                .presence
                .answer(&announce.device, source, &discovery.client, &socket)
                .await;
        }
    }
}

/// 登记设备并在信息变化时广播 `DeviceSeen`；listener、sweeper 共用。
async fn record_device(
    devices: &Arc<Mutex<DeviceTable>>,
    events: &broadcast::Sender<ServiceEvent>,
    device: DeviceInfo,
    address: IpAddr,
) {
    let changed = devices.lock().await.upsert(device, address, Instant::now());
    if let Some(device) = changed {
        let _ = events.send(ServiceEvent::DeviceSeen(device));
    }
}

/// 向 announce 来源回应自身设备信息；端口取对端 announce 里声明的 HTTP 端口。
async fn reply_register(
    own_device: &DeviceInfo,
    remote: &DeviceInfo,
    source: std::net::SocketAddr,
    client: &reqwest::Client,
    socket: &UdpSocket,
) {
    let Some(url) = remote
        .http_base_url()
        .map(|base| format!("{base}{REGISTER_PATH}"))
    else {
        return;
    };
    let result = post_register(client, &url, own_device, REGISTER_TIMEOUT).await;
    if result.is_ok() {
        return;
    }
    // HTTP 不可达（对端未开 HTTP 或防火墙）：协议允许退化 UDP 回 announce:false。
    tracing::debug!(event = "localsend_register_http_failed", %url, "register via http failed, falling back to udp");
    if let Ok(payload) = encode_announce(own_device, false) {
        if let Err(error) = socket.send_to(&payload, source).await {
            tracing::debug!(event = "localsend_udp_reply_failed", %source, %error, "udp announce reply failed");
        }
    }
}

/// register 出站请求：announce 回应与 legacy 扫描共用，请求体就是本机设备信息。
/// 官方接收端在 TLS 下只接受 fingerprint 与客户端证书哈希一致的登记，所以必须经
/// 服务唯一的出站客户端（出示设备身份）发出。
pub(crate) async fn post_register(
    client: &reqwest::Client,
    url: &str,
    own_device: &DeviceInfo,
    timeout: Duration,
) -> reqwest::Result<reqwest::Response> {
    client
        .post(url)
        .timeout(timeout)
        .json(own_device)
        .send()
        .await
}

/// 对每个局域网候选 IPv4 各发一份组播 announce：不同接口可达不同网段。
async fn announce_to_all(device: &DeviceInfo, port: u16) -> Result<(), LocalSendError> {
    let payload = encode_announce(device, true)?;
    let target = SocketAddrV4::new(MULTICAST_GROUP, port);
    for interface in local_ipv4s()? {
        // 组播出口接口由 IP_MULTICAST_IF 决定，须用 socket2 在创建阶段配置；
        // std/tokio 未暴露该设置。
        let local = std::net::SocketAddr::from((interface, 0));
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.bind(&local.into())?;
        socket.set_multicast_if_v4(&interface)?;
        let _ = socket.set_multicast_loop_v4(false);
        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(std::net::UdpSocket::from(socket))?;
        let _ = socket.send_to(&payload, target).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

    pub(super) fn discovery(port: u16, presence: Presence) -> Discovery {
        Discovery {
            own_fingerprint: "OWN".into(),
            port,
            presence,
            devices: Arc::new(Mutex::new(DeviceTable::default())),
            events: broadcast::channel(16).0,
            // 测试只连回环；环境里的代理变量不能把请求带走。
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    #[tokio::test]
    async fn only_a_visible_presence_answers_announces() {
        let peer = fake_peer().await;
        let mut remote = device("PEER");
        remote.address = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
        remote.port = peer.port;
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let source = socket.local_addr().unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        Presence::Hidden
            .answer(&remote, source, &client, &socket)
            .await;
        assert!(
            peer.hits.lock().is_empty(),
            "hidden must not register itself"
        );

        Presence::Visible(device("OWN"))
            .answer(&remote, source, &client, &socket)
            .await;
        assert_eq!(*peer.hits.lock(), vec!["register"]);
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
}
