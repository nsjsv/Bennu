//! LocalSend v2 协议设备与文件元数据类型及 serde 往返。
//!
//! 字段类型刻意宽容（String/Option），serde 默认忽略未知字段：
//! 真实设备（不同版本/平台）携带的扩展字段与新枚举值不能让整包解析失败。

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// LocalSend 设备信息（协议 v2 的 `DeviceInfo`）。
///
/// `address` 不参与协议序列化：它由本机发现流程（UDP 源地址或 HTTP 连接地址）回填，
/// 供 `send_to_device` 解析直推目标；app-ui 自建的自身设备信息该字段为 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub alias: String,
    pub version: String,
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// 规范里 `download` 是可选 string（对端下载页路径）；官方 App 实测会发
    /// bool `false`，必须宽松解析（bool/null/缺失都视为无下载页）。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_lenient_download"
    )]
    pub download: Option<String>,
    /// 官方 App 的 info/register 响应省略 port：缺省即协议默认端口。
    #[serde(default = "default_port")]
    pub port: u16,
    /// 官方 App 省略 protocol：缺省按官方默认 HTTPS 假定；扫描流程以实际
    /// 探测成功的 scheme 覆盖。
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
}

/// 官方 App 省略 port 时假定协议默认端口。
fn default_port() -> u16 {
    53317
}

/// 官方 App 省略 protocol 时按官方默认 HTTPS 假定。
fn default_protocol() -> String {
    "https".to_owned()
}

/// 官方 App 对 `download` 字段的类型不守规范（实测发 bool `false`）：
/// string 保留，其余一律视为无下载页。
fn deserialize_lenient_download<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(path)) if !path.is_empty() => Some(path),
        _ => None,
    })
}

impl DeviceInfo {
    /// 供直推/回应使用的 base URL；scheme 按对端 announce 声明的 protocol：
    /// 官方 App 默认 HTTPS（自签名），明文 HTTP 需对端显式声明。
    pub fn http_base_url(&self) -> Option<String> {
        let address = self.address?;
        let scheme = if self.protocol == "https" {
            "https"
        } else {
            "http"
        };
        Some(format!("{scheme}://{address}:{}", self.port))
    }
}

/// announce 报文 = 设备信息 + announce 标志（`#[serde(flatten)]` 保证线上 JSON 结构不变）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Announce {
    #[serde(flatten)]
    pub device: DeviceInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
}

/// prepare-upload 请求体：发送方设备信息 + 文件清单（键为发送方的 fileId）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadRequest {
    pub info: DeviceInfo,
    pub files: std::collections::HashMap<String, FileMetadata>,
}

/// prepare-upload 成功响应：接收方生成的会话 id + 每个 fileId 的上传令牌。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponse {
    pub session_id: String,
    pub files: std::collections::HashMap<String, String>,
}

/// 单个待传文件的元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_device() -> DeviceInfo {
        DeviceInfo {
            alias: "Bennu".into(),
            version: "2.0".into(),
            fingerprint: "fp-1".into(),
            device_model: Some("PC".into()),
            device_type: Some("computer".into()),
            download: None,
            port: 53317,
            protocol: "http".into(),
            address: None,
        }
    }

    // PRD A6 要求的 JSON 往返：协议字段不丢、未知字段被忽略。
    #[test]
    fn device_info_json_roundtrip_preserves_fields() {
        let json = serde_json::to_string(&sample_device()).unwrap();
        let parsed: DeviceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, sample_device());
    }

    // 官方 App 2.2 实测 /info 与 register 响应体：省略 port/protocol，
    // 且 download 违反规范发 bool——宽松解析是互通的前提（PRD A2）。
    #[test]
    fn device_info_parses_official_app_variants() {
        let raw = r#"{"alias":"YM's Phone +","version":"2.2","deviceModel":"OnePlus","deviceType":"mobile","fingerprint":"38FF","download":false}"#;
        let device: DeviceInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(device.port, 53317);
        assert_eq!(device.protocol, "https");
        assert_eq!(device.download, None);

        // 规范形态的 string download 仍按原样解析。
        let with_path: DeviceInfo = serde_json::from_str(
            r#"{"alias":"a","version":"2.2","fingerprint":"f","port":53317,"protocol":"http","download":"/download"}"#,
        )
        .unwrap();
        assert_eq!(with_path.download.as_deref(), Some("/download"));
    }

    #[test]
    fn announce_roundtrip_keeps_flag_and_device() {
        let announce = Announce {
            device: sample_device(),
            announce: Some(true),
        };
        let json = serde_json::to_string(&announce).unwrap();
        let parsed: Announce = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.announce, Some(true));
        assert_eq!(parsed.device, sample_device());
    }

    // 真实设备可能携带本阶段不认识的字段与类型值，解析必须容忍。
    #[test]
    fn unknown_fields_and_missing_optionals_are_tolerated() {
        let parsed: DeviceInfo = serde_json::from_str(
            r#"{"alias":"Pixel","version":"2.1","fingerprint":"abc","port":53317,
                "protocol":"http","deviceType":"phone","extraField":42}"#,
        )
        .unwrap();
        assert_eq!(parsed.alias, "Pixel");
        assert_eq!(parsed.device_type.as_deref(), Some("phone"));
        assert_eq!(parsed.address, None);
    }

    #[test]
    fn http_base_url_requires_discovered_address() {
        let mut device = sample_device();
        assert_eq!(device.http_base_url(), None);
        device.address = Some("192.168.1.9".parse().unwrap());
        assert_eq!(
            device.http_base_url().as_deref(),
            Some("http://192.168.1.9:53317")
        );
    }

    #[test]
    fn prepare_upload_roundtrip() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "0".to_string(),
            FileMetadata {
                id: "0".into(),
                file_name: "a.txt".into(),
                size: 3,
                file_type: Some("text/plain".into()),
                sha256: None,
            },
        );
        let request = PrepareUploadRequest {
            info: sample_device(),
            files,
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: PrepareUploadRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.files["0"].file_name, "a.txt");
        assert_eq!(parsed.info, sample_device());
    }
}
