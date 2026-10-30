//! 搜索的唯一文本规则（参照 GNOME：LocalSearch 大小写折叠 + 去变音，Nautilus 无索引
//! 引擎规范化后按子串匹配）。名称、内容、查询词、过滤与本地回退都经这里规范化和切词，
//! 索引与回退因此对同一批文件给出同一组结果。

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use unicase::UniCase;
use unicode_segmentation::UnicodeSegmentation;

/// 名称子串进索引的最短字数（trigram）。更短的词由调用方直接扫规范化名称。
pub(crate) const NAME_INDEX_GRAM_CHARACTERS: usize = 3;

/// NFKD → Unicode 完整大小写折叠 → 去组合变音符号 → NFC。
///
/// 全角字母与半角一致、"ß" 与 "ss" 一致、"Ärger" 与 "arger" 一致；最后重新组合，
/// 韩文音节与日文浊音仍是一个字。结果幂等。
pub(crate) fn normalize_search_text(text: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(text);
    let folded = UniCase::new(decomposed.as_ref()).to_folded_case();
    let unaccented = folded
        .chars()
        .filter(|character| !is_combining_diacritic(*character))
        .collect::<String>();
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(&unaccented)
        .into_owned()
}

/// 只去组合变音符号各区块（与 LocalSearch 的 unaccent 同范围）。不按"组合类非零"一刀切：
/// 那会连日文浊点、希伯来/阿拉伯元音符号一起去掉，改变字义。
fn is_combining_diacritic(character: char) -> bool {
    matches!(
        u32::from(character),
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// 普通模式的查询词：整串规范化后按空白切分（规范化可能产生空白，所以先规范化再切）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchTerms(Vec<String>);

impl SearchTerms {
    pub(crate) fn parse(raw: &str) -> Self {
        Self(
            normalize_search_text(raw)
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        )
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// 名称匹配：每个词都是规范化名称的子串。
    pub(crate) fn all_in_name(&self, search_name: &str) -> bool {
        self.iter().all(|term| search_name.contains(term))
    }
}

/// 规范化名称进索引的文本：每个连续 3 字的 UTF-8 字节写成十六进制，作一个 token。
/// 十六进制只含 `[0-9a-f]`，分词器把它当成一个词；短语查询要求位置相邻，所以
/// "名称包含某词"与"该词的 trigram 短语命中"完全等价。
pub(crate) fn name_index_text(search_name: &str) -> String {
    let characters = search_name.chars().collect::<Vec<_>>();
    characters
        .windows(NAME_INDEX_GRAM_CHARACTERS)
        .map(hex_gram)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 已规范化的查询词在名称索引里的短语（token 以空格分隔）。不满 3 个字时为 None。
pub(crate) fn name_term_tokens(term: &str) -> Option<String> {
    let tokens = name_index_text(term);
    (!tokens.is_empty()).then_some(tokens)
}

fn hex_gram(gram: &[char]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::new();
    let mut buffer = [0_u8; 4];
    for character in gram {
        for byte in character.encode_utf8(&mut buffer).as_bytes() {
            encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// 内容里的一个词：规范化后的文本，以及它在原文里起始的字符序号（摘要定位用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentToken {
    pub(crate) text: String,
    pub(crate) char_start: usize,
}

/// 内容按词切：字母数字开头的字素连成一个词；汉字、假名、泰文等不用空格分词的文字
/// 每个字单独成词（不带词典，"度报"也能命中"年度报告"）。每个词再规范化。
pub(crate) fn content_tokens(text: &str) -> Vec<ContentToken> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut word_start = 0;
    let mut char_index = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_start = char_index;
        char_index += grapheme.chars().count();
        let first = grapheme.chars().next().unwrap_or(' ');
        if is_unsegmented_script(first) {
            push_content_token(&mut tokens, &word, word_start);
            word.clear();
            push_content_token(&mut tokens, grapheme, grapheme_start);
        } else if first.is_alphanumeric() {
            if word.is_empty() {
                word_start = grapheme_start;
            }
            word.push_str(grapheme);
        } else {
            push_content_token(&mut tokens, &word, word_start);
            word.clear();
        }
    }
    push_content_token(&mut tokens, &word, word_start);
    tokens
}

fn push_content_token(tokens: &mut Vec<ContentToken>, original: &str, char_start: usize) {
    if original.is_empty() {
        return;
    }
    for text in normalize_search_text(original).split_whitespace() {
        tokens.push(ContentToken {
            text: text.to_owned(),
            char_start,
        });
    }
}

/// 不用空格分词的文字：汉字（含部首、扩展区、兼容字）、平假名、片假名、泰文、老挝文、
/// 高棉文、缅甸文。韩文词间有空格，按普通词处理。
fn is_unsegmented_script(character: char) -> bool {
    matches!(
        u32::from(character),
        0x2E80..=0x2FDF
            | 0x3005
            | 0x3007
            | 0x3021..=0x3029
            | 0x3038..=0x303B
            | 0x3040..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF66..=0xFF9D
            | 0x0E00..=0x0EFF
            | 0x1000..=0x109F
            | 0x1780..=0x17FF
            | 0x19E0..=0x19FF
            | 0xA9E0..=0xA9FF
            | 0xAA60..=0xAA7F
            | 0x20000..=0x3FFFF
    )
}

/// 内容进索引的文本：规范化后的词以空格连接。分词器配置为只在空格处切，看到的词与这里一致。
pub(crate) fn content_index_text(text: &str) -> String {
    content_tokens(text)
        .into_iter()
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 查询词在内容里的短语（token 以空格分隔）。词里没有可索引的字（如只有标点）时为 None。
pub(crate) fn content_term_tokens(term: &str) -> Option<String> {
    let tokens = content_index_text(term);
    (!tokens.is_empty()).then_some(tokens)
}

/// 查询词在原文里第一次按内容规则命中的字符序号（相邻 token 序列与该词的 token 相同）。
pub(crate) fn locate_content_term(text: &str, term: &str) -> Option<usize> {
    let wanted = content_tokens(term)
        .into_iter()
        .map(|token| token.text)
        .collect::<Vec<_>>();
    if wanted.is_empty() {
        return None;
    }
    let tokens = content_tokens(text);
    tokens
        .windows(wanted.len())
        .find(|window| {
            window
                .iter()
                .zip(&wanted)
                .all(|(token, wanted)| token.text == *wanted)
        })
        .map(|window| window[0].char_start)
}

/// FTS5 短语字面量：整体加双引号，内部双引号写两遍。
pub(crate) fn fts_phrase(tokens: &str) -> String {
    format!("\"{}\"", tokens.replace('"', "\"\""))
}

#[cfg(test)]
#[path = "text_matching_tests.rs"]
mod tests;
