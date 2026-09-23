//! `org.freedesktop.impl.portal.FileChooser` 后端接口（xdg-desktop-portal
//! 1.22 同步式协议）：portal 预创建 Request 对象并把路径作为 `handle`
//! 传入；后端阻塞至用户操作完成，直接返回 `(response, results)`。

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};
use zbus::fdo;
use zbus::zvariant::{ObjectPath, Value};

use crate::picker_request::{FilePattern, FilterRule, PickerKind, PickerRequestSpec};

/// UI 侧回传的选择结果载荷：choices 与激活过滤规则跟 paths 一起回信，
/// 由 D-Bus 层序列化；UI 效果层（SessionEffect）不感知协议细节。
pub(crate) struct ResolutionPayload {
    pub(crate) paths: Vec<std::path::PathBuf>,
    /// (choice id, 选中值)，保持请求里的 choices 顺序。
    pub(crate) choices: Vec<(String, String)>,
    /// 确认时激活的过滤规则；调用方未传 filters = None（响应不带键）。
    pub(crate) current_filter: Option<FilterRule>,
}

/// UI 侧回传的选择结果。
pub(crate) enum PickerResolution {
    Confirmed(ResolutionPayload),
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
        title: &str,
        options: &HashMap<String, Value<'_>>,
        kind_seed: PickerKind,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        let invocation = PickerInvocation {
            request_path: handle.to_string(),
            spec: PickerRequestSpec::from_options(kind_seed, title, options),
            reply: reply_sender,
        };

        self.bridge
            .send(BridgeEvent::Invocation(invocation))
            .await
            .map_err(|_| fdo::Error::Failed("选择窗口不可用".to_string()))?;

        match reply_receiver.await {
            Ok(PickerResolution::Confirmed(payload)) => {
                Ok((RESPONSE_SUCCESS, response_results(&payload)))
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
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let kind_seed = PickerKind::OpenFile {
            multiple: false,
            directory: false,
        };
        self.begin_invocation(&handle, title, &options, kind_seed)
            .await
    }

    /// 另存为：选目标目录并输入文件名。选项：current_name / current_folder /
    /// accept_label / choices。
    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let kind_seed = PickerKind::SaveFile { default_name: None };
        self.begin_invocation(&handle, title, &options, kind_seed)
            .await
    }

    /// 批量保存：选目标目录，确认返回 `目录/每个名字` 的完整路径列表。
    /// 文件名列表（current_names）只读展示，窗口复用目录浏览形态。
    async fn save_files(
        &self,
        handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> fdo::Result<(u32, HashMap<String, Value<'_>>)> {
        let kind_seed = PickerKind::SaveFiles {
            default_names: Vec::new(),
        };
        self.begin_invocation(&handle, title, &options, kind_seed)
            .await
    }
}

/// 成功结果载荷：`uris`（`as`）、`choices`（`a(ss)`，有选择项才带）
/// 与 `current_filter`（`(sa(us))`，调用方传过 filters 才带）。portal
/// 前端忽略未知响应键，新增键前向兼容。
fn response_results(payload: &ResolutionPayload) -> HashMap<String, Value<'static>> {
    // 必须从具体类型构造数组：`Array::from(Vec<Value>)` 的元素签名
    // 是 `v`，会产出 `av`；portal 协议要求 `uris` 为 `as`，签错了
    // 调用方拿不到结果。
    let uris: Vec<String> = payload
        .paths
        .iter()
        .filter_map(|path| path_to_file_uri(path))
        .collect();
    let mut results = HashMap::with_capacity(3);
    results.insert(
        "uris".to_string(),
        Value::new(zbus::zvariant::Array::from(uris)),
    );
    if !payload.choices.is_empty() {
        // (String, String) 实现了 Type（静态签名 "(ss)"）：从具体元组
        // 类型出发产出 `a(ss)`，而不是包 Value 产出 `av`。
        results.insert(
            "choices".to_string(),
            Value::new(zbus::zvariant::Array::from(payload.choices.clone())),
        );
    }
    if let Some(rule) = &payload.current_filter {
        results.insert(
            "current_filter".to_string(),
            Value::new(filter_rule_to_value(rule)),
        );
    }
    results
}

/// FilterRule → D-Bus `(sa(us))`。模式码沿用协议定义：0 = glob，
/// 1 = mime，与解析侧一致。模式数组从 `(u8, String)` 元组出发
/// （静态签名 "(us)"），避免包 Value 产出 `av`。
fn filter_rule_to_value(rule: &FilterRule) -> zbus::zvariant::Structure<'static> {
    let patterns: Vec<(u32, String)> = rule
        .patterns
        .iter()
        .map(|pattern| match pattern {
            FilePattern::Glob(glob) => (0u32, glob.clone()),
            FilePattern::Mime(mime) => (1u32, mime.clone()),
        })
        .collect();
    zbus::zvariant::Structure::from((rule.name.clone(), zbus::zvariant::Array::from(patterns)))
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

    fn payload(
        paths: &[&str],
        choices: Vec<(String, String)>,
        current_filter: Option<FilterRule>,
    ) -> ResolutionPayload {
        ResolutionPayload {
            paths: paths.iter().map(std::path::PathBuf::from).collect(),
            choices,
            current_filter,
        }
    }

    #[test]
    fn uris_signature_is_string_array() {
        let results = response_results(&payload(&["/tmp/x.png"], Vec::new(), None));
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
        let results = response_results(&payload(&["/tmp/x.png"], Vec::new(), None));
        assert_eq!(results.len(), 1);
        let Value::Array(array) = results.get("uris").unwrap() else {
            panic!("uris 必须是数组");
        };
        assert_eq!(array.len(), 1);
    }

    #[test]
    fn choices_key_is_struct_array_with_id_and_value() {
        let results = response_results(&payload(
            &["/tmp/x.png"],
            vec![("fmt".to_string(), "docx".to_string())],
            None,
        ));
        let Value::Array(array) = results.get("choices").unwrap() else {
            panic!("choices 必须是数组");
        };
        // 协议不变量：choices 是 `a(ss)`（id, 选中值），签成 `av` 前端
        // 解析失败。
        assert_eq!(array.signature().to_string(), "a(ss)");
        assert_eq!(array.len(), 1);
    }

    #[test]
    fn current_filter_present_only_when_filters_were_given() {
        // 未传 filters：不带 current_filter 键。
        let without = response_results(&payload(&["/tmp/x"], Vec::new(), None));
        assert!(!without.contains_key("current_filter"));

        // 传了 filters：结构与传入规则一致 (sa(us))，模式码 0 = glob。
        let rule = FilterRule {
            name: "PNG 图片".to_string(),
            patterns: vec![FilePattern::Glob("*.png".to_string())],
        };
        let with = response_results(&payload(&["/tmp/x.png"], Vec::new(), Some(rule)));
        let Value::Structure(structure) = with.get("current_filter").unwrap() else {
            panic!("current_filter 必须是结构体");
        };
        assert_eq!(structure.signature().to_string(), "(sa(us))");
        assert_eq!(structure.fields().len(), 2);
    }
}
