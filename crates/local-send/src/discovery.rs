//! 发现：UDP 组播 announce 与回应、legacy HTTP 扫描、设备表老化。
//!
//! 组播组 `224.0.0.167`，端口与接收端口相同（默认 53317）；对端收到
//! `announce:true` 后优先用 HTTP `POST /api/localsend/v2/register` 回应，失败退化为
//! UDP 回 `announce:false`。本机是否露面（announce、回应、扫描时登记自己）由
//! [`Presence`] 决定：接收关着时只看不露面。
//!
//! 设备表只收连上过的设备，开了加密的按握手证书认人（同官方最新版）：组播报文是
//! 谁都能发的明文，只当线索，按它报的地址去连一下才登记。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Mutex};

use crate::device::{Announce, DeviceInfo};
use crate::error::LocalSendError;
use crate::{Favorite, FindDeviceError, ServiceEvent};

mod sweep;

/// LocalSend v2 固定组播地址。
pub(crate) const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);

/// 设备表老化阈值（N5：~30s 未刷新剔除）。
pub(crate) const DEVICE_LIFETIME: Duration = Duration::from_secs(30);
/// announce 周期（N5：~5s）。
pub(crate) const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// 听到广播后连对方的超时：局域网单跳请求，超过即退化 UDP。
const ANNOUNCE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// 用户按地址找设备时的超时：人在等，但对方可能在别的网段或隧道另一头。
const FIND_TIMEOUT: Duration = Duration::from_secs(5);
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
///
/// 本机永远不进表，在这里统一拦：本机的出站请求连到本机接收端时（"按 IP 添加"填了
/// 本机 IP、扫描经别的地址绕回本机），register 的证书与自报都对得上，入口处看不出
/// 这是自己。
pub(crate) struct DeviceTable {
    own_fingerprint: String,
    entries: HashMap<String, DeviceEntry>,
}

impl DeviceTable {
    pub(crate) fn new(own_fingerprint: String) -> Self {
        Self {
            own_fingerprint,
            entries: HashMap::new(),
        }
    }

    /// 按 fingerprint 插入或刷新；返回 `Some(device)` 表示首次发现或信息变化（需发 `DeviceSeen`）。
    pub(crate) fn upsert(
        &mut self,
        device: DeviceInfo,
        address: IpAddr,
        now: Instant,
    ) -> Option<DeviceInfo> {
        if device.fingerprint == self.own_fingerprint {
            return None;
        }
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

    /// 连不上 `announce:true` 的来源时改用 UDP 回 `announce:false`（协议允许）；
    /// 不露面时不回应。
    async fn reply_by_udp(&self, socket: &UdpSocket, source: SocketAddr) {
        let Presence::Visible(own_device) = self else {
            return;
        };
        if let Ok(payload) = encode_announce(own_device, false) {
            if let Err(error) = socket.send_to(&payload, source).await {
                tracing::debug!(event = "localsend_udp_reply_failed", %source, %error, "udp announce reply failed");
            }
        }
    }

    /// 连一台设备：露面时 POST register（顺带让对方认识本机，也是对 announce 的
    /// 回应），不露面时只 GET info。
    async fn probe(
        &self,
        client: &reqwest::Client,
        base: &str,
        timeout: Duration,
    ) -> reqwest::Result<reqwest::Response> {
        match self {
            Presence::Visible(own_device) => {
                post_register(
                    client,
                    &format!("{base}{REGISTER_PATH}"),
                    own_device,
                    timeout,
                )
                .await
            }
            Presence::Hidden => {
                client
                    .get(format!("{base}{INFO_PATH}"))
                    .timeout(timeout)
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
    /// 收藏快照：扫描按记住的地址直接连。
    pub(crate) favorites: watch::Receiver<Vec<Favorite>>,
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

    let socket = Arc::new(socket);
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
        // 各连各的：一台连不上的设备不能拖住后面的报文。
        tokio::spawn(accept_announce(
            Arc::clone(&discovery),
            announce,
            source,
            Arc::clone(&socket),
        ));
    }
}

/// 按报文里的端口和协议连一下来源 IP，连上才登记。露面时这一连就是 register，也就是
/// 对 `announce:true` 的回应；连不上时协议允许改用 UDP 回 `announce:false`。
async fn accept_announce(
    discovery: Arc<Discovery>,
    announce: Announce,
    source: SocketAddr,
    socket: Arc<UdpSocket>,
) {
    let address = SocketAddr::new(source.ip(), announce.device.port);
    let scheme = if announce.device.protocol == "https" {
        "https"
    } else {
        "http"
    };
    match probe_target(&discovery, address, scheme, ANNOUNCE_PROBE_TIMEOUT).await {
        Ok(device) => {
            record_device(&discovery.devices, &discovery.events, device, source.ip()).await;
        }
        Err(ProbeError::Unreachable(reason)) if announce.announce == Some(true) => {
            tracing::debug!(event = "localsend_register_http_failed", %address, %reason, "register via http failed, falling back to udp");
            discovery.presence.reply_by_udp(&socket, source).await;
        }
        Err(_) => {}
    }
}

/// 探测失败的原因：按 IP 添加要把它告诉用户，扫描直接丢弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeError {
    Unreachable(String),
    /// 连上的是本机（扫到自己的另一个接口，或用户填了本机地址）。
    ThisDevice,
}

/// 探测一个地址：200 响应体 = 对端 DeviceInfo。对方是谁：开了加密看握手证书（响应体里
/// 自报的不作数），明文 HTTP 只能信自报的。对端响应可能省略 port/protocol，反序列化会
/// 填默认值：以实际连上的端口与 scheme 为准。
pub(crate) async fn probe_target(
    discovery: &Discovery,
    address: SocketAddr,
    scheme: &'static str,
    timeout: Duration,
) -> Result<DeviceInfo, ProbeError> {
    let unreachable =
        |error: reqwest::Error| ProbeError::Unreachable(crate::error::error_chain_text(&error));
    let base = format!("{scheme}://{address}");
    let response = discovery
        .presence
        .probe(&discovery.client, &base, timeout)
        .await
        .map_err(unreachable)?;
    if !response.status().is_success() {
        return Err(ProbeError::Unreachable(format!(
            "HTTP {}",
            response.status()
        )));
    }
    let certified = crate::outbound_tls::certified_fingerprint(&response);
    let mut device = response.json::<DeviceInfo>().await.map_err(unreachable)?;
    if scheme == "https" {
        device.fingerprint = certified.ok_or_else(|| {
            ProbeError::Unreachable("the device did not present a certificate".to_owned())
        })?;
    }
    if device.fingerprint == discovery.own_fingerprint {
        return Err(ProbeError::ThisDevice);
    }
    device.protocol = scheme.to_owned();
    device.port = address.port();
    Ok(device)
}

/// 按地址找设备：先按 HTTPS 认证书；不通再试明文，只为告诉用户"对方没开加密"。
/// 找到就登记，和扫描到的一样马上出现在列表里。
pub(crate) async fn find_device(
    discovery: &Discovery,
    address: SocketAddr,
) -> Result<DeviceInfo, FindDeviceError> {
    match probe_target(discovery, address, "https", FIND_TIMEOUT).await {
        Ok(mut device) => {
            record_device(
                &discovery.devices,
                &discovery.events,
                device.clone(),
                address.ip(),
            )
            .await;
            device.address = Some(address.ip());
            Ok(device)
        }
        Err(ProbeError::ThisDevice) => Err(FindDeviceError::ThisDevice),
        Err(ProbeError::Unreachable(reason)) => {
            match probe_target(discovery, address, "http", FIND_TIMEOUT).await {
                Ok(_) => Err(FindDeviceError::NotEncrypted),
                Err(_) => Err(FindDeviceError::Unreachable(reason)),
            }
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

/// register 出站请求：回应 announce 与扫描共用，请求体就是本机设备信息。
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
pub(crate) mod tests;
