//! 搜索模式的候选和排序。候选在打开框时拼好一次，每打一个字只在这份内存里比较。

use std::path::PathBuf;

use crate::actions::ActionId;
use crate::model::SettingsCategory;
use crate::shortcuts::ShortcutAction;

use super::mode::fold;

/// 一次最多列这么多条。
pub(crate) const MOST_ROWS: usize = 10;

/// 同一档里按这个顺序排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntryKind {
    Feature,
    Action,
    Setting,
    Favorite,
    SmartFolder,
}

impl EntryKind {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Feature => tr!("Feature"),
            Self::Action => tr!("Action"),
            Self::Setting => tr!("Settings"),
            Self::Favorite => tr!("Favorite"),
            Self::SmartFolder => tr!("Smart Folder"),
        }
    }
}

/// 执行一条候选时做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoxTarget {
    Builtin(ShortcutAction),
    Action(ActionId),
    Settings(SettingsCategory),
    Folder(PathBuf),
    /// 智能文件夹：它存的 `f:` 写法。
    SmartFolder(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxEntry {
    pub(crate) kind: EntryKind,
    pub(crate) title: String,
    /// 收藏的路径、动作的说明之类，显示在名字后面。
    pub(crate) detail: Option<String>,
    pub(crate) key_hint: Option<String>,
    pub(crate) target: BoxTarget,
    folded_title: String,
    /// 功能的英文配置名（下划线换成空格），中文界面里打英文也能找到。
    folded_keywords: Option<String>,
}

impl BoxEntry {
    pub(crate) fn new(kind: EntryKind, title: String, target: BoxTarget) -> Self {
        Self {
            kind,
            folded_title: fold(&title),
            title,
            detail: None,
            key_hint: None,
            target,
            folded_keywords: None,
        }
    }

    pub(crate) fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub(crate) fn with_key_hint(mut self, key_hint: Option<String>) -> Self {
        self.key_hint = key_hint;
        self
    }

    pub(crate) fn with_keywords(mut self, keywords: &str) -> Self {
        self.folded_keywords = Some(fold(&keywords.replace('_', " ")));
        self
    }

    /// 越小越靠前：整个名字对上（含拼音首字母对上整个名字）、开头、词首、拼音首字母开头、
    /// 包含，最后是英文配置名。不匹配为 None。
    fn tier(&self, needle: &str) -> Option<u8> {
        let pinyin =
            !self.title.is_ascii() && file_core::type_ahead::name_matches(&self.title, needle);
        let whole_pinyin = pinyin
            && self
                .title
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                == needle.chars().count();
        if self.folded_title == needle || whole_pinyin {
            return Some(0);
        }
        let in_title = text_tier(&self.folded_title, needle);
        if let Some(tier @ 0..=1) = in_title {
            return Some(tier + 1);
        }
        if pinyin {
            return Some(3);
        }
        if in_title.is_some() {
            return Some(4);
        }
        self.folded_keywords
            .as_deref()
            .and_then(|keywords| text_tier(keywords, needle))
            .map(|_| 5)
    }
}

/// 0 开头，1 某个词的开头，2 任意位置。
fn text_tier(folded: &str, needle: &str) -> Option<u8> {
    if folded.starts_with(needle) {
        return Some(0);
    }
    let word_start = folded
        .match_indices(needle)
        .any(|(at, _)| folded[..at].ends_with(|character: char| is_word_break(character)));
    if word_start {
        return Some(1);
    }
    folded.contains(needle).then_some(2)
}

fn is_word_break(character: char) -> bool {
    character.is_whitespace() || "-_/·.()（）".contains(character)
}

/// 和 `needle` 匹配的候选在 `entries` 里的下标，排好序，最多 [`MOST_ROWS`] 条。
pub(crate) fn ranked(entries: &[BoxEntry], text: &str) -> Vec<usize> {
    let needle = fold(text.trim());
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matched = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry.tier(&needle).map(|tier| {
                (
                    (tier, entry.kind, entry.title.chars().count(), index),
                    index,
                )
            })
        })
        .collect::<Vec<_>>();
    matched.sort_unstable_by_key(|(key, _)| *key);
    matched
        .into_iter()
        .take(MOST_ROWS)
        .map(|(_, index)| index)
        .collect()
}
