use std::time::Duration;

use futures_util::stream::{self, BoxStream, Stream, StreamExt};
use gio::glib::{MainContext, SignalHandlerId};
use gio::prelude::ObjectExt;
use tokio::sync::mpsc::{self, UnboundedSender};
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream};

use crate::gio_runtime::run_on_gio_thread;

/// 一次插入或挂载会连发十几个信号；安静这么久才算一次变化。
const CHANGE_SETTLE: Duration = Duration::from_millis(200);
const VOLUME_MONITOR_SIGNALS: [&str; 9] = [
    "volume-added",
    "volume-removed",
    "volume-changed",
    "mount-added",
    "mount-removed",
    "mount-changed",
    "drive-connected",
    "drive-disconnected",
    "drive-changed",
];

/// 设备与挂载（含网络位置）有变化时发一个 `()`，一阵变化只发一次。
///
/// 两路来源缺一不可：GIO VolumeMonitor 覆盖挂载和 gvfs 的卷（含网络位置），
/// 但没装 gvfs 时它看不到未挂载的块设备，那部分只有 UDisks2 的信号能报出来。
/// 任一路连不上只记一条警告，另一路照常工作。
pub fn storage_changes() -> impl Stream<Item = ()> + Send + 'static {
    let changes = stream::select(gio_volume_changes(), udisks_changes());
    stream::unfold(changes, |mut changes| async move {
        changes.next().await?;
        while let Ok(Some(())) = tokio::time::timeout(CHANGE_SETTLE, changes.next()).await {}
        Some(((), changes))
    })
}

fn gio_volume_changes() -> BoxStream<'static, ()> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let connected = run_on_gio_thread(move |monitor| async move {
        // GIO 对象只能在 bennu-gio 线程上用：信号在这里连，也要在这里断，
        // 所以挂一个本地任务等到流被丢弃（发送端关闭）再断开。
        let handlers = connect_volume_monitor(&monitor, &sender);
        MainContext::ref_thread_default().spawn_local(async move {
            sender.closed().await;
            for handler in handlers {
                monitor.disconnect(handler);
            }
        });
    });
    stream::once(connected)
        .filter_map(|connected| async move {
            if let Err(error) = connected {
                tracing::warn!(%error, "storage changes from GIO are unavailable");
            }
            None::<()>
        })
        .chain(stream::poll_fn(move |context| receiver.poll_recv(context)))
        .boxed()
}

fn connect_volume_monitor(
    monitor: &gio::VolumeMonitor,
    sender: &UnboundedSender<()>,
) -> Vec<SignalHandlerId> {
    VOLUME_MONITOR_SIGNALS
        .iter()
        .map(|signal| {
            let sender = sender.clone();
            monitor.connect_local(signal, false, move |_| {
                let _ = sender.send(());
                None
            })
        })
        .collect()
}

fn udisks_changes() -> BoxStream<'static, ()> {
    stream::once(udisks_signal_stream())
        .flat_map(|signals| match signals {
            Ok(signals) => signals.map(|_| ()).boxed(),
            Err(error) => {
                tracing::warn!(%error, "storage changes from UDisks2 are unavailable");
                stream::empty().boxed()
            }
        })
        .boxed()
}

async fn udisks_signal_stream() -> zbus::Result<MessageStream> {
    let connection = Connection::system().await?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender("org.freedesktop.UDisks2")?
        .path_namespace("/org/freedesktop/UDisks2")?
        .build();
    MessageStream::for_match_rule(rule, &connection, None).await
}
