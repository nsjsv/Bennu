//! legacy HTTP 扫描（规范 §3.2）：组播常被 AP 隔离、IGMP snooping 或 TUN 防火墙
//! 吞掉，退化为周期对候选子网逐 IP 探测。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::{is_lan_candidate, lan_candidates, record_device, Discovery};
use crate::device::DeviceInfo;

/// 扫描周期。
const HTTP_SWEEP_INTERVAL: Duration = Duration::from_secs(10);

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

/// legacy HTTP 扫描兜底：周期对候选子网逐 IP 探测；对方 200 响应即完成发现
/// （手机端打开 LocalSend 或常驻 HTTP 服务时均可响应）。`refresh` 被通知
/// （手动刷新）时立即再扫一轮。
pub(super) async fn run_http_sweeper(
    discovery: Arc<Discovery>,
    refresh: Arc<tokio::sync::Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    let own_ips: std::collections::HashSet<Ipv4Addr> = lan_candidates()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, ip)| ip)
        .collect();
    loop {
        let targets = sweep_targets(&own_ips);
        if !targets.is_empty() {
            // 官方 App 默认 HTTPS（自签名），部分设备显式用 HTTP；扫描时
            // 无先验协议，双协议并发探测（幂等登记，重复响应无害）。
            let discovery = &discovery;
            let probes = targets.iter().flat_map(|target| {
                ["https", "http"].map(|scheme| async move {
                    (
                        *target,
                        probe_target(discovery, IpAddr::V4(*target), scheme).await,
                    )
                })
            });
            for (ip, device) in futures_util::future::join_all(probes).await {
                if let Some(device) = device {
                    record_device(
                        &discovery.devices,
                        &discovery.events,
                        device,
                        IpAddr::V4(ip),
                    )
                    .await;
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

/// 探测一个地址：200 响应体 = 对端 DeviceInfo。忽略自身（扫到自己的另一接口）。
/// 对端响应可能省略 port/protocol，反序列化会填默认值：以实际探测成功的端口与
/// scheme 为准。
async fn probe_target(discovery: &Discovery, ip: IpAddr, scheme: &str) -> Option<DeviceInfo> {
    let base = format!("{scheme}://{ip}:{}", discovery.port);
    let response = discovery
        .presence
        .probe(&discovery.client, &base)
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut device = response.json::<DeviceInfo>().await.ok()?;
    if device.fingerprint == discovery.own_fingerprint {
        return None;
    }
    device.protocol = scheme.to_owned();
    device.port = discovery.port;
    Some(device)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{discovery, fake_peer};
    use super::super::Presence;
    use super::*;

    fn localhost() -> IpAddr {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }

    #[tokio::test]
    async fn hidden_probe_only_queries_info() {
        let peer = fake_peer().await;
        let discovery = discovery(peer.port, Presence::Hidden);

        let device = probe_target(&discovery, localhost(), "http").await.unwrap();

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
        let own = DeviceInfo {
            alias: "TestPC".into(),
            version: "2.0".into(),
            fingerprint: "OWN".into(),
            device_model: None,
            device_type: None,
            download: None,
            port: peer.port,
            protocol: "http".into(),
            address: None,
        };
        let discovery = discovery(peer.port, Presence::Visible(own));

        let device = probe_target(&discovery, localhost(), "http").await.unwrap();

        assert_eq!(*peer.hits.lock(), vec!["register"]);
        // 对端响应省略 port/protocol：反序列化默认值（53317/https）不能当真。
        assert_eq!(device.port, peer.port);
        assert_eq!(device.protocol, "http");
    }

    #[tokio::test]
    async fn probe_ignores_its_own_device() {
        let peer = fake_peer().await;
        let mut discovery = discovery(peer.port, Presence::Hidden);
        discovery.own_fingerprint = "PEER".into();

        assert!(probe_target(&discovery, localhost(), "http")
            .await
            .is_none());
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
}
