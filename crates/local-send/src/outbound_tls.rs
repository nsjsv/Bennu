//! 出站 TLS：对方是谁由它的服务端证书决定，和接收端按客户端证书认人是同一条规则。
//!
//! 照新版官方 App `packages/core/src/http/client/server_cert_verifier.rs`：证书检查与
//! 接收端共用 `identity::check_peer_certificate`；发文件时把目标指纹钉在握手上，对不上
//! 在发出任何请求字节前就断开，对方连文件名都看不到。握手签名照常校验：证书是公开的，
//! 不验签名谁都能拿别人的证书冒充（reqwest 的 `danger_accept_invalid_certs` 连签名都
//! 不验，所以不用它）。

use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::error::LocalSendError;
use crate::identity::{check_peer_certificate, fingerprint_of, DeviceIdentity};

/// 出站 TLS 配置：本机身份证书作客户端证书（新版官方 App 强制要求），服务启动时
/// 建一次；每个客户端只换证书检查（钉不钉指纹）。
#[derive(Clone)]
pub(crate) struct OutboundTls {
    config: rustls::ClientConfig,
    provider: Arc<CryptoProvider>,
}

impl OutboundTls {
    /// 身份证书当不了客户端证书时返回 rustls 的原因。
    pub(crate) fn new(identity: &DeviceIdentity) -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PeerCertificateVerifier {
                provider: Arc::clone(&provider),
                pinned: None,
            }))
            .with_client_auth_cert(vec![identity.certificate().clone()], identity.private_key())?;
        Ok(Self { config, provider })
    }

    /// `pinned` = 要发给的那台设备的证书指纹；None 只用于发现（还不知道对方是谁，
    /// 身份从回应的握手证书读，见 [`certified_fingerprint`]）。
    pub(crate) fn client(&self, pinned: Option<&str>) -> Result<reqwest::Client, LocalSendError> {
        let mut config = self.config.clone();
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(PeerCertificateVerifier {
                provider: Arc::clone(&self.provider),
                pinned: pinned.map(str::to_owned),
            }));
        reqwest::Client::builder()
            .use_preconfigured_tls(config)
            .tls_info(true)
            // 局域网内握手超过 5s 视为对端不可达；总时长不设限（大文件直推）。
            .connect_timeout(Duration::from_secs(5))
            // 对端都在局域网：系统代理会把 TLS 握手带到代理上（官方 1.18.2 同样不走代理）。
            .no_proxy()
            // 对端回的跳转不跟：只和发现的那台设备说话（官方 1.18.2 安全修复）。
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|source| LocalSendError::Request {
                endpoint: "client",
                source,
            })
    }
}

/// 回应所走连接的对方证书指纹；明文 HTTP 没有证书，返回 None。
pub(crate) fn certified_fingerprint(response: &reqwest::Response) -> Option<String> {
    let tls = response.extensions().get::<reqwest::tls::TlsInfo>()?;
    tls.peer_certificate().map(fingerprint_of)
}

#[derive(Debug)]
struct PeerCertificateVerifier {
    provider: Arc<CryptoProvider>,
    pinned: Option<String>,
}

/// 钉住的指纹对不上：作为 rustls 错误的来源带出去，失败原因里看得到是证书的事。
struct NotTheChosenDevice;

impl std::fmt::Display for NotTheChosenDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the device's certificate does not belong to the device you chose")
    }
}

/// rustls 用 `Debug` 显示 `CertificateError::Other`：派生的 Debug 只会写出类型名。
impl std::fmt::Debug for NotTheChosenDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for NotTheChosenDevice {}

impl ServerCertVerifier for PeerCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        check_peer_certificate(end_entity)?;
        if let Some(pinned) = &self.pinned {
            if fingerprint_of(end_entity) != *pinned {
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                        NotTheChosenDevice,
                    ))),
                ));
            }
        }
        Ok(ServerCertVerified::assertion())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn new_identity() -> DeviceIdentity {
        let dir = tempfile::tempdir().unwrap();
        DeviceIdentity::load_or_create(&dir.path().join("identity.pem")).unwrap()
    }

    /// 只接一个连接的 TLS 服务端：出示 `certificate`、用 `signing_key` 签握手，然后回
    /// 一份 info。证书与私钥可以故意不配对：rustls 的 `CertifiedKey::new` 不检查。
    async fn serve_one_info(
        certificate: CertificateDer<'static>,
        signing_key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> u16 {
        #[derive(Debug)]
        struct Fixed(Arc<rustls::sign::CertifiedKey>);
        impl rustls::server::ResolvesServerCert for Fixed {
            fn resolve(
                &self,
                _: rustls::server::ClientHello<'_>,
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                Some(Arc::clone(&self.0))
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let signing = provider.key_provider.load_private_key(signing_key).unwrap();
        let certified = rustls::sign::CertifiedKey::new(vec![certificate], signing);
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(Arc::new(certified))));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let Ok(mut stream) = acceptor.accept(stream).await else {
                return;
            };
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let body = r#"{"alias":"Phone","version":"2.0","fingerprint":"ANY"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        port
    }

    async fn probe(port: u16) -> Result<crate::DeviceInfo, crate::discovery::ProbeError> {
        let discovery =
            crate::discovery::tests::discovery(port, crate::discovery::Presence::Hidden);
        crate::discovery::probe_target(
            &discovery,
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            "https",
            Duration::from_secs(5),
        )
        .await
    }

    #[tokio::test]
    async fn a_device_cannot_pass_as_another_by_showing_its_public_certificate() {
        let phone = new_identity();
        let impostor = new_identity();

        // 对照：证书配自己的私钥，认出来的就是这台手机。
        let genuine = serve_one_info(phone.certificate().clone(), phone.private_key()).await;
        assert_eq!(
            probe(genuine).await.unwrap().fingerprint,
            phone.fingerprint()
        );

        // 拿着手机的证书、却只有自己的私钥：握手签名验不过。
        let forged = serve_one_info(phone.certificate().clone(), impostor.private_key()).await;
        assert!(
            matches!(
                probe(forged).await,
                Err(crate::discovery::ProbeError::Unreachable(_))
            ),
            "a certificate without its private key must not identify the phone"
        );
    }

    #[tokio::test]
    async fn clients_do_not_follow_redirects_sent_by_peers() {
        let followed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hit = Arc::clone(&followed);
        let router = axum::Router::new()
            .route(
                "/start",
                axum::routing::get(|| async { axum::response::Redirect::temporary("/elsewhere") }),
            )
            .route(
                "/elsewhere",
                axum::routing::get(move || async move {
                    hit.store(true, std::sync::atomic::Ordering::SeqCst);
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let identity = new_identity();

        let response = OutboundTls::new(&identity)
            .unwrap()
            .client(None)
            .unwrap()
            .get(format!("http://127.0.0.1:{port}/start"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 307);
        assert!(!followed.load(std::sync::atomic::Ordering::SeqCst));
    }
}
