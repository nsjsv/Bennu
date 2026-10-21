//! 本机 LocalSend 设备身份：自签名证书 + 私钥，持久保存在一个 PEM 文件里。
//!
//! 新版官方 App（Rust 核心）对 HTTPS 连接强制要求客户端证书，并以证书 DER 的
//! SHA-256 识别发送方；本机对外 fingerprint 必须等于这张证书的哈希，所以身份
//! 文件是 fingerprint 与客户端证书的唯一来源。

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

use crate::error::LocalSendError;

/// 证书主题与官方 App 一致：对端只按指纹识别设备，名称不携带信息。
const CERTIFICATE_COMMON_NAME: &str = "LocalSend User";

/// 设备身份：PEM（证书 + PKCS#8 私钥）与证书指纹。
///
/// 不实现 `Debug`：PEM 里有私钥，不能被日志或错误文本带出去。
pub(crate) struct DeviceIdentity {
    pem: Vec<u8>,
    fingerprint: String,
}

impl DeviceIdentity {
    /// 读取持久身份；文件不存在时生成并写入。
    ///
    /// 内容不合法或读写失败都返回错误，不覆盖、不重新生成：悄悄换身份会让
    /// 已收藏本机的手机再也认不出它。
    pub(crate) fn load_or_create(path: &Path) -> Result<Self, LocalSendError> {
        let io_error = |source| LocalSendError::Io {
            path: Some(path.to_path_buf()),
            source,
        };
        match std::fs::read(path) {
            Ok(pem) => Self::from_pem(pem).map_err(|reason| LocalSendError::InvalidIdentity {
                path: path.to_path_buf(),
                reason,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let identity =
                    Self::generate().map_err(|error| io_error(std::io::Error::other(error)))?;
                publish(path, &identity.pem).map_err(io_error)?;
                Ok(identity)
            }
            Err(error) => Err(io_error(error)),
        }
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn reqwest_identity(&self) -> Result<reqwest::Identity, LocalSendError> {
        reqwest::Identity::from_pem(&self.pem).map_err(|source| LocalSendError::Request {
            endpoint: "client",
            source,
        })
    }

    fn generate() -> Result<Self, rcgen::Error> {
        // rcgen 默认 ECDSA P-256 SHA-256；默认有效期 1975–4096，与官方一致，免轮换。
        let key_pair = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, CERTIFICATE_COMMON_NAME);
        let certificate = params.self_signed(&key_pair)?;
        Ok(Self {
            fingerprint: fingerprint_of(certificate.der()),
            pem: format!("{}{}", certificate.pem(), key_pair.serialize_pem()).into_bytes(),
        })
    }

    /// 身份文件是外部输入，只在这里校验一次：恰好一张证书、一把私钥，且两者配对。
    fn from_pem(pem: Vec<u8>) -> Result<Self, String> {
        let certificates = CertificateDer::pem_slice_iter(&pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("unreadable certificate: {error}"))?;
        let keys = PrivateKeyDer::pem_slice_iter(&pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("unreadable private key: {error}"))?;
        let ([certificate], [key]) = (certificates.as_slice(), keys.as_slice()) else {
            return Err(format!(
                "expected one certificate and one private key, found {} and {}",
                certificates.len(),
                keys.len()
            ));
        };
        let signing_key = rustls::crypto::ring::default_provider()
            .key_provider
            .load_private_key(key.clone_key())
            .map_err(|error| format!("unsupported private key: {error}"))?;
        rustls::sign::CertifiedKey::new(vec![certificate.clone()], signing_key)
            .keys_match()
            .map_err(|error| format!("private key does not match the certificate: {error}"))?;
        Ok(Self {
            fingerprint: fingerprint_of(certificate),
            pem,
        })
    }
}

/// 证书 DER 的 SHA-256 大写 hex：与官方 `fingerprint_from_cert_der` 同格式，
/// 官方 register 校验时把对端声明的 fingerprint 转大写后逐字比较。
fn fingerprint_of(certificate_der: &[u8]) -> String {
    Sha256::digest(certificate_der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// 先写同目录临时文件（600）再 rename：写到一半失败不会留下半个身份文件。
fn publish(path: &Path, pem: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("pem.tmp");
    let published = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(pem)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if published.is_err() {
        // 清理失败不改变主错误：调用方需要的是写入失败本身的原因。
        let _ = std::fs::remove_file(&temporary);
    }
    published
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// 按官方格式独立重算文件里那张证书的指纹（SHA-256 后逐字节大写 hex）。
    fn certificate_fingerprint_in(path: &Path) -> String {
        let pem = std::fs::read(path).unwrap();
        let certificate = CertificateDer::pem_slice_iter(&pem)
            .next()
            .unwrap()
            .unwrap();
        Sha256::digest(&certificate)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }

    #[test]
    fn missing_file_creates_a_private_identity_that_reloads_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bennu").join("localsend-identity.pem");

        let created = DeviceIdentity::load_or_create(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!path.with_extension("pem.tmp").exists());
        assert_eq!(created.fingerprint(), certificate_fingerprint_in(&path));
        assert_eq!(created.fingerprint().len(), 64);
        let reloaded = DeviceIdentity::load_or_create(&path).unwrap();
        assert_eq!(reloaded.fingerprint(), created.fingerprint());
    }

    #[test]
    fn unreadable_identity_is_rejected_without_being_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("localsend-identity.pem");
        std::fs::write(&path, b"not a pem file").unwrap();

        let error = DeviceIdentity::load_or_create(&path).err().unwrap();

        assert!(
            matches!(&error, LocalSendError::InvalidIdentity { path: rejected, .. } if *rejected == path),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not a pem file");
    }

    #[test]
    fn certificate_with_a_foreign_private_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.pem");
        let second = dir.path().join("second.pem");
        DeviceIdentity::load_or_create(&first).unwrap();
        DeviceIdentity::load_or_create(&second).unwrap();
        let key_start = "-----BEGIN PRIVATE KEY-----";
        let first_pem = std::fs::read_to_string(&first).unwrap();
        let second_pem = std::fs::read_to_string(&second).unwrap();
        let (first_certificate, _) = first_pem.split_once(key_start).unwrap();
        let (_, second_key) = second_pem.split_once(key_start).unwrap();
        let mixed = format!("{first_certificate}{key_start}{second_key}");
        let path = dir.path().join("mixed.pem");
        std::fs::write(&path, &mixed).unwrap();

        let error = DeviceIdentity::load_or_create(&path).err().unwrap();

        assert!(
            matches!(error, LocalSendError::InvalidIdentity { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mixed);
    }
}
