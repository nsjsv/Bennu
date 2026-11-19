//! 打字跳转：在文件列表里直接打字，跳到名字以所打内容开头的项。主程序和文件选择器共用
//! 同一份缓冲规则和匹配规则。
//!
//! - 停 [`RESET_AFTER`] 不打字，下一次输入重新开始（惰性过期，不需要定时器）。
//! - 同一个字母连打（`aaa`）在以它开头的项之间轮流跳。
//! - 名字里的常用汉字按拼音首字母也算：`xz` 匹配“下载”。

use std::borrow::Cow;
use std::time::{Duration, Instant};

use crate::pinyin_initials::pinyin_initial;

pub const RESET_AFTER: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Default)]
pub struct TypeAheadBuffer {
    typed: String,
    deadline: Option<Instant>,
}

/// 这一次输入之后要找什么。都是小写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeAheadQuery {
    /// 名字（或汉字拼音首字母串）以它开头；从当前项开始找，当前项还匹配就不动。
    Prefix(String),
    /// 同一个字母连打：从当前项的下一项开始，找下一个以它开头的。
    Cycle(char),
}

impl TypeAheadBuffer {
    pub fn push(&mut self, typed: char, now: Instant) -> TypeAheadQuery {
        if self.deadline.is_none_or(|deadline| deadline <= now) {
            self.typed.clear();
        }
        self.typed.extend(typed.to_lowercase());
        self.deadline = Some(now + RESET_AFTER);

        let mut characters = self.typed.chars();
        let first = characters.next().expect("a character was just pushed");
        if self.typed.chars().count() > 1 && characters.all(|character| character == first) {
            TypeAheadQuery::Cycle(first)
        } else {
            TypeAheadQuery::Prefix(self.typed.clone())
        }
    }

    pub fn clear(&mut self) {
        self.typed.clear();
        self.deadline = None;
    }

    /// 过了截止时刻就清空；返回清没清。
    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self.deadline.is_some_and(|deadline| deadline <= now);
        if expired {
            self.clear();
        }
        expired
    }

    /// 还在打的内容（给界面显示）；没在打字时为空。
    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn is_active(&self, now: Instant) -> bool {
        !self.typed.is_empty() && self.deadline.is_some_and(|deadline| deadline > now)
    }
}

/// 在按显示顺序排好的名字里找匹配项的下标，到末尾绕回开头。`current` 是当前项的下标。
pub fn find_match<S: AsRef<str>>(
    names: &[S],
    current: Option<usize>,
    query: &TypeAheadQuery,
) -> Option<usize> {
    if names.is_empty() {
        return None;
    }
    let (start, needle) = match query {
        TypeAheadQuery::Prefix(needle) => (current.unwrap_or(0), Cow::Borrowed(needle.as_str())),
        TypeAheadQuery::Cycle(letter) => (
            current.map_or(0, |index| index + 1),
            Cow::Owned(letter.to_string()),
        ),
    };
    (0..names.len())
        .map(|offset| (start + offset) % names.len())
        .find(|index| name_matches(names[*index].as_ref(), &needle))
}

/// 名字小写后以 `needle` 开头，或把常用汉字换成拼音首字母（其余字符小写，空白去掉）后
/// 以 `needle` 开头。
pub fn name_matches(name: &str, needle: &str) -> bool {
    if name.to_lowercase().starts_with(needle) {
        return true;
    }
    if name.is_ascii() {
        return false;
    }
    let mut needle = needle.chars();
    for character in name.chars().filter(|character| !character.is_whitespace()) {
        let Some(expected) = needle.next() else {
            return true;
        };
        let matched = match pinyin_initial(character) {
            Some(initial) => initial == expected,
            None => character.to_lowercase().eq(std::iter::once(expected)),
        };
        if !matched {
            return false;
        }
    }
    needle.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(buffer: &mut TypeAheadBuffer, typed: &str, now: Instant) -> TypeAheadQuery {
        let mut last = None;
        for character in typed.chars() {
            last = Some(buffer.push(character, now));
        }
        last.expect("something was typed")
    }

    #[test]
    fn typing_builds_a_prefix_and_resets_after_a_pause() {
        let start = Instant::now();
        let mut buffer = TypeAheadBuffer::default();
        assert_eq!(
            query(&mut buffer, "Re", start),
            TypeAheadQuery::Prefix("re".to_owned())
        );
        assert!(buffer.is_active(start));
        assert_eq!(
            buffer.push('p', start + RESET_AFTER / 2),
            TypeAheadQuery::Prefix("rep".to_owned())
        );
        let later = start + RESET_AFTER * 3;
        assert!(!buffer.is_active(later));
        assert_eq!(
            buffer.push('x', later),
            TypeAheadQuery::Prefix("x".to_owned())
        );
        assert!(buffer.expire(later + RESET_AFTER));
        assert_eq!(buffer.typed(), "");
    }

    #[test]
    fn the_same_letter_typed_again_cycles() {
        let now = Instant::now();
        let mut buffer = TypeAheadBuffer::default();
        assert_eq!(query(&mut buffer, "aa", now), TypeAheadQuery::Cycle('a'));
        assert_eq!(query(&mut buffer, "a", now), TypeAheadQuery::Cycle('a'));
        assert_eq!(
            buffer.push('b', now),
            TypeAheadQuery::Prefix("aaab".to_owned())
        );
    }

    #[test]
    fn prefixes_match_case_insensitively_and_by_pinyin_initials() {
        assert!(name_matches("Report.pdf", "rep"));
        assert!(!name_matches("my report", "rep"));
        assert!(name_matches("下载", "xz"));
        assert!(name_matches("下载", "x"));
        assert!(name_matches("下载", "下"));
        assert!(name_matches("我的 文档", "wdw"));
        assert!(name_matches("工作2024", "gz20"));
        assert!(!name_matches("下载", "xzz"));
        assert!(name_matches("Ärger", "ä"));
    }

    #[test]
    fn matches_start_at_the_current_item_and_wrap() {
        let names = ["apple", "banana", "avocado", "cherry", "apricot"];
        let prefix = |needle: &str| TypeAheadQuery::Prefix(needle.to_owned());

        assert_eq!(find_match(&names, None, &prefix("a")), Some(0));
        assert_eq!(find_match(&names, Some(2), &prefix("a")), Some(2));
        assert_eq!(find_match(&names, Some(3), &prefix("a")), Some(4));
        assert_eq!(find_match(&names, Some(1), &prefix("z")), None);

        assert_eq!(
            find_match(&names, Some(0), &TypeAheadQuery::Cycle('a')),
            Some(2)
        );
        assert_eq!(
            find_match(&names, Some(4), &TypeAheadQuery::Cycle('a')),
            Some(0)
        );
    }
}
