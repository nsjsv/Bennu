//! 动作对哪些项显示：`types` 写的 MIME 类型，规则同 KIO 的服务菜单（`kfileitemactions.cpp`
//! 的 `mimeTypeListContains`），Dolphin 服务菜单子任务直接用这一套。

use std::path::Path;

use file_core::mime::MimeDatabase;

const DIRECTORY: &str = "inode/directory";
const ALL_FILES: &str = "application/octet-stream";
const TEXT_PLAIN: &str = file_core::mime::TEXT_PLAIN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeFilter {
    /// 不写 `types`：所有文件和文件夹。
    Anything,
    Patterns(Vec<TypePattern>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypePattern {
    /// 这个类型或它的子类，比如 `image/png`、`inode/directory`。
    Type(String),
    /// `video/*` 这样的一大类，存的是 `/` 前面的部分。
    Group(String),
    /// `application/octet-stream`（KIO 也认 `allfiles`、`all/allfiles`）：所有文件，不含文件夹。
    AllFiles,
    /// `all/all`：什么都算。
    Anything,
}

/// 一项的类型：文件夹是 `inode/directory`，文件只按名字查，查不出就是
/// `application/octet-stream`。打开菜单时不读文件内容。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ItemType(String);

impl ItemType {
    pub(crate) fn of(path: &Path, is_directory: bool, types: &MimeDatabase) -> Self {
        if is_directory {
            return Self(DIRECTORY.to_owned());
        }
        let mime = file_core::mime::type_for_name_only(types, path)
            .map(|mime| types.canonical(&mime))
            .unwrap_or_else(|| ALL_FILES.to_owned());
        Self(mime)
    }
}

impl TypePattern {
    /// 读文件时调用，`Type` 存规范名（去掉别名）。
    pub(crate) fn parse(text: &str, types: &MimeDatabase) -> Option<Self> {
        match text {
            "all/all" => return Some(Self::Anything),
            ALL_FILES | "allfiles" | "all/allfiles" => return Some(Self::AllFiles),
            _ => {}
        }
        let (group, subtype) = text.split_once('/')?;
        let well_formed = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+-._".contains(&byte))
        };
        if !well_formed(group) || subtype.contains('/') {
            return None;
        }
        if subtype == "*" {
            return Some(Self::Group(group.to_owned()));
        }
        well_formed(subtype).then(|| Self::Type(types.canonical(text)))
    }

    fn matches(&self, item: &ResolvedType) -> bool {
        match self {
            Self::Anything => true,
            Self::AllFiles => !item.mime.starts_with("inode/"),
            Self::Group(group) => item
                .mime
                .strip_prefix(group.as_str())
                .is_some_and(|rest| rest.starts_with('/')),
            // 所有 text/* 都算 text/plain 的子类（shared-mime-info 规范的约定）。
            Self::Type(base) => {
                item.ancestors.contains(base)
                    || (base == TEXT_PLAIN && item.mime.starts_with("text/"))
            }
        }
    }
}

/// 一项的类型和它的所有父类，比很多个动作时只算一次。
struct ResolvedType {
    mime: String,
    ancestors: Vec<String>,
}

/// 一批项的类型，每种只查一次父类。
pub(crate) struct TargetTypes(Vec<ResolvedType>);

impl TargetTypes {
    pub(crate) fn new(items: impl IntoIterator<Item = ItemType>, types: &MimeDatabase) -> Self {
        let mut distinct: Vec<ResolvedType> = Vec::new();
        for ItemType(mime) in items {
            if distinct.iter().any(|known| known.mime == mime) {
                continue;
            }
            let ancestors = types.ancestors(&mime);
            distinct.push(ResolvedType { mime, ancestors });
        }
        Self(distinct)
    }
}

impl TypeFilter {
    /// 设置页上的写法，和文件里写的一样。
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Anything => tr!("Everything"),
            Self::Patterns(patterns) => patterns
                .iter()
                .map(|pattern| match pattern {
                    TypePattern::Type(mime) => mime.clone(),
                    TypePattern::Group(group) => format!("{group}/*"),
                    TypePattern::AllFiles => ALL_FILES.to_owned(),
                    TypePattern::Anything => "all/all".to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// 至少有一项匹配其中一个类型：服务菜单的排除类型用它（同 KIO 的 `ExcludeServiceTypes`）。
    pub(crate) fn matches_any(&self, targets: &TargetTypes) -> bool {
        match self {
            Self::Anything => !targets.0.is_empty(),
            Self::Patterns(patterns) => targets
                .0
                .iter()
                .any(|item| patterns.iter().any(|pattern| pattern.matches(item))),
        }
    }

    /// 每一项都至少匹配一个类型才算（同 KIO）。
    pub(crate) fn matches_all(&self, targets: &TargetTypes) -> bool {
        match self {
            Self::Anything => true,
            Self::Patterns(patterns) => targets
                .0
                .iter()
                .all(|item| patterns.iter().any(|pattern| pattern.matches(item))),
        }
    }
}

#[cfg(test)]
#[path = "type_filter_tests.rs"]
mod tests;
