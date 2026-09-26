//! UDP 组播发现：announce 周期广播、register 回应与设备表老化。
//!
//! 组播组 `224.0.0.167:53317`（UDP）；对端收到 `announce:true` 后优先用 HTTP
//! `POST /api/localsend/v2/register` 回应，失败退化为 UDP 回 `announce:false`。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Mutex};

use crate::device::{Announce, DeviceInfo};
use crate::error::LocalSendError;
use crate::ServiceEvent;

/// LocalSend v2 固定组播地址与端口（UDP，与 HTTP 的 TCP 53317 互不冲突）。
pub(crate) const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
pub(crate) const MULTICAST_PORT: u16 = 53317;

/// 设备表老化阈值（N5：~30s 未刷新剔除）。
pub(crate) const DEVICE_LIFETIME: Duration = Duration::from_secs(30);
/// announce 周期（N5：~5s）。
pub(crate) const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// register 回应的 HTTP 超时：局域网单跳请求，超过即退化 UDP。
const REGISTER_TIMEOUT: Duration = Duration::from_secs(2);
/// legacy 发现（规范 §3.2）扫描周期：组播常被 AP 隔离、IGMP snooping 或
/// TUN 防火墙吞掉，退化为对候选子网逐 IP 发送 register。
const HTTP_SWEEP_INTERVAL: Duration = Duration::from_secs(10);
/// 单个扫描请求超时：局域网单跳，超时即视为不存在。
const SWEEP_TIMEOUT: Duration = Duration::from_millis(500);
/// register 路由路径（listener 回应与主动扫描共用）。
const REGISTER_PATH: &str = "/api/localsend/v2/register";

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

/// 驱动全部发现任务：announce 循环、组播监听与设备老化；收到 shutdown 后全部退出。
pub(crate) fn spawn(
    device: DeviceInfo,
    devices: Arc<Mutex<DeviceTable>>,
    events: broadcast::Sender<ServiceEvent>,
    client: reqwest::Client,
    sweeper_notify: Arc<tokio::sync::Notify>,
    shutdown: watch::Receiver<bool>,
) {
    // announce：启动立即一次 + 周期重发；逐非环回 IPv4 接口各发一份。
    let announce_device = device.clone();
    let mut announce_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            if announce_to_all(&announce_device).await.is_err() {
                // 单次发送失败（如临时的接口变化）只影响本轮，等待下一轮即可。
                tracing::debug!(event = "localsend_announce_failed", "announce round failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(ANNOUNCE_INTERVAL) => {}
                _ = announce_shutdown.changed() => return,
            }
        }
    });

    // 老化：周期剔除 30s 未刷新的设备并广播 DeviceLost。
    let aging_devices = Arc::clone(&devices);
    let aging_events = events.clone();
    let mut aging_shutdown = shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(ANNOUNCE_INTERVAL) => {}
                _ = aging_shutdown.changed() => return,
            }
            let removed = aging_devices
                .lock()
                .await
                .prune(Instant::now(), DEVICE_LIFETIME);
            for device in removed {
                let _ = aging_events.send(ServiceEvent::DeviceLost(device));
            }
        }
    });

    // legacy HTTP 扫描兜底：组播被网络吞掉时仍能发现常驻 LocalSend 设备。
    let sweeper_shutdown = shutdown.clone();
    tokio::spawn(run_http_sweeper(
        device.clone(),
        Arc::clone(&devices),
        events.clone(),
        client.clone(),
        sweeper_notify,
        sweeper_shutdown,
    ));

    // 组播监听：处理对端 announce 并回应 register。
    let listener = tokio::spawn(run_listener(
        device,
        devices,
        events,
        client,
        shutdown.clone(),
    ));
    let mut listener_shutdown = shutdown;
    tokio::spawn(async move {
        let _ = listener_shutdown.changed().await;
        listener.abort();
    });
}

async fn run_listener(
    own_device: DeviceInfo,
    devices: Arc<Mutex<DeviceTable>>,
    events: broadcast::Sender<ServiceEvent>,
    client: reqwest::Client,
    shutdown: watch::Receiver<bool>,
) {
    let socket = match UdpSocket::bind(("0.0.0.0", MULTICAST_PORT)).await {
        Ok(socket) => socket,
        Err(error) => {
            // N2：UDP 53317 被占（如另一 LocalSend 实例）只报一次错误，不重试。
            tracing::warn!(event = "localsend_udp_bind_failed", %error, "udp 53317 bind failed");
            let _ = events.send(ServiceEvent::ServiceError(format!(
                "UDP 端口 {MULTICAST_PORT} 被占用（可能有另一个 LocalSend 应用正在运行）: {error}"
            )));
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
            "join multicast failed"
        );
        let _ = events.send(ServiceEvent::ServiceError("加入组播组失败".to_string()));
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
        let Some(announce) = parse_announce(&buffer[..size], &own_device.fingerprint) else {
            continue;
        };
        record_device(&devices, &events, announce.device.clone(), source.ip()).await;
        // 对端请求被发现（announce:true）→ HTTP register 回应；失败退化 UDP announce:false。
        if announce.announce == Some(true) {
            reply_register(&own_device, &announce.device, source, &client, &socket).await;
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
    let result = client
        .post(url.clone())
        .timeout(REGISTER_TIMEOUT)
        .json(own_device)
        .send()
        .await;
    if result.is_ok() {
        return;
    }
    // HTTP 不可达（对端未开 HTTP 或防火墙）：协议允许退化 UDP 回 announce:false。
    tracing::debug!(event = "localsend_register_http_failed", %url, "register via http failed, falling back to udp");
    if let Ok(payload) = encode_announce(own_device, false) {
        let _ = socket.send_to(&payload, source).await;
    }
}

/// 对每个局域网候选 IPv4 各发一份组播 announce：不同接口可达不同网段。
async fn announce_to_all(device: &DeviceInfo) -> Result<(), LocalSendError> {
    let payload = encode_announce(device, true)?;
    let target = SocketAddrV4::new(MULTICAST_GROUP, MULTICAST_PORT);
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

/// 展开一个候选接口所在子网的主机地址：只处理 /24..=/30 常规网段（更大
/// 的前缀会地址爆炸，跳过并留给组播），排除网络/广播地址与自身地址。
fn expand_subnet(
    interface_ip: Ipv4Addr,
    netmask: Ipv4Addr,
    own_ips: &std::collections::HashSet<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let prefix = u32::from(netmask).leading_ones();
    if !(24..=30).contains(&prefix) {
        return Vec::new();
    }
    let network = u32::from(interface_ip) & u32::from(netmask);
    let size = 1u32 << (32 - prefix);
    (1..size - 1)
        .map(|host| Ipv4Addr::from(network + host))
        .filter(|candidate| !own_ips.contains(candidate))
        .collect()
}

/// legacy 发现（规范 §3.2）：枚举候选接口子网，展开全部主机地址。
pub(crate) fn sweep_targets(own_ips: &std::collections::HashSet<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let Ok(interfaces) = getifaddrs::getifaddrs() else {
        return Vec::new();
    };
    let mut targets: Vec<Ipv4Addr> = Vec::new();
    for interface in interfaces {
        let Some(IpAddr::V4(v4)) = interface.address.ip_addr() else {
            continue;
        };
        if !is_lan_candidate(&interface.name, v4) {
            continue;
        }
        let Some(netmask) = interface.address.netmask().and_then(|mask| match mask {
            IpAddr::V4(mask) => Some(mask),
            _ => None,
        }) else {
            continue;
        };
        targets.extend(expand_subnet(v4, netmask, own_ips));
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// legacy HTTP 扫描兜底：周期对候选子网逐 IP 发送 register；对方 200 响应
/// 即完成发现（手机端打开 LocalSend 或常驻 HTTP 服务时均可响应）。
/// `refresh` 被通知（手动刷新）时立即再扫一轮。
async fn run_http_sweeper(
    own_device: DeviceInfo,
    devices: Arc<Mutex<DeviceTable>>,
    events: broadcast::Sender<ServiceEvent>,
    client: reqwest::Client,
    refresh: Arc<tokio::sync::Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    let own_ips: std::collections::HashSet<Ipv4Addr> = lan_candidates()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, ip)| ip)
        .collect();
    loop {
        let payload = match serde_json::to_vec(&own_device) {
            Ok(payload) => payload,
            Err(_) => return,
        };
        let targets = sweep_targets(&own_ips);
        if !targets.is_empty() {
            // 官方 App 默认 HTTPS（自签名），部分设备显式用 HTTP；扫描时
            // 无先验协议，双协议并发探测（幂等登记，重复响应无害）。
            let responses = targets.iter().flat_map(|target| {
                ["https", "http"].map(|scheme| {
                    let request = client
                        .post(format!(
                            "{scheme}://{target}:{}{REGISTER_PATH}",
                            own_device.port
                        ))
                        .timeout(SWEEP_TIMEOUT)
                        .body(payload.clone())
                        .send();
                    async move { (*target, scheme, request.await) }
                })
            });
            let results = futures_util::future::join_all(responses).await;
            for (ip, scheme, response) in results {
                let Ok(response) = response else { continue };
                if !response.status().is_success() {
                    continue;
                }
                // 200 响应体 = 对端 DeviceInfo；忽略自身 fingerprint（扫到自己的另一接口）。
                if let Ok(mut device) = response.json::<DeviceInfo>().await {
                    if device.fingerprint != own_device.fingerprint {
                        // 对端响应可能省略 protocol：以实际探测成功的 scheme 为准。
                        device.protocol = scheme.to_owned();
                        record_device(&devices, &events, device, IpAddr::from(ip)).await;
                    }
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(HTTP_SWEEP_INTERVAL) => {}
            _ = refresh.notified() => {}
            _ = shutdown.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(fingerprint: &str) -> DeviceInfo {
        DeviceInfo {
            alias: "A".into(),
            version: "2.0".into(),
            fingerprint: fingerprint.into(),
            device_model: None,
            device_type: None,
            download: None,
            port: 53317,
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

    #[test]
    fn sweep_expands_regular_subnet_excluding_self_and_edges() {
        let own: std::collections::HashSet<Ipv4Addr> = ["192.168.10.11".parse().unwrap()].into();
        let targets = expand_subnet(
            "192.168.10.11".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            &own,
        );
        assert!(targets.contains(&"192.168.10.1".parse().unwrap()));
        assert!(targets.contains(&"192.168.10.254".parse().unwrap()));
        // 网络/广播地址与自身必须排除。
        assert!(!targets.contains(&"192.168.10.0".parse().unwrap()));
        assert!(!targets.contains(&"192.168.10.255".parse().unwrap()));
        assert!(!targets.contains(&"192.168.10.11".parse().unwrap()));
        assert_eq!(targets.len(), 253);
    }

    #[test]
    fn sweep_skips_non_regular_prefixes() {
        let own: std::collections::HashSet<Ipv4Addr> = ["10.1.0.1".parse().unwrap()].into();
        // /16 子网展开会地址爆炸（6.5 万目标），扫描范围明确跳过。
        assert!(expand_subnet(
            "10.1.0.1".parse().unwrap(),
            "255.255.0.0".parse().unwrap(),
            &own,
        )
        .is_empty());
        // /32 无子网可扫。
        assert!(expand_subnet(
            "100.127.113.87".parse().unwrap(),
            "255.255.255.255".parse().unwrap(),
            &own,
        )
        .is_empty());
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
