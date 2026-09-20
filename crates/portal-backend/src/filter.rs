//! 文件名过滤匹配：glob 通配与 MIME 类型两类模式。目录不受过滤约束
//! （进目录导航不能被调用方过滤器挡住），该豁免由 `PickerFilter::entry_allowed`
//! 统一实施。

use file_core::entry::FileKind;

use crate::picker_request::{FilePattern, FilterRule};

/// 一次请求的生效过滤器：无规则 = 全部文件可见。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerFilter {
    rules: Vec<FilterRule>,
}

impl PickerFilter {
    /// 无过滤（所有文件可见）。
    pub(crate) fn unconstrained() -> Self {
        PickerFilter { rules: Vec::new() }
    }

    /// 按指定规则集过滤。
    pub(crate) fn from_rules(rules: Vec<FilterRule>) -> Self {
        PickerFilter { rules }
    }

    /// 规则组的展示名（供过滤器下拉）。
    pub(crate) fn label(&self) -> String {
        match self.rules.as_slice() {
            [] => "所有文件".to_string(),
            [only] => only.name.clone(),
            _ => self
                .rules
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// 目录永远放行；隐藏文件一律不放行（与桌面文件选择器惯例一致）；
    /// 其余条目按任一规则命中放行。
    pub(crate) fn entry_allowed(&self, name: &str, kind: FileKind, is_hidden: bool) -> bool {
        if is_hidden {
            return false;
        }
        match kind {
            FileKind::Directory => true,
            _ => {
                self.rules.is_empty()
                    || self.rules.iter().any(|rule| {
                        rule.patterns
                            .iter()
                            .any(|pattern| pattern_matches(pattern, name))
                    })
            }
        }
    }
}

fn pattern_matches(pattern: &FilePattern, file_name: &str) -> bool {
    match pattern {
        FilePattern::Glob(glob) => glob_matches_ci(glob, file_name),
        FilePattern::Mime(mime) => mime_matches(mime, file_name),
    }
}

/// 大小写不敏感 glob：支持 `*`、`?`、`[...]`（含区间与 `!` 取反）。
/// 与 shell glob 的差别是整串匹配而非按路径段，符合 FileChooser 语义
/// （模式针对单个文件名）。
pub(crate) fn glob_matches_ci(glob: &str, text: &str) -> bool {
    let glob = glob.to_lowercase();
    let text = text.to_lowercase();
    let glob_chars: Vec<char> = glob.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    glob_match(&glob_chars, &text_chars)
}

fn glob_match(glob: &[char], text: &[char]) -> bool {
    match (glob.split_first(), text.split_first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some((&'*', rest)), _) => {
            glob_match(rest, text)
                || text
                    .split_first()
                    .is_some_and(|(_, tail)| glob_match(glob, tail))
        }
        (Some((&'?', rest)), Some((_, tail))) => glob_match(rest, tail),
        (Some((&'[', _)), _) => match glob.iter().position(|&ch| ch == ']') {
            Some(close) => {
                let class: Vec<char> = glob[1..close].to_vec();
                let rest = &glob[close + 1..];
                let Some((&first, tail)) = text.split_first() else {
                    return false;
                };
                let mut negated = false;
                let class = if let Some((_, stripped)) = class
                    .split_first()
                    .filter(|(&head, _)| head == '!' || head == '^')
                {
                    negated = true;
                    stripped.to_vec()
                } else {
                    class
                };
                let in_class = char_class_matches(&class, first);
                (in_class != negated) && glob_match(rest, tail)
            }
            None => false,
        },
        (Some((&expected, rest)), Some((&actual, tail))) => {
            expected == actual && glob_match(rest, tail)
        }
        (Some(_), None) => false,
    }
}

fn char_class_matches(class: &[char], ch: char) -> bool {
    let mut iter = class.iter().copied();
    while let Some(start) = iter.next() {
        if start == ']' {
            continue;
        }
        if let (Some('-'), Some(end)) = (iter.clone().nth(0), iter.clone().nth(1)) {
            if end != ']' && start <= ch && ch <= end {
                return true;
            }
        }
        if start == ch {
            return true;
        }
    }
    false
}

/// MIME 匹配：`image/*` 通配或精确类型。类型推断用 gio
/// `content_type_guess`（按文件名），与桌面环境结论一致。
pub(crate) fn mime_matches(mime: &str, file_name: &str) -> bool {
    let Some(separator) = mime.find('/') else {
        return false;
    };
    let (major, minor) = mime.split_at(separator);
    let minor = &minor[1..];
    let Some(guess) = gio_content_type_guess(file_name) else {
        return false;
    };
    if minor == "*" {
        guess.split('/').next() == Some(major)
    } else {
        guess == mime
    }
}

fn gio_content_type_guess(file_name: &str) -> Option<String> {
    let (content_type, _uncertain) =
        gio::functions::content_type_guess(None::<&str>, Some(file_name.as_bytes()));
    // uncertain 只表示置信度低；扩展名推断仍然比放弃匹配好。
    Some(content_type.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(patterns: Vec<FilePattern>) -> Vec<FilterRule> {
        vec![FilterRule {
            name: "测试".into(),
            patterns,
        }]
    }

    #[test]
    fn glob_star_and_question() {
        assert!(glob_matches_ci("*.png", "photo.PNG"));
        assert!(glob_matches_ci("*.png", "a.b.png"));
        assert!(!glob_matches_ci("*.png", "photo.jpg"));
        assert!(glob_matches_ci("file?.txt", "file1.txt"));
        assert!(!glob_matches_ci("file?.txt", "file12.txt"));
        assert!(glob_matches_ci("*", "anything"));
        assert!(glob_matches_ci("照片*", "照片 001.jpg"));
    }

    #[test]
    fn glob_character_class() {
        assert!(glob_matches_ci("photo[123].jpg", "photo2.jpg"));
        assert!(!glob_matches_ci("photo[123].jpg", "photo4.jpg"));
        assert!(glob_matches_ci("photo[!123].jpg", "photo4.jpg"));
        assert!(glob_matches_ci("photo[1-3].jpg", "photo2.jpg"));
        assert!(!glob_matches_ci("photo[1-3].jpg", "photo5.jpg"));
    }

    #[test]
    fn directory_bypasses_filter_and_hidden_files_filtered() {
        let filter = PickerFilter::from_rules(rule(vec![FilePattern::Glob("*.png".into())]));
        assert!(filter.entry_allowed("docs", FileKind::Directory, false));
        assert!(filter.entry_allowed("x.png", FileKind::File, false));
        assert!(!filter.entry_allowed("x.jpg", FileKind::File, false));
        assert!(!filter.entry_allowed(".hidden.png", FileKind::File, true));
    }

    #[test]
    fn unconstrained_filter_allows_all_visible_files() {
        let filter = PickerFilter::unconstrained();
        assert_eq!(filter.label(), "所有文件");
        assert!(filter.entry_allowed("任何.ext", FileKind::File, false));
        assert!(!filter.entry_allowed(".bashrc", FileKind::File, true));
    }

    #[test]
    fn any_rule_hit_admits_entry() {
        let filter = PickerFilter::from_rules(vec![
            FilterRule {
                name: "图片".into(),
                patterns: vec![FilePattern::Glob("*.jpg".into())],
            },
            FilterRule {
                name: "文本".into(),
                patterns: vec![FilePattern::Glob("*.txt".into())],
            },
        ]);
        assert!(filter.entry_allowed("a.txt", FileKind::File, false));
        assert!(filter.entry_allowed("a.jpg", FileKind::File, false));
        assert!(!filter.entry_allowed("a.doc", FileKind::File, false));
    }

    #[test]
    fn label_joins_multiple_rules() {
        let filter = PickerFilter::from_rules(vec![
            FilterRule {
                name: "图片".into(),
                patterns: vec![],
            },
            FilterRule {
                name: "音频".into(),
                patterns: vec![],
            },
        ]);
        assert_eq!(filter.label(), "图片, 音频");
        assert_eq!(PickerFilter::unconstrained().label(), "所有文件");
    }
}
