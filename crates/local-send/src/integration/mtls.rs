//! 双向 TLS 互通：测试用接收端模仿新版官方 App（Rust 核心）的要求——强制客户端
//! 证书，只接受在有效期内、自签名能验证通过的证书，并按证书 DER 的 SHA-256 识别
//! 对端（官方 `packages/core/src/http/server/common/client_cert_verifier.rs`）。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::MockConnectInfo;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, Mutex};

use super::own_device;
use crate::device::DeviceInfo;
use crate::discovery::DeviceTable;
use crate::identity::DeviceIdentity;
use crate::receive::{router as receive_router, ReceiveState};
use crate::ServiceEvent;

/// 官方的客户端证书校验：强制出示；证书须在有效期内且自签名验证通过；握手签名照常校验。
#[derive(Debug)]
struct OfficialClientCertVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ClientCertVerifier for OfficialClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        use x509_parser::prelude::{FromDer, X509Certificate};
        let rejected = || {
            rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            )
        };
        let (_, certificate) = X509Certificate::from_der(end_entity).map_err(|_| rejected())?;
        if !certificate.validity().is_valid() {
            return Err(rejected());
        }
        certificate.verify_signature(None).map_err(|_| rejected())?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 按官方格式计算证书指纹：DER 的 SHA-256，逐字节大写 hex。
fn official_fingerprint(certificate: &CertificateDer<'_>) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// 每个连接先完成双向 TLS 握手再交给 axum；握手失败（如对端没带证书）的连接
/// 直接丢弃，与官方服务端一样进不了 HTTP 层。
struct MutualTlsListener {
    tcp: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    client_fingerprints: Arc<parking_lot::Mutex<Vec<String>>>,
    // 拒绝原因只在服务端是确定的：客户端看到告警还是「连接未就绪」取决于时序。
    handshake_rejections: mpsc::UnboundedSender<String>,
}

impl axum::serve::Listener for MutualTlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((stream, address)) = self.tcp.accept().await else {
                continue;
            };
            let tls = match self.acceptor.accept(stream).await {
                Ok(tls) => tls,
                Err(error) => {
                    let _ = self.handshake_rejections.send(error.to_string());
                    continue;
                }
            };
            if let Some(certificate) = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|chain| chain.first())
            {
                self.client_fingerprints
                    .lock()
                    .push(official_fingerprint(certificate));
            }
            return (tls, address);
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

fn mutual_tls_acceptor() -> tokio_rustls::TlsAcceptor {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(Arc::new(OfficialClientCertVerifier { provider }))
        .with_single_cert(
            vec![server.cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server.signing_key.serialize_der())),
        )
        .unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

struct MutualTlsReceiver {
    base: String,
    port: u16,
    /// 握手时验证过的客户端证书指纹（官方以它识别发送方）。
    client_fingerprints: Arc<parking_lot::Mutex<Vec<String>>>,
    /// prepare-upload 请求体里对端自报的 fingerprint（信任回调收到的值）。
    claimed_fingerprints: Arc<parking_lot::Mutex<Vec<String>>>,
    handshake_rejections: mpsc::UnboundedReceiver<String>,
    events: broadcast::Receiver<ServiceEvent>,
    dir: tempfile::TempDir,
}

impl MutualTlsReceiver {
    fn target(&self) -> DeviceInfo {
        DeviceInfo {
            address: Some("127.0.0.1".parse().unwrap()),
            port: self.port,
            protocol: "https".into(),
            ..super::remote_device("phone-fp", "Phone")
        }
    }
}

async fn spawn_mutual_tls_receive() -> MutualTlsReceiver {
    let dir = tempfile::tempdir().unwrap();
    let (events_tx, events_rx) = broadcast::channel(128);
    let claimed_fingerprints = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let claimed_by_trust = Arc::clone(&claimed_fingerprints);
    let state = Arc::new(ReceiveState::new(
        own_device(),
        dir.path().to_path_buf(),
        Arc::new(move |device: &DeviceInfo| {
            claimed_by_trust.lock().push(device.fingerprint.clone());
            true
        }),
        Arc::new(Mutex::new(DeviceTable::default())),
        events_tx,
        Duration::from_secs(10),
    ));
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp.local_addr().unwrap().port();
    let client_fingerprints = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (rejections_tx, handshake_rejections) = mpsc::unbounded_channel();
    let listener = MutualTlsListener {
        tcp,
        acceptor: mutual_tls_acceptor(),
        client_fingerprints: Arc::clone(&client_fingerprints),
        handshake_rejections: rejections_tx,
    };
    // axum 只为 TcpListener 实现了 SocketAddr 的 ConnectInfo；对端地址在这里不重要。
    let router =
        receive_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service()).await;
    });
    MutualTlsReceiver {
        base: format!("https://127.0.0.1:{port}"),
        port,
        client_fingerprints,
        claimed_fingerprints,
        handshake_rejections,
        events: events_rx,
        dir,
    }
}

// 对照：证明测试接收端确实强制客户端证书，后面的互通用例才有意义。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_requiring_client_certificates_rejects_anonymous_clients() {
    let mut receiver = spawn_mutual_tls_receive().await;
    let anonymous = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let outcome = anonymous
        .post(format!("{}/api/localsend/v2/register", receiver.base))
        .json(&own_device())
        .send()
        .await;

    assert!(outcome.is_err(), "{outcome:?}");
    let rejection = tokio::time::timeout(super::TEST_TIMEOUT, receiver.handshake_rejections.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejection, "peer sent no certificates");
    assert!(receiver.client_fingerprints.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_send_presents_the_identity_its_fingerprint_claims() {
    let mut receiver = spawn_mutual_tls_receive().await;
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("one.txt");
    std::fs::write(&file, b"first").unwrap();
    let (sender, fingerprint) = crate::send::sender_with_new_identity(source.path());
    let (progress, _) = mpsc::channel(1);

    sender
        .send(&receiver.target(), vec![file], progress)
        .await
        .unwrap();

    super::wait_for(&mut receiver.events, |event| {
        matches!(event, ServiceEvent::ReceiveFinished { .. })
    })
    .await;
    assert_eq!(
        std::fs::read(receiver.dir.path().join("one.txt")).unwrap(),
        b"first"
    );
    assert_eq!(
        *receiver.claimed_fingerprints.lock(),
        vec![fingerprint.clone()]
    );
    let presented = receiver.client_fingerprints.lock().clone();
    assert!(!presented.is_empty());
    assert!(
        presented.iter().all(|presented| *presented == fingerprint),
        "{presented:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_presents_the_identity_its_fingerprint_claims() {
    let mut receiver = spawn_mutual_tls_receive().await;
    let dir = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap();
    let own = crate::own_device_info("TestPC".to_owned(), None, 53317, &identity);

    let response = crate::discovery::post_register(
        &crate::peer_client(&identity).unwrap(),
        &format!("{}/api/localsend/v2/register", receiver.base),
        &own,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert!(response.status().is_success());
    let ServiceEvent::DeviceSeen(device) = super::wait_for(&mut receiver.events, |event| {
        matches!(event, ServiceEvent::DeviceSeen(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(device.fingerprint, identity.fingerprint());
    assert_eq!(
        *receiver.client_fingerprints.lock(),
        vec![identity.fingerprint().to_owned()]
    );
}
