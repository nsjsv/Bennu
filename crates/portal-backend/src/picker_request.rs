//! FileChooser 请求规格：从 D-Bus `a{sv}` options 解析出选择窗口所需的
//! 全部决策参数。解析失败的单个 option 按协议前向兼容原则忽略，不使整个
//! 请求失败。

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::zvariant::Value;

/// 调用方给出的文件名过滤规则中的一条匹配模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilePattern {
    /// type 0：glob 通配（如 `*.png`），按文件名匹配。
    Glob(String),
    /// type 1：MIME 类型（如 `image/png`、`image/*`）。
    Mime(String),
}

/// 一组具名过滤规则（如"图片"= *.png;*.jpg）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilterRule {
    pub(crate) name: String,
    pub(crate) patterns: Vec<FilePattern>,
}

/// 请求决定的选择模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerKind {
    OpenFile { multiple: bool, directory: bool },
    SaveFile { default_name: Option<String> },
}

impl PickerKind {
    /// 窗口标题缺省值：调用方未提供 title（空串/纯空白归一化为 None）时的兜底。
    pub(crate) fn default_title(&self) -> &'static str {
        match self {
            PickerKind::OpenFile {
                directory: true, ..
            } => "选择文件夹",
            PickerKind::OpenFile { .. } => "选择文件",
            PickerKind::SaveFile { .. } => "另存为",
        }
    }
}

/// 一次 FileChooser 请求解析出的完整规格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerRequestSpec {
    pub(crate) kind: PickerKind,
    pub(crate) accept_label: Option<String>,
    /// 调用方 title 入参；协议里空串等价未提供，归一化为 None 走默认
    /// 标题；纯空白串视同空串处理。仅作窗口标题显示，不参与选择语义。
    pub(crate) title: Option<String>,
    pub(crate) filters: Vec<FilterRule>,
    pub(crate) active_filter: Option<usize>,
    /// 调用方通过 `current_folder` 指定的起始目录。
    pub(crate) start_folder: Option<PathBuf>,
}

impl PickerRequestSpec {
    /// 从 D-Bus options 容器解析。`kind_seed` 由调用来源（OpenFile/SaveFile）
    /// 决定；options 只补充该来源允许的标志。`title` 来自方法入参而非
    /// options（协议的 title 是独立位置参数）。
    pub(crate) fn from_options(
        kind_seed: PickerKind,
        title: &str,
        options: &HashMap<String, Value<'_>>,
    ) -> Self {
        let mut spec = PickerRequestSpec {
            kind: kind_seed,
            accept_label: None,
            title: (!title.trim().is_empty()).then(|| title.to_string()),
            filters: Vec::new(),
            active_filter: None,
            start_folder: None,
        };

        for (key, value) in options {
            match key.as_str() {
                "accept_label" => spec.accept_label = value_as_str(value),
                "handle_token" => {}
                "modal" => {}
                "choices" => {}
                _ => {}
            }
        }

        match &mut spec.kind {
            PickerKind::OpenFile {
                multiple,
                directory,
            } => {
                for (key, value) in options {
                    match key.as_str() {
                        "multiple" => {
                            if let Some(flag) = value_as_bool(value) {
                                *multiple = flag;
                            }
                        }
                        "directory" => {
                            if let Some(flag) = value_as_bool(value) {
                                *directory = flag;
                            }
                        }
                        _ => {}
                    }
                }
            }
            PickerKind::SaveFile { default_name } => {
                for (key, value) in options {
                    if key == "current_name" {
                        if let Some(name) = value_as_str(value) {
                            *default_name = Some(name);
                        }
                    }
                }
            }
        }

        // 顺序敏感：current_filter 的索引解析依赖 filters 先就位，
        // 因此这两项不能跟随 HashMap 的随机迭代顺序。
        if let Some(value) = options.get("filters") {
            spec.filters = parse_filters(value);
        }
        if let Some(value) = options.get("current_filter") {
            spec.active_filter = parse_filters(value)
                .first()
                .and_then(|first| spec.filters.iter().position(|rule| rule == first));
        }
        if let Some(value) = options.get("current_folder") {
            spec.start_folder = value_as_path(value);
        }

        spec
    }
}

/// `s` 型 option 取值；其他类型忽略。
fn value_as_str(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.to_string()),
        _ => None,
    }
}

/// `b` 型 option 取值；其他类型忽略。
fn value_as_bool(value: &Value<'_>) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// `ay`（字节数组）按路径解析；`s` 也接受。路径必须以 `/` 开头才有效，
/// 防止把杂项字符串误当目录。
fn value_as_path(value: &Value<'_>) -> Option<PathBuf> {
    match value {
        Value::Str(text) => {
            let path = PathBuf::from(text.as_str());
            (path.starts_with("/")).then_some(path)
        }
        Value::Array(array) => {
            let bytes: Option<Vec<u8>> = array
                .iter()
                .map(|item| match item {
                    Value::U8(byte) => Some(*byte),
                    _ => None,
                })
                .collect();
            let bytes = bytes?;
            let text = String::from_utf8(bytes).ok()?;
            let path = PathBuf::from(text);
            (path.starts_with("/")).then_some(path)
        }
        _ => None,
    }
}

/// 解析 `a(sa(us))` 形态的过滤器列表；`current_filter` 是其单元素退化
/// `(sa(us))`，同一解析器复用：非数组包裹即视为单条。结构不符时返回空，
/// 由调用方按"无过滤"处理。
/// `a{sv}` 取值与 av 数组都可能把真实值包在 variant 里；解包后再匹配。
fn unwrap_variant<'a>(value: &'a Value<'_>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn parse_filters(value: &Value<'_>) -> Vec<FilterRule> {
    match unwrap_variant(value) {
        Value::Array(array) => array
            .iter()
            .filter_map(|item| match unwrap_variant(item) {
                Value::Structure(structure) => filter_rule_from_structure(structure),
                _ => None,
            })
            .collect::<Vec<_>>(),
        Value::Structure(structure) => filter_rule_from_structure(structure).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn filter_rule_from_structure(structure: &zbus::zvariant::Structure<'_>) -> Option<FilterRule> {
    let fields = structure.fields();
    if fields.len() != 2 {
        return None;
    }
    let name = value_as_str(unwrap_variant(&fields[0]))?;
    let Value::Array(patterns) = unwrap_variant(&fields[1]) else {
        return None;
    };
    let mut parsed = Vec::new();
    for pattern in patterns.iter() {
        let Value::Structure(pattern_structure) = unwrap_variant(pattern) else {
            continue;
        };
        let pattern_fields = pattern_structure.fields();
        if pattern_fields.len() != 2 {
            continue;
        }
        let kind = match unwrap_variant(&pattern_fields[0]) {
            Value::U8(0) => FilePattern::Glob,
            Value::U8(1) => FilePattern::Mime,
            _ => continue,
        };
        let Some(text) = value_as_str(unwrap_variant(&pattern_fields[1])) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        parsed.push(kind(text));
    }
    if parsed.is_empty() {
        return None;
    }
    Some(FilterRule {
        name,
        patterns: parsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Str;

    fn options(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, Value<'static>> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    #[test]
    fn empty_options_yields_defaults() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(Vec::new()),
        );
        assert_eq!(
            spec.kind,
            PickerKind::OpenFile {
                multiple: false,
                directory: false
            }
        );
        assert_eq!(spec.accept_label, None);
        assert_eq!(spec.title, None);
        assert!(spec.filters.is_empty());
        assert_eq!(spec.active_filter, None);
    }

    #[test]
    fn empty_title_normalizes_to_none() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            "",
            &options(Vec::new()),
        );
        assert_eq!(spec.title, None);
    }

    #[test]
    fn whitespace_title_normalizes_to_none() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            " ",
            &options(Vec::new()),
        );
        assert_eq!(spec.title, None);
    }

    #[test]
    fn caller_title_parses() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            "导出报告",
            &options(Vec::new()),
        );
        assert_eq!(spec.title.as_deref(), Some("导出报告"));
    }

    #[test]
    fn open_file_flags_parse() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![
                ("multiple", Value::Bool(true)),
                ("directory", Value::Bool(true)),
                ("accept_label", Value::Str(Str::from("上传"))),
            ]),
        );
        assert_eq!(
            spec.kind,
            PickerKind::OpenFile {
                multiple: true,
                directory: true
            }
        );
        assert_eq!(spec.accept_label.as_deref(), Some("上传"));
    }

    #[test]
    fn save_file_default_name_parses() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            "",
            &options(vec![("current_name", Value::Str(Str::from("报告.pdf")))]),
        );
        assert_eq!(
            spec.kind,
            PickerKind::SaveFile {
                default_name: Some("报告.pdf".to_string())
            }
        );
    }

    #[test]
    fn multiple_flag_ignored_for_save_file() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            "",
            &options(vec![("multiple", Value::Bool(true))]),
        );
        assert_eq!(spec.kind, PickerKind::SaveFile { default_name: None });
    }

    #[test]
    fn current_folder_accepts_bytes_and_str() {
        for value in [
            Value::Str(Str::from("/tmp")),
            Value::Array(zbus::zvariant::Array::from(b"/tmp".to_vec())),
        ] {
            let spec = PickerRequestSpec::from_options(
                PickerKind::OpenFile {
                    multiple: false,
                    directory: false,
                },
                "",
                &options(vec![("current_folder", value)]),
            );
            assert_eq!(
                spec.start_folder.as_deref(),
                Some(std::path::Path::new("/tmp"))
            );
        }
    }

    #[test]
    fn current_folder_rejects_relative_text() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![("current_folder", Value::Str(Str::from("relative")))]),
        );
        assert_eq!(spec.start_folder, None);
    }

    #[test]
    fn filter_rules_parse_with_current_filter_index() {
        let glob_pattern = Value::Structure(zbus::zvariant::Structure::from((0u8, "*.png")));
        let mime_pattern = Value::Structure(zbus::zvariant::Structure::from((1u8, "image/jpeg")));
        let png_rule = Value::Structure(zbus::zvariant::Structure::from((
            "PNG 图片",
            zbus::zvariant::Array::from(vec![glob_pattern]),
        )));
        let jpeg_rule = Value::Structure(zbus::zvariant::Structure::from((
            "JPEG 图片",
            zbus::zvariant::Array::from(vec![mime_pattern]),
        )));

        let filters = Value::Array(zbus::zvariant::Array::from(vec![
            png_rule.clone(),
            jpeg_rule.clone(),
        ]));
        let current = jpeg_rule;

        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![("filters", filters), ("current_filter", current)]),
        );

        assert_eq!(spec.filters.len(), 2);
        assert_eq!(spec.filters[0].name, "PNG 图片");
        assert_eq!(
            spec.filters[0].patterns,
            vec![FilePattern::Glob("*.png".to_string())]
        );
        assert_eq!(spec.active_filter, Some(1));
    }

    #[test]
    fn malformed_filter_rule_is_skipped() {
        let broken = Value::Structure(zbus::zvariant::Structure::from((7u8, 9u8)));
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![(
                "filters",
                Value::Array(zbus::zvariant::Array::from(vec![broken])),
            )]),
        );
        assert!(spec.filters.is_empty());
    }
}
