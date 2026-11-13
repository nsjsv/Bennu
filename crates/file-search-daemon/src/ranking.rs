//! 相关度查询的排序规则。先按名字匹配分档，档内再按位置、最近使用、最近修改、
//! 正文相关度和名字长短打分。字符类别切换处（大小写、字母数字、中西文）算词开头；
//! 名字匹配的档位压过目录层级，中文数字后的单位字（2024年、第3章）与数字算一个词。

use crate::text_matching::{normalize_search_text, SearchTerms};

const DAY_MILLISECONDS: i64 = 86_400_000;
/// 数字后紧跟这些字时与数字算一个词，后面的字算词开头：「2024年报告」里的「报告」。
const NUMERAL_UNITS: [&str; 16] = [
    "季度", "年", "月", "日", "号", "期", "版", "周", "届", "次", "章", "节", "集", "卷", "篇",
    "级",
];

/// 名字匹配的档位，小的更相关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NameTier {
    /// 去掉扩展名后与整句搜索词完全一样。
    Exact,
    /// 名字以搜索词开头。
    Prefix,
    /// 名字里某个词以搜索词开头。
    WordStart,
    /// 名字中间包含搜索词。
    Contains,
    /// 至少有一个词只在正文里。
    ContentOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NameMatch {
    pub(crate) tier: NameTier,
    /// 每个词的匹配都止于词尾：report-2024 里的 report 是整词，reporter 里的不是。
    pub(crate) whole_words: bool,
    /// 去掉分隔符后，搜索词按输入顺序首尾相接出现：project-plan、Q3报告。
    pub(crate) in_order: bool,
    /// 去掉扩展名后，名字比搜索词多出的字数。
    pub(crate) extra_characters: usize,
}

impl NameMatch {
    const CONTENT_ONLY: Self = Self {
        tier: NameTier::ContentOnly,
        whole_words: false,
        in_order: false,
        extra_characters: 0,
    };
}

/// 一条命中打分所需的事实。时间都是 Unix 毫秒。
pub(crate) struct RankingFacts<'a> {
    pub(crate) display_name: &'a str,
    /// 索引里规范化过的名字，用来先判断是不是只有正文命中，省掉切词。
    pub(crate) search_name: &'a str,
    /// 相对搜索目录的层数，直接子项为 0；全局搜索没有层数。
    pub(crate) depth: Option<usize>,
    pub(crate) modified_ms: Option<i64>,
    /// 系统最近使用列表里的最后使用时间。
    pub(crate) last_used_ms: Option<i64>,
    /// 只算正文列的 bm25（越小越相关）；不是全文命中时没有。
    pub(crate) content_bm25: Option<f64>,
}

/// 一条命中的相关度，越大越相关。档位、整词、按顺序紧挨依次编进高位，
/// 档内分小于 `TIER_UNIT / 100`，所以按这个数排序就是按排序键的字典序。
pub(crate) fn relevance(terms: &SearchTerms, facts: &RankingFacts<'_>, now_ms: i64) -> f64 {
    const TIER_UNIT: f64 = 1_000_000.0;
    let name_match = if terms.all_in_name(facts.search_name) {
        name_match(facts.display_name, terms)
    } else {
        NameMatch::CONTENT_ONLY
    };
    let tier_rank = (NameTier::ContentOnly as u8 - name_match.tier as u8) as f64;
    tier_rank * TIER_UNIT
        + f64::from(u8::from(name_match.whole_words)) * TIER_UNIT / 10.0
        + f64::from(u8::from(name_match.in_order)) * TIER_UNIT / 100.0
        + within_tier_score(&name_match, facts, now_ms)
}

fn within_tier_score(name_match: &NameMatch, facts: &RankingFacts<'_>, now_ms: i64) -> f64 {
    let proximity = facts
        .depth
        .map_or(0.0, |depth| 100.0 * (10 - depth.min(10)) as f64);
    // 打开文件是用户主动用了它，修改可能来自同步或程序自动写入，所以最近使用的分量翻倍。
    let recently_used = facts
        .last_used_ms
        .map_or(0.0, |used_ms| 2.0 * recency_bucket(now_ms, used_ms));
    let recently_modified = facts
        .modified_ms
        .map_or(0.0, |modified_ms| recency_bucket(now_ms, modified_ms));
    let content = facts
        .content_bm25
        .map_or(0.0, |bm25| (-25.0 * bm25).clamp(0.0, 100.0));
    let shortness = match name_match.tier {
        NameTier::ContentOnly => 0.0,
        _ => 30.0_f64 - 3.0 * name_match.extra_characters.min(10) as f64,
    };
    proximity + recently_used + recently_modified + content + shortness
}

/// 最近分桶：一天内、一周内、两周内、一月内、三月内依次递减，更早的不加分。
fn recency_bucket(now_ms: i64, then_ms: i64) -> f64 {
    match now_ms.saturating_sub(then_ms).max(0) / DAY_MILLISECONDS {
        0..=1 => 100.0,
        2..=7 => 70.0,
        8..=14 => 50.0,
        15..=30 => 30.0,
        31..=90 => 10.0,
        _ => 0.0,
    }
}

pub(crate) fn name_match(display_name: &str, terms: &SearchTerms) -> NameMatch {
    let name = SegmentedName::new(display_name);
    let stem = normalize_search_text(name_stem(display_name));
    let whole_query = terms.iter().collect::<Vec<_>>().join(" ");
    if stem == whole_query || name.text == whole_query {
        return NameMatch {
            tier: NameTier::Exact,
            whole_words: true,
            in_order: true,
            extra_characters: 0,
        };
    }

    let mut tier = NameTier::Exact;
    let mut whole_words = true;
    for term in terms.iter() {
        let Some((term_tier, whole_word)) = name.best_occurrence(term) else {
            return NameMatch::CONTENT_ONLY;
        };
        tier = tier.max(term_tier);
        whole_words &= whole_word;
    }
    let matched_characters = terms.iter().map(|term| term.chars().count()).sum::<usize>();
    let compact_query = terms
        .iter()
        .flat_map(str::chars)
        .filter(|character| character_class(*character) != CharacterClass::Separator)
        .collect::<String>();
    NameMatch {
        tier,
        whole_words,
        in_order: name.compact().contains(&compact_query),
        extra_characters: stem.chars().count().saturating_sub(matched_characters),
    }
}

/// 去掉最后一个扩展名：点不在开头，扩展名是 1～10 个字母数字。
fn name_stem(display_name: &str) -> &str {
    match display_name.rfind('.') {
        Some(dot)
            if dot > 0
                && (1..=10).contains(&display_name[dot + 1..].chars().count())
                && display_name[dot + 1..].chars().all(char::is_alphanumeric) =>
        {
            &display_name[..dot]
        }
        _ => display_name,
    }
}

/// 规范化后的名字和其中每个词的起止位置（字节偏移）。先在原文上按字符类别切词再逐段
/// 规范化：驼峰等信息在规范化（大小写折叠）后就看不出来了。
struct SegmentedName {
    text: String,
    words: Vec<(usize, usize)>,
}

impl SegmentedName {
    fn new(display_name: &str) -> Self {
        let mut text = String::new();
        let mut words = Vec::new();
        for (segment, is_separator) in segments(display_name) {
            let start = text.len();
            text.push_str(&normalize_search_text(segment));
            if !is_separator {
                words.push((start, text.len()));
            }
        }
        Self { text, words }
    }

    /// 搜索词在名字里最好的一次出现：档位，以及是否止于词尾。
    fn best_occurrence(&self, term: &str) -> Option<(NameTier, bool)> {
        self.text
            .match_indices(term)
            .map(|(start, _)| {
                let end = start + term.len();
                let tier = if start == 0 {
                    NameTier::Prefix
                } else if self
                    .words
                    .iter()
                    .any(|(word_start, _)| *word_start == start)
                {
                    NameTier::WordStart
                } else {
                    NameTier::Contains
                };
                let whole_word = self.words.iter().any(|(_, word_end)| *word_end == end);
                (tier, whole_word)
            })
            .min_by_key(|(tier, whole_word)| (*tier, !*whole_word))
    }

    /// 只留词、去掉分隔符的名字，用来判断多个词是否按顺序紧挨。
    fn compact(&self) -> String {
        self.words
            .iter()
            .map(|(start, end)| &self.text[*start..*end])
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharacterClass {
    Separator,
    Lower,
    Upper,
    Digit,
    Cjk,
    OtherLetter,
}

fn character_class(character: char) -> CharacterClass {
    if character.is_whitespace() || is_separator_punctuation(character) {
        CharacterClass::Separator
    } else if is_cjk(character) {
        CharacterClass::Cjk
    } else if character.is_numeric() {
        CharacterClass::Digit
    } else if character.is_uppercase() {
        CharacterClass::Upper
    } else if character.is_lowercase() {
        CharacterClass::Lower
    } else {
        CharacterClass::OtherLetter
    }
}

fn is_separator_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
        || matches!(
            u32::from(character),
            // CJK 标点、全角 ASCII 标点、中点与引号破折号省略号。
            0x3000..=0x3003 | 0x3008..=0x3011 | 0x3014..=0x301F | 0x30FB
                | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65
                | 0x00B7 | 0x2010..=0x2027
        )
}

fn is_cjk(character: char) -> bool {
    matches!(
        u32::from(character),
        0x3040..=0x30FA | 0x30FC..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF | 0x20000..=0x3134F | 0x1100..=0x11FF | 0x3130..=0x318F
            | 0xAC00..=0xD7AF
    )
}

/// 按词开头切开原始名字，返回 (片段, 是否分隔符)。
fn segments(display_name: &str) -> Vec<(&str, bool)> {
    let characters = display_name.char_indices().collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut segment_start = 0;
    for index in 1..characters.len() {
        let (byte_index, character) = characters[index];
        let previous = character_class(characters[index - 1].1);
        let current = character_class(character);
        let next = characters
            .get(index + 1)
            .map(|(_, next)| character_class(*next));
        if starts_word(previous, current, next)
            || follows_numeral_unit(&display_name[..byte_index], current)
        {
            segments.push(&display_name[segment_start..byte_index]);
            segment_start = byte_index;
        }
    }
    segments.push(&display_name[segment_start..]);
    segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let is_separator = segment
                .chars()
                .all(|character| character_class(character) == CharacterClass::Separator);
            (segment, is_separator)
        })
        .collect()
}

fn starts_word(
    previous: CharacterClass,
    current: CharacterClass,
    next: Option<CharacterClass>,
) -> bool {
    use CharacterClass::*;
    match (previous, current) {
        (Separator, Separator) => false,
        (Separator, _) | (_, Separator) => true,
        (Lower, Upper) => true,
        // HTMLParser：大写串里最后一个大写字母后跟小写，才是新词开头。
        (Upper, Upper) => next == Some(Lower),
        (Digit, Digit) | (Cjk, Cjk) => false,
        (Digit, _) | (_, Digit) | (Cjk, _) | (_, Cjk) => true,
        _ => false,
    }
}

/// 数字后紧跟单位字时，单位字之后算词开头。
fn follows_numeral_unit(before: &str, current: CharacterClass) -> bool {
    current == CharacterClass::Cjk
        && NUMERAL_UNITS.iter().any(|unit| {
            before.strip_suffix(unit).is_some_and(|rest| {
                rest.chars()
                    .next_back()
                    .is_some_and(|character| character_class(character) == CharacterClass::Digit)
            })
        })
}

#[cfg(test)]
#[path = "ranking_tests.rs"]
mod tests;
