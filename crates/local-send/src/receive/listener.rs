//! 接收端监听：TCP 绑定、TLS 握手、对端身份、端口何时放掉与优雅关停。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

use super::{router, ReceiveState};
use crate::identity::fingerprint_of;
use crate::{ReceiverFailure, ServiceEvent};

/// 一次 TLS 握手最多等多久：连上 TCP 却不握手的连接到点丢弃。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 握手完成、等 axum 取走的连接数上限。
const HANDSHAKEN_CAPACITY: usize = 32;

/// 接收端何时放掉端口：完成（Ok，或发送端已 drop）即端口已关闭。
pub(crate) type PortReleased = oneshot::Receiver<()>;

/// 没有占着端口：首次启动，或服务没有接收端。
pub(crate) fn port_already_released() -> PortReleased {
    oneshot::channel().1
}

/// 连接的一端：地址 + 握手时验证过的证书指纹。接收端强制客户端证书，每个请求的
/// 对端都有指纹；处理函数只认它，不看请求体里自报的 fingerprint。
#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub(crate) address: SocketAddr,
    pub(crate) fingerprint: String,
}

impl Connected<IncomingStream<'_, ReceiverListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, ReceiverListener>) -> Self {
        stream.remote_addr().clone()
    }
}

type Handshaken = (TlsStream<TcpStream>, Peer);

/// 交给 axum 的监听：只交出握手成功的连接。握手在各自的任务里做，不握手的连接
/// 挡不住别人。axum 优雅关停时先 drop listener 再等进行中的连接，所以端口立刻
/// 放掉、进行中的传输照样传完；等端口的人只需要知道 listener 何时 drop。
pub(crate) struct ReceiverListener {
    // 字段按声明顺序 drop：先关套接字，再 drop 发送端唤醒等端口的新接收端。
    socket: TcpListener,
    _released: oneshot::Sender<()>,
    acceptor: TlsAcceptor,
    own_fingerprint: String,
    handshaken_tx: mpsc::Sender<Handshaken>,
    handshaken: mpsc::Receiver<Handshaken>,
}

impl Listener for ReceiverListener {
    type Io = TlsStream<TcpStream>;
    type Addr = Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                // 自己持有一个发送端，通道不会关。
                Some(handshaken) = self.handshaken.recv() => return handshaken,
                (stream, address) = Listener::accept(&mut self.socket) => {
                    tokio::spawn(handshake(
                        self.acceptor.clone(),
                        stream,
                        address,
                        self.handshaken_tx.clone(),
                    ));
                }
            }
        }
    }

    /// 本端：监听地址 + 本机证书指纹。
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(Peer {
            address: self.socket.local_addr()?,
            fingerprint: self.own_fingerprint.clone(),
        })
    }
}

/// 握手失败（没带证书、证书无效、不是 TLS、超时）只记调试日志：局域网里扫端口的
/// 程序很多，提示用户只会烦人（同官方）。
async fn handshake(
    acceptor: TlsAcceptor,
    stream: TcpStream,
    address: SocketAddr,
    handshaken: mpsc::Sender<Handshaken>,
) {
    let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            tracing::debug!(event = "localsend_tls_handshake_rejected", %address, %error, "tls handshake rejected");
            return;
        }
        Err(_) => {
            tracing::debug!(event = "localsend_tls_handshake_rejected", %address, "tls handshake timed out");
            return;
        }
    };
    // 客户端证书是强制的：握手成功就一定有。
    let Some(certificate) = tls.get_ref().1.peer_certificates().and_then(<[_]>::first) else {
        return;
    };
    let peer = Peer {
        address,
        fingerprint: fingerprint_of(certificate),
    };
    // listener 已 drop（关停中）时发送失败，连接随之丢弃。
    let _ = handshaken.send((tls, peer)).await;
}

/// 启动接收端：先等旧接收端放掉端口（同端口重启不等必然冲突），绑定失败发一次
/// `ReceiverFailed` 后不重试；`shutdown` 置位即优雅关停。返回本接收端的端口释放信号。
pub(crate) fn spawn_server(
    state: Arc<ReceiveState>,
    port: u16,
    tls: Arc<rustls::ServerConfig>,
    previous_released: PortReleased,
    mut shutdown: watch::Receiver<bool>,
) -> PortReleased {
    let (released_tx, released_rx) = oneshot::channel();
    let events = state.events.clone();
    let own_fingerprint = state.own_device.fingerprint.clone();
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
        let (handshaken_tx, handshaken) = mpsc::channel(HANDSHAKEN_CAPACITY);
        let listener = ReceiverListener {
            socket,
            _released: released_tx,
            acceptor: TlsAcceptor::from(tls),
            own_fingerprint,
            handshaken_tx,
            handshaken,
        };
        let serve = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<Peer>(),
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
    use tokio::sync::broadcast;

    use super::*;
    use crate::integration::{free_port, start_receiver, wait_until_serving, TEST_TIMEOUT};

    fn receiver(
        alias: &str,
        port: u16,
        previous_released: PortReleased,
    ) -> crate::integration::ReceiveServer {
        start_receiver(
            alias,
            port,
            previous_released,
            super::super::CONFIRM_TIMEOUT,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn occupied_port_reports_port_in_use_and_holds_nothing() {
        let occupant = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = occupant.local_addr().unwrap().port();

        let mut blocked = receiver("Blocked", port, port_already_released());

        let event = tokio::time::timeout(TEST_TIMEOUT, blocked.events.recv())
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
        let _ = tokio::time::timeout(TEST_TIMEOUT, blocked.released)
            .await
            .expect("a receiver that never bound must not block the next one");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_releases_the_port_for_a_restart_on_the_same_port() {
        let port = free_port().await;
        let first = receiver("First", port, port_already_released());
        wait_until_serving(&first.sender.client, port, "First").await;

        first.shutdown.send(true).unwrap();
        let mut second = receiver("Second", port, first.released);
        wait_until_serving(&second.sender.client, port, "Second").await;

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
