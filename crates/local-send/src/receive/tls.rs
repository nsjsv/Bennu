//! 接收端 TLS：服务端证书是本机设备身份证书，客户端证书强制出示。
//!
//! 校验规则照新版官方 App（Rust 核心）`packages/core/src/http/server/common/
//! client_cert_verifier.rs`：LocalSend 设备之间没有共同的 CA，只要证书在有效期内、
//! 自签名验证通过就接受；对端是谁由证书指纹决定。

use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::identity::DeviceIdentity;

/// 接收端 TLS 配置；身份证书不能当服务端证书用时返回 rustls 的原因。
pub(crate) fn server_config(
    identity: &DeviceIdentity,
) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(Arc::new(ClientCertificateVerifier { provider }))
        .with_single_cert(vec![identity.certificate().clone()], identity.private_key())?;
    Ok(Arc::new(config))
}

/// 强制出示客户端证书；证书须在有效期内且自签名验证通过；握手签名照常校验。
#[derive(Debug)]
struct ClientCertificateVerifier {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for ClientCertificateVerifier {
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
