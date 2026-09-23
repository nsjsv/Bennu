//! FileChooser 请求规格：从 D-Bus `a{sv}` options 解析出选择窗口所需的
//! 全部决策参数。解析失败的单个 option 按协议前向兼容原则忽略，不使整个
//! 请求失败。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// 批量保存：调用方经 `current_names`（as）传入文件名列表，用户只
    /// 选目标目录。名字列表是调用方资产，本端永不修改；列表为空时
    /// 确认按钮禁用（与 SaveFile 空输入一致）。
    SaveFiles { default_names: Vec<String> },
}

impl PickerKind {
    /// 窗口标题缺省值：调用方未提供 title（空串/纯空白归一化为 None）时的兜底。
    pub(crate) fn default_title(&self) -> &'static str {
        match self {
            PickerKind::OpenFile {
                directory: true, ..
            } => "选择文件夹",
            PickerKind::OpenFile { .. } => "选择文件",
            PickerKind::SaveFile { .. } | PickerKind::SaveFiles { .. } => "另存为",
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
    /// 调用方 `choices` 选项（下拉/复选）；三种模式都解析，选中值由
    /// 会话簿记，确认时按请求顺序回传。
    pub(crate) choices: Vec<PickerChoice>,
}

/// 调用方经 `choices`（协议 `a(ssa(ss)s)`）传入的单个选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerChoice {
    pub(crate) id: String,
    pub(crate) label: String,
    /// (value, label)；空 = 协议 boolean 形态（复选框，值恒为
    /// "true"/"false"）。
    pub(crate) options: Vec<(String, String)>,
    /// 选中值：初值 = 调用方 current_value；不在 options 里仍保留，
    /// 原样回传由调用方裁决。
    pub(crate) selected: String,
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
            choices: Vec::new(),
        };

        for (key, value) in options {
            match key.as_str() {
                "accept_label" => spec.accept_label = value_as_str(value),
                "handle_token" => {}
                "modal" => {}
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
            PickerKind::SaveFiles { default_names } => {
                for (key, value) in options {
                    if key == "current_names" {
                        *default_names = current_names_from_value(value);
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
        if let Some(value) = options.get("choices") {
            spec.choices = parse_choices(value);
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

/// `as` 型 option 取值：字符串数组。数组元素同样可能被 variant 包裹
/// （av 陷阱），逐个解包后再取字符串；非数组容器按空处理。
fn value_as_string_list(value: &Value<'_>) -> Vec<String> {
    let Value::Array(array) = unwrap_variant(value) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| value_as_str(unwrap_variant(item)))
        .collect()
}

/// SaveFiles 的 `current_names`（as，调用方文件名列表）：逐条归一为
/// 非空 basename。协议约定传纯文件名；防御性丢弃空名与带路径分量的
/// 条目（选目录不是 SaveFiles 的语义）。
fn current_names_from_value(value: &Value<'_>) -> Vec<String> {
    value_as_string_list(value)
        .into_iter()
        .filter_map(|name| {
            let base = Path::new(&name).file_name()?.to_string_lossy().into_owned();
            (!base.is_empty()).then_some(base)
        })
        .collect()
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

/// 解析 `choices`（`a(ssa(ss)s)`）。畸形条目跳过（与 filters 同策略：
/// 单个解析失败不使整个请求失败）。
fn parse_choices(value: &Value<'_>) -> Vec<PickerChoice> {
    let Value::Array(array) = unwrap_variant(value) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| match unwrap_variant(item) {
            Value::Structure(structure) => picker_choice_from_structure(structure),
            _ => None,
        })
        .collect()
}

fn picker_choice_from_structure(
    structure: &zbus::zvariant::Structure<'_>,
) -> Option<PickerChoice> {
    let fields = structure.fields();
    if fields.len() != 4 {
        return None;
    }
    let id = value_as_str(unwrap_variant(&fields[0]))?;
    if id.is_empty() {
        return None;
    }
    let label = value_as_str(unwrap_variant(&fields[1]))?;
    let Value::Array(options_array) = unwrap_variant(&fields[2]) else {
        return None;
    };
    let options = options_array
        .iter()
        .filter_map(|option| match unwrap_variant(option) {
            Value::Structure(option_structure) => {
                let option_fields = option_structure.fields();
                if option_fields.len() != 2 {
                    return None;
                }
                Some((
                    value_as_str(unwrap_variant(&option_fields[0]))?,
                    value_as_str(unwrap_variant(&option_fields[1]))?,
                ))
            }
            _ => None,
        })
        .collect();
    let selected = value_as_str(unwrap_variant(&fields[3]))?;
    Some(PickerChoice {
        id,
        label,
        options,
        selected,
    })
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
        // 协议的模式类型码是 `u`（u32）：真实调用方按 (sa(us)) 发送，
        // zvariant 反序列化为 Value::U32；之前的 Value::U8 分支对应 D-Bus
        // `y`，只会匹配协议外的字节型载荷，导致真实过滤器被整条丢弃。
        let kind = match unwrap_variant(&pattern_fields[0]) {
            Value::U32(0) => FilePattern::Glob,
            Value::U32(1) => FilePattern::Mime,
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
        let glob_pattern = Value::Structure(zbus::zvariant::Structure::from((0u32, "*.png")));
        let mime_pattern = Value::Structure(zbus::zvariant::Structure::from((1u32, "image/jpeg")));
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

    /// 构造一条 `a(ssa(ss)s)` 里的 choice 结构。
    fn choice_value(
        id: &str,
        label: &str,
        opts: Vec<(&str, &str)>,
        selected: &str,
    ) -> Value<'static> {
        let options = zbus::zvariant::Array::from(
            opts.into_iter()
                .map(|(value, text)| {
                    Value::Structure(zbus::zvariant::Structure::from((
                        value.to_string(),
                        text.to_string(),
                    )))
                })
                .collect::<Vec<_>>(),
        );
        Value::Structure(zbus::zvariant::Structure::from((
            id.to_string(),
            label.to_string(),
            options,
            selected.to_string(),
        )))
    }

    #[test]
    fn current_names_parse_into_save_files_kind() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFiles {
                default_names: Vec::new(),
            },
            "",
            &options(vec![(
                "current_names",
                Value::Array(zbus::zvariant::Array::from(vec![
                    "a.txt".to_string(),
                    "b.txt".to_string(),
                ])),
            )]),
        );
        assert_eq!(
            spec.kind,
            PickerKind::SaveFiles {
                default_names: vec!["a.txt".to_string(), "b.txt".to_string()]
            }
        );
    }

    #[test]
    fn current_names_empty_array_yields_empty_list() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFiles {
                default_names: vec!["stale".to_string()],
            },
            "",
            &options(vec![(
                "current_names",
                Value::Array(zbus::zvariant::Array::from(Vec::<String>::new())),
            )]),
        );
        assert_eq!(
            spec.kind,
            PickerKind::SaveFiles {
                default_names: Vec::new()
            }
        );
    }

    #[test]
    fn current_names_rejects_non_array_and_keeps_basenames() {
        // 非 as 容器拒绝（保持空列表）；带路径分量/空条目归一后丢弃。
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFiles {
                default_names: Vec::new(),
            },
            "",
            &options(vec![
                ("current_names", Value::Str(Str::from("not-an-array"))),
                (
                    "current_names",
                    Value::Array(zbus::zvariant::Array::from(vec![
                        "/tmp/a.txt".to_string(),
                        String::new(),
                    ])),
                ),
            ]),
        );
        // HashMap 迭代顺序随机：两种取值可能，但归一结果都合法。
        match &spec.kind {
            PickerKind::SaveFiles { default_names } => {
                assert!(
                    default_names.is_empty() || default_names == &["a.txt".to_string()],
                    "非预期归一结果：{default_names:?}"
                );
            }
            other => panic!("kind 被 current_names 改写：{other:?}"),
        }
    }

    #[test]
    fn choices_parse_dropdown_form() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![(
                "choices",
                Value::Array(zbus::zvariant::Array::from(vec![
                    choice_value(
                        "fmt",
                        "格式",
                        vec![("pdf", "PDF"), ("docx", "DOCX")],
                        "pdf",
                    ),
                    choice_value("tag", "标签", vec![("a", "甲")], "b"),
                ])),
            )]),
        );
        assert_eq!(spec.choices.len(), 2);
        assert_eq!(spec.choices[0].id, "fmt");
        assert_eq!(spec.choices[0].label, "格式");
        assert_eq!(
            spec.choices[0].options,
            vec![
                ("pdf".to_string(), "PDF".to_string()),
                ("docx".to_string(), "DOCX".to_string())
            ]
        );
        assert_eq!(spec.choices[0].selected, "pdf");
        // current_value 不在 options 里仍保留（原样回传由调用方裁决）。
        assert_eq!(spec.choices[1].selected, "b");
    }

    #[test]
    fn choices_parse_boolean_form_and_variant_wrapped_value() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::SaveFile { default_name: None },
            "",
            &options(vec![
                // a{sv} 取值可能整体包在 variant 里（av 陷阱）。
                (
                    "choices",
                    Value::Value(Box::new(Value::Array(zbus::zvariant::Array::from(vec![
                        choice_value("compress", "压缩", Vec::new(), "true"),
                    ])))),
                ),
            ]),
        );
        assert_eq!(spec.choices.len(), 1);
        assert_eq!(spec.choices[0].id, "compress");
        assert!(spec.choices[0].options.is_empty());
        assert_eq!(spec.choices[0].selected, "true");
    }

    #[test]
    fn malformed_choice_entries_are_skipped() {
        let spec = PickerRequestSpec::from_options(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            "",
            &options(vec![(
                "choices",
                Value::Array(zbus::zvariant::Array::from(vec![
                    Value::U8(7),
                    Value::Structure(zbus::zvariant::Structure::from((
                        "bad".to_string(),
                        1u8,
                        2u8,
                    ))),
                    choice_value("ok", "可用", Vec::new(), "false"),
                ])),
            )]),
        );
        assert_eq!(spec.choices.len(), 1);
        assert_eq!(spec.choices[0].id, "ok");
    }

    #[test]
    fn choices_parse_for_all_modes() {
        for kind_seed in [
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            PickerKind::SaveFile { default_name: None },
            PickerKind::SaveFiles {
                default_names: Vec::new(),
            },
        ] {
            let spec = PickerRequestSpec::from_options(
                kind_seed,
                "",
                &options(vec![(
                    "choices",
                    Value::Array(zbus::zvariant::Array::from(vec![choice_value(
                        "fmt",
                        "格式",
                        vec![("pdf", "PDF")],
                        "pdf",
                    )])),
                )]),
            );
            assert_eq!(spec.choices.len(), 1, "三种模式都要解析 choices");
        }
    }
}
