//! 接收端 TCP 监听：绑定、对端地址、端口何时放掉与优雅关停。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};

use super::{router, ReceiveState};
use crate::{ReceiverFailure, ServiceEvent};

/// 接收端何时放掉端口：完成（Ok，或发送端已 drop）即端口已关闭。
pub(crate) type PortReleased = oneshot::Receiver<()>;

/// 没有占着端口：首次启动，或服务没有接收端。
pub(crate) fn port_already_released() -> PortReleased {
    oneshot::channel().1
}

/// 对端地址。axum 只为 `TcpListener` 实现了 `SocketAddr` 的 `Connected`，孤儿规则
/// 不允许为本 crate 的 listener 补那个实现，所以用本 crate 的类型承载。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerAddr(pub(crate) SocketAddr);

impl Connected<IncomingStream<'_, ReceiverListener>> for PeerAddr {
    fn connect_info(stream: IncomingStream<'_, ReceiverListener>) -> Self {
        PeerAddr(*stream.remote_addr())
    }
}

/// 接收端监听套接字。axum 优雅关停时先 drop listener 再等进行中的连接，所以
/// 端口立刻放掉、进行中的传输照样传完；等端口的人只需要知道 listener 何时 drop。
pub(crate) struct ReceiverListener {
    socket: TcpListener,
    // 字段按声明顺序 drop：先关套接字，再 drop 发送端唤醒等端口的新接收端。
    _released: oneshot::Sender<()>,
}

impl Listener for ReceiverListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        Listener::accept(&mut self.socket).await
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.socket.local_addr()
    }
}

/// 启动接收端：先等旧接收端放掉端口（同端口重启不等必然冲突），绑定失败发一次
/// `ReceiverFailed` 后不重试；`shutdown` 置位即优雅关停。返回本接收端的端口释放信号。
pub(crate) fn spawn_server(
    state: Arc<ReceiveState>,
    port: u16,
    previous_released: PortReleased,
    mut shutdown: watch::Receiver<bool>,
) -> PortReleased {
    let (released_tx, released_rx) = oneshot::channel();
    let events = state.events.clone();
    let router = router(state);
    tokio::spawn(async move {
        let _ = previous_released.await;
        let socket = match bind(port).await {
            Ok(socket) => socket,
            Err(failure) => {
                tracing::warn!(
                    event = "localsend_tcp_bind_failed",
                    ?failure,
                    "tcp bind failed"
                );
                let _ = events.send(ServiceEvent::ReceiverFailed(failure));
                return;
            }
        };
        let listener = ReceiverListener {
            socket,
            _released: released_tx,
        };
        let serve = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<PeerAddr>(),
        )
        .with_graceful_shutdown(async move {
            // wait_for 而非 changed：关停信号可能在绑定完成前就已发出。
            let _ = shutdown.wait_for(|stop| *stop).await;
        });
        // serve 只在监听套接字出错时返回 Err；之后再也收不到文件，必须让界面知道。
        if let Err(error) = serve.await {
            tracing::error!(event = "localsend_tcp_serve_failed", %error, port, "tcp receive service stopped");
            let _ = events.send(ServiceEvent::ReceiverFailed(ReceiverFailure::Stopped {
                port,
                reason: error.to_string(),
            }));
        }
    });
    released_rx
}

async fn bind(port: u16) -> Result<TcpListener, ReceiverFailure> {
    TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse => ReceiverFailure::PortInUse { port },
            _ => ReceiverFailure::BindFailed {
                port,
                reason: error.to_string(),
            },
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::{broadcast, Mutex};

    use super::*;
    use crate::device::DeviceInfo;
    use crate::discovery::DeviceTable;

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    struct Receiver {
        events: broadcast::Receiver<ServiceEvent>,
        shutdown: watch::Sender<bool>,
        released: PortReleased,
        _dir: tempfile::TempDir,
    }

    fn own_device(alias: &str) -> DeviceInfo {
        DeviceInfo {
            alias: alias.into(),
            version: "2.0".into(),
            fingerprint: "OWN".into(),
            device_model: None,
            device_type: Some("computer".into()),
            download: None,
            port: crate::DEFAULT_PORT,
            protocol: "http".into(),
            address: None,
        }
    }

    fn spawn_receiver(alias: &str, port: u16, previous_released: PortReleased) -> Receiver {
        let dir = tempfile::tempdir().unwrap();
        let (events_tx, events) = broadcast::channel(16);
        let state = Arc::new(ReceiveState::new(
            own_device(alias),
            dir.path().to_path_buf(),
            Arc::new(|_| false),
            Arc::new(Mutex::new(DeviceTable::default())),
            events_tx,
            super::super::CONFIRM_TIMEOUT,
        ));
        let (shutdown, shutdown_rx) = watch::channel(false);
        let released = spawn_server(state, port, previous_released, shutdown_rx);
        Receiver {
            events,
            shutdown,
            released,
            _dir: dir,
        }
    }

    async fn free_port() -> u16 {
        let probe = TcpListener::bind("0.0.0.0:0").await.unwrap();
        probe.local_addr().unwrap().port()
    }

    /// 端口上回应 info 的是名为 `alias` 的接收端，才算它在服务。
    async fn wait_until_serving(port: u16, alias: &str) {
        let url = format!("http://127.0.0.1:{port}/api/localsend/v2/info");
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                if let Ok(response) = reqwest::get(&url).await {
                    if let Ok(device) = response.json::<DeviceInfo>().await {
                        if device.alias == alias {
                            return;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("receiver never started serving");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn occupied_port_reports_port_in_use_and_holds_nothing() {
        let occupant = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = occupant.local_addr().unwrap().port();

        let mut receiver = spawn_receiver("Blocked", port, port_already_released());

        let event = tokio::time::timeout(TEST_TIMEOUT, receiver.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                event,
                ServiceEvent::ReceiverFailed(ReceiverFailure::PortInUse { port: reported })
                    if reported == port
            ),
            "{event:?}"
        );
        // 发送端 drop（Err）同样表示端口已放掉，只断言没有超时。
        let _ = tokio::time::timeout(TEST_TIMEOUT, receiver.released)
            .await
            .expect("a receiver that never bound must not block the next one");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_releases_the_port_for_a_restart_on_the_same_port() {
        let port = free_port().await;
        let first = spawn_receiver("First", port, port_already_released());
        wait_until_serving(port, "First").await;

        first.shutdown.send(true).unwrap();
        let mut second = spawn_receiver("Second", port, first.released);
        wait_until_serving(port, "Second").await;

        assert!(
            matches!(
                second.events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "restart on the same port must not report a failure"
        );
        second.shutdown.send(true).unwrap();
        let _ = tokio::time::timeout(TEST_TIMEOUT, second.released)
            .await
            .expect("shutdown must release the port");
        TcpListener::bind(("0.0.0.0", port))
            .await
            .expect("port is free after shutdown");
    }
}
