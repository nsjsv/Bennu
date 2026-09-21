//! `org.freedesktop.impl.portal.FileChooser` 后端接口（xdg-desktop-portal
//! 1.22 同步式协议）：portal 预创建 Request 对象并把路径作为 `handle`
//! 传入；后端阻塞至用户操作完成，直接返回 `(response, results)`。

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};
use zbus::fdo;
use zbus::zvariant::{ObjectPath, Value};

use crate::picker_request::{PickerKind, PickerRequestSpec};

/// UI 侧回传的选择结果。
pub(crate) enum PickerResolution {
    Confirmed(Vec<std::path::PathBuf>),
    Cancelled,
}

/// 一次待处理的选取请求（D-Bus → UI）。
pub(crate) struct PickerInvocation {
    /// portal 预创建的 Request 对象路径，同时充当窗口的唯一标识。
    pub(crate) request_path: String,
    pub(crate) spec: PickerRequestSpec,
    pub(crate) reply: oneshot::Sender<PickerResolution>,
}

/// D-Bus 事件到 UI 的通道消息。
pub(crate) enum BridgeEvent {
    Invocation(PickerInvocation),
}

/// 成功响应码（results 携带 uris）；1 = 用户取消。
const RESPONSE_SUCCESS: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;

pub(crate) struct FileChooserInterface {
    bridge: mpsc::Sender<BridgeEvent>,
}

impl FileChooserInterface {
    pub(crate) fn new(bridge: mpsc::Sender<BridgeEvent>) -> Self {
        FileChooserInterface { bridge }
    }

    /// 公共路径：解析 options → 交给 UI → 阻塞等待窗口结果 → 同步返回。
    /// 方法调用本身即为等待期（与 xdg-desktop-portal-gtk 行为一致）。
    async fn begin_invocation(
        &self,
        handle: &ObjectPath<'_>,
        options: &HashMap<String, Value<'_>>,
        kind_seed: PickerKind,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        let invocation = PickerInvocation {
            request_path: handle.to_string(),
            spec: PickerRequestSpec::from_options(kind_seed, options),
            reply: reply_sender,
        };

        self.bridge
            .send(BridgeEvent::Invocation(invocation))
            .await
            .map_err(|_| fdo::Error::Failed("选择窗口不可用".to_string()))?;

        match reply_receiver.await {
            Ok(PickerResolution::Confirmed(paths)) => {
                Ok((RESPONSE_SUCCESS, response_results(&paths)))
            }
            Ok(PickerResolution::Cancelled) | Err(_) => Ok((RESPONSE_CANCELLED, HashMap::new())),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooserInterface {
    /// 选取已有文件（或目录）。选项：multiple / directory / filters /
    /// current_filter / accept_label / current_folder。
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let kind_seed = PickerKind::OpenFile {
            multiple: false,
            directory: false,
        };
        self.begin_invocation(&handle, &options, kind_seed).await
    }

    /// 另存为：选目标目录并输入文件名。选项：current_name / current_folder /
    /// accept_label。
    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let kind_seed = PickerKind::SaveFile { default_name: None };
        self.begin_invocation(&handle, &options, kind_seed).await
    }

    /// 批量保存：本后端不支持，按协议返回显式错误。
    async fn save_files(
        &self,
        _handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        Err(fdo::Error::NotSupported(
            "SaveFiles is not supported by this backend".to_string(),
        ))
    }
}

/// 成功结果载荷：`uris`（`file://` URL 数组，含特殊字符百分号转义）。
fn response_results(paths: &[std::path::PathBuf]) -> HashMap<String, Value<'static>> {
    // 必须从 `Vec<String>` 构造数组：`Array::from(Vec<Value>)` 的元素签名
    // 是 `v`，会产出 `av`；portal 协议要求 `uris` 为 `as`，portal 前端按
    // `as` 解析，签错了调用方拿不到结果。
    let uris: Vec<String> = paths
        .iter()
        .filter_map(|path| path_to_file_uri(path))
        .collect();
    let mut results = HashMap::with_capacity(1);
    results.insert(
        "uris".to_string(),
        Value::new(zbus::zvariant::Array::from(uris)),
    );
    results
}

/// 路径 → `file://` URL。
pub(crate) fn path_to_file_uri(path: &std::path::Path) -> Option<String> {
    url::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_signature_is_string_array() {
        let results = response_results(&[std::path::PathBuf::from("/tmp/x.png")]);
        let Value::Array(array) = results.get("uris").unwrap() else {
            panic!("uris 必须是数组");
        };
        // 协议不变量：portal 前端按 `as` 解析 uris，签成 `av` 会导致
        // 调用方收不到任何结果。
        assert_eq!(array.signature().to_string(), "as");
    }

    #[test]
    fn uri_escapes_special_characters() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new("/tmp/a b&c.png")).as_deref(),
            Some("file:///tmp/a%20b&c.png")
        );
        assert_eq!(
            path_to_file_uri(std::path::Path::new("/tmp/中文 目录")).as_deref(),
            Some("file:///tmp/%E4%B8%AD%E6%96%87%20%E7%9B%AE%E5%BD%95")
        );
    }

    #[test]
    fn relative_path_has_no_uri() {
        assert!(path_to_file_uri(std::path::Path::new("relative/x")).is_none());
    }

    #[test]
    fn response_results_carries_uris_key() {
        let results = response_results(&[std::path::PathBuf::from("/tmp/x.png")]);
        assert_eq!(results.len(), 1);
        let Value::Array(array) = results.get("uris").unwrap() else {
            panic!("uris 必须是数组");
        };
        assert_eq!(array.len(), 1);
    }
}
