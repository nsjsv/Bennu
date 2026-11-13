//! 搜到的字在原文里的位置，给界面高亮用。与匹配同一套规则：名字按规范化后的子串，
//! 正文按词序列（见 `text_matching`），所以高亮的正是让这条结果命中的那几个字。

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::text_matching::{content_tokens, normalize_search_text, SearchTerms};

/// 名字里每个词出现的位置（原文字节区间，已排序并合并重叠）。
pub fn name_match_ranges(text: &str, terms: &SearchTerms) -> Vec<Range<usize>> {
    // 逐个字素规范化，记下规范化后每个字来自原文的哪个字素：规范化会改变长度
    // （「ß」变「ss」、全角变半角），只能这样把规范化文本里的位置换回原文。
    let mut normalized = String::new();
    let mut sources = Vec::new();
    for (start, grapheme) in text.grapheme_indices(true) {
        let piece = normalize_search_text(grapheme);
        sources.extend(std::iter::repeat_n(
            start..start + grapheme.len(),
            piece.chars().count(),
        ));
        normalized.push_str(&piece);
    }
    let char_index_of = |byte: usize| normalized[..byte].chars().count();
    let mut ranges = Vec::new();
    for term in terms.iter() {
        let term_chars = term.chars().count();
        // 重叠的出现也要找全（「ababa」里的两个「aba」），每次从上一次的下一个字起找。
        let mut from = 0;
        while let Some(found) = normalized[from..].find(term) {
            let byte = from + found;
            let first = char_index_of(byte);
            if let (Some(first), Some(last)) =
                (sources.get(first), sources.get(first + term_chars - 1))
            {
                ranges.push(first.start..last.end);
            }
            from = byte + normalized[byte..].chars().next().map_or(1, char::len_utf8);
        }
    }
    merge(ranges)
}

/// 正文里每个词按词序列命中的位置（原文字节区间，已排序并合并重叠）。
pub fn content_match_ranges(text: &str, terms: &SearchTerms) -> Vec<Range<usize>> {
    let tokens = content_tokens(text);
    let byte_of_char = |char_index: usize| {
        text.char_indices()
            .nth(char_index)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let mut ranges = Vec::new();
    for term in terms.iter() {
        let wanted = content_tokens(term)
            .into_iter()
            .map(|token| token.text)
            .collect::<Vec<_>>();
        if wanted.is_empty() {
            continue;
        }
        for window in tokens.windows(wanted.len()) {
            if window
                .iter()
                .zip(&wanted)
                .all(|(token, wanted)| token.text == *wanted)
            {
                ranges.push(
                    byte_of_char(window[0].char_start)
                        ..byte_of_char(window[window.len() - 1].char_end),
                );
            }
        }
    }
    merge(ranges)
}

fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
#[path = "text_highlight_tests.rs"]
mod tests;
