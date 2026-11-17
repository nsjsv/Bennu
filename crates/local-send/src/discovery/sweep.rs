//! 主动找设备：连收藏和已知设备的地址、扫候选子网（规范 §3.2 legacy HTTP 扫描）。
//! 组播常被 AP 隔离、IGMP snooping 或 TUN 防火墙吞掉，光等广播找不全。
//!
//! 只在有人看设备列表时找（`watchers` > 0）：没人看时要做的只有接收，露面靠组播
//! announce 与回应就够了；一直扫子网等于每 10 秒一阵 ARP 风暴，在公司网络里像
//! 端口扫描。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};

use super::{
    is_lan_candidate, lan_candidates, probe_target, record_device, DeviceTable, Discovery,
};
use crate::Favorite;

/// 有人看设备列表时多久找一轮：只能直连才找得到的设备靠它刷新，不然 30 秒就被
/// 老化剔除。
const ROUND_INTERVAL: Duration = Duration::from_secs(10);
/// 广播、连完已知地址后等多久再看设备表：开着 LocalSend 的设备收到广播会马上来
/// 登记，有人来了就不用扫子网。
const ANSWER_GRACE: Duration = Duration::from_secs(1);
/// 单个探测超时：局域网单跳，超时即视为不存在。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// 同时开着的探测连接上限：一个 /24 两种协议是 506 个连接，一次全开会占掉常见
/// 默认文件描述符上限（1024）的一半。
const MAX_CONCURRENT_PROBES: usize = 64;

/// 一轮怎么找。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Round {
    /// 定时或刚有窗口打开：先广播、连已知地址，等一会儿设备表还是空的才扫子网。
    Periodic,
    /// 手动刷新：设备表刚被清空，已知地址连完直接扫整个子网，不等有没有人来登记。
    Refresh,
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

/// 已知地址：设备表里现有的设备按它连上时的协议，收藏只会是开了加密的设备，按记住的
/// 地址只试 HTTPS。对方在别的网段（如 Tailscale）时子网扫描够不着，只能靠这些。
fn known_probes(table: &DeviceTable, favorites: &[Favorite]) -> Vec<(SocketAddr, &'static str)> {
    let mut probes: Vec<(SocketAddr, &'static str)> = table.addresses().collect();
    for address in favorites.iter().filter_map(|favorite| favorite.address) {
        if !probes.contains(&(address, "https")) {
            probes.push((address, "https"));
        }
    }
    probes
}

/// 子网里每个地址两种协议都试（扫描时不知道对方开没开加密），已知地址里连过的不再连。
fn subnet_probes(
    subnet: &[Ipv4Addr],
    port: u16,
    known: &[(SocketAddr, &'static str)],
) -> Vec<(SocketAddr, &'static str)> {
    subnet
        .iter()
        .flat_map(|ip| {
            let address = SocketAddr::new(IpAddr::V4(*ip), port);
            [(address, "https"), (address, "http")]
        })
        .filter(|probe| !known.contains(probe))
        .collect()
}

/// 有人看设备列表时每 `ROUND_INTERVAL` 找一轮；看的人变了（开窗口）马上找一轮，
/// 全走了就停。手动刷新（`refresh_requests`）立刻找一轮整的，找完回它的 oneshot。
pub(super) async fn run_device_search(
    discovery: Arc<Discovery>,
    refresh_requests: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
    watchers: watch::Receiver<usize>,
    shutdown: watch::Receiver<bool>,
) {
    let own_ips: Arc<std::collections::HashSet<Ipv4Addr>> = Arc::new(
        lan_candidates()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, ip)| ip)
            .collect(),
    );
    drive_rounds(refresh_requests, watchers, shutdown, move |round| {
        let discovery = Arc::clone(&discovery);
        let own_ips = Arc::clone(&own_ips);
        Box::pin(async move { search_round(&discovery, &own_ips, round).await })
    })
    .await;
}

/// 什么时候找一轮：和怎么找分开，测试里换成假的一轮只看时机。一轮装箱交进来：
/// 泛型 future 进 `tokio::spawn` 会撞上编译器证不出高阶生命周期 `Send` 的老问题。
///
/// 刷新的人等的是在他点了之后才开始的那一轮：正在找的这一轮可能已经错过了他刚清空
/// 的设备表。服务停了，没回的 oneshot 随之放掉，等的人照样会醒。
async fn drive_rounds(
    mut refresh_requests: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
    mut watchers: watch::Receiver<usize>,
    mut shutdown: watch::Receiver<bool>,
    mut search: impl FnMut(Round) -> BoxFuture<'static, ()> + Send,
) {
    let mut waiting_refreshes = Vec::new();
    loop {
        tokio::select! {
            watched = watchers.wait_for(|count| *count > 0) => {
                if watched.is_err() {
                    return;
                }
            }
            _ = shutdown.changed() => return,
        }
        while let Ok(done) = refresh_requests.try_recv() {
            waiting_refreshes.push(done);
        }
        let round = if waiting_refreshes.is_empty() {
            Round::Periodic
        } else {
            Round::Refresh
        };
        search(round).await;
        for done in waiting_refreshes.drain(..) {
            let _ = done.send(());
        }
        tokio::select! {
            _ = tokio::time::sleep(ROUND_INTERVAL) => {}
            request = refresh_requests.recv() => match request {
                Some(done) => waiting_refreshes.push(done),
                None => return,
            },
            changed = watchers.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

async fn search_round(
    discovery: &Discovery,
    own_ips: &std::collections::HashSet<Ipv4Addr>,
    round: Round,
) {
    discovery.presence.announce(discovery.port).await;
    let known = {
        let favorites = discovery.favorites.borrow().clone();
        known_probes(&*discovery.devices.lock().await, &favorites)
    };
    probe_all(discovery, known.clone()).await;
    if round == Round::Periodic {
        tokio::time::sleep(ANSWER_GRACE).await;
        if !discovery.devices.lock().await.is_empty() {
            return;
        }
    }
    let subnet = subnet_probes(&sweep_targets(own_ips), discovery.port, &known);
    probe_all(discovery, subnet).await;
}

/// 最多 `MAX_CONCURRENT_PROBES` 个同时连，走一个补一个；连上一个登记一个，列表里设备
/// 陆续出现。不用 stream 的 map + buffer_unordered：闭包参数里的 `&'static str` 会让
/// 编译器证不出这一轮能进 `tokio::spawn`。
async fn probe_all(discovery: &Discovery, probes: Vec<(SocketAddr, &'static str)>) {
    let mut waiting = probes.into_iter();
    let mut running = FuturesUnordered::new();
    loop {
        while running.len() < MAX_CONCURRENT_PROBES {
            let Some((address, scheme)) = waiting.next() else {
                break;
            };
            running.push(probe_one(discovery, address, scheme));
        }
        if running.next().await.is_none() {
            return;
        }
    }
}

async fn probe_one(discovery: &Discovery, address: SocketAddr, scheme: &'static str) {
    if let Ok(device) = probe_target(discovery, address, scheme, PROBE_TIMEOUT).await {
        record_device(&discovery.devices, &discovery.events, device, address.ip()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn known_devices_keep_their_protocol_and_favorites_are_reached_over_https_only() {
        let mut table = DeviceTable::new("SELF".into());
        let mut listed = crate::discovery::tests::device("LISTED");
        listed.port = 53318;
        table.upsert(
            listed,
            "192.168.1.9".parse().unwrap(),
            std::time::Instant::now(),
        );
        let favorite = |text: &str| Favorite {
            fingerprint: "F".into(),
            address: Some(address(text)),
        };
        let favorites = [
            // 别的网段（如 Tailscale）：子网扫描够不着。
            favorite("100.64.0.7:53317"),
            // 同一 IP 换了端口仍要单独连。
            favorite("192.168.1.5:53318"),
            Favorite {
                fingerprint: "NO-ADDRESS".into(),
                address: None,
            },
        ];

        assert_eq!(
            known_probes(&table, &favorites),
            [
                (address("192.168.1.9:53318"), "http"),
                (address("100.64.0.7:53317"), "https"),
                (address("192.168.1.5:53318"), "https"),
            ]
        );
    }

    #[test]
    fn the_subnet_skips_what_known_addresses_already_tried() {
        let subnet: Vec<Ipv4Addr> = vec!["192.168.1.5".parse().unwrap()];
        let known = [(address("192.168.1.5:53317"), "https")];

        assert_eq!(
            subnet_probes(&subnet, 53317, &known),
            [(address("192.168.1.5:53317"), "http")]
        );
    }

    /// 跑 `drive_rounds`，假的一轮只记下第几秒、哪种。
    struct RoundLog {
        watchers: watch::Sender<usize>,
        refresh_requests: mpsc::UnboundedSender<oneshot::Sender<()>>,
        rounds: Arc<parking_lot::Mutex<Vec<(u64, Round)>>>,
        _shutdown: watch::Sender<bool>,
    }

    fn drive_logged_rounds() -> RoundLog {
        let (watchers, watchers_rx) = watch::channel(0);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (refresh_requests, refresh_rx) = mpsc::unbounded_channel();
        let rounds = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let start = tokio::time::Instant::now();
        let log = Arc::clone(&rounds);
        tokio::spawn(drive_rounds(
            refresh_rx,
            watchers_rx,
            shutdown_rx,
            move |round| {
                log.lock().push((start.elapsed().as_secs(), round));
                Box::pin(async {})
            },
        ));
        RoundLog {
            watchers,
            refresh_requests,
            rounds,
            _shutdown: shutdown,
        }
    }

    async fn wait(seconds: u64) {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn rounds_run_only_while_someone_watches_the_device_list() {
        let log = drive_logged_rounds();
        wait(30).await;
        assert!(log.rounds.lock().is_empty(), "nobody watches: no round");

        log.watchers.send_modify(|count| *count += 1);
        wait(25).await;
        log.watchers.send_modify(|count| *count -= 1);
        wait(60).await;

        assert_eq!(
            *log.rounds.lock(),
            [
                (30, Round::Periodic),
                (40, Round::Periodic),
                (50, Round::Periodic),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refresh_runs_a_full_round_at_once_and_answers_when_it_is_done() {
        let log = drive_logged_rounds();
        log.watchers.send_modify(|count| *count += 1);
        wait(3).await;
        let (done, mut finished) = oneshot::channel();
        log.refresh_requests.send(done).unwrap();
        wait(1).await;

        assert_eq!(
            *log.rounds.lock(),
            [(0, Round::Periodic), (3, Round::Refresh)]
        );
        assert_eq!(finished.try_recv(), Ok(()));
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
