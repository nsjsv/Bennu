use super::*;

#[test]
fn case_and_diacritics_do_not_matter() {
    for (text, expected) in [
        ("Ärger.md", "arger.md"),
        ("ÄRGER", "arger"),
        ("Crème Brûlée", "creme brulee"),
        ("Straße", "strasse"),
        ("ＲＥＰＯＲＴ２０２４", "report2024"),
        ("İstanbul", "istanbul"),
    ] {
        assert_eq!(normalize_search_text(text), expected, "{text}");
    }
}

#[test]
fn scripts_that_need_their_marks_are_left_intact() {
    // 日文浊音、韩文音节重新组合后仍是一个字；中文与希伯来元音符号不受影响。
    for text in ["ガイド", "한국어", "年度报告", "\u{05E9}\u{05B8}"] {
        assert_eq!(normalize_search_text(text), text, "{text}");
    }
    assert_eq!(normalize_search_text("ｶﾞｲﾄﾞ"), "ガイド");
}

#[test]
fn normalization_is_idempotent() {
    for text in [
        "Ärger",
        "ǅungla",
        "ẞ",
        "Ｋelvin",
        "ｶﾞ",
        "한국어",
        "ﬁle",
        "Ωmega",
        "Σίσυφος",
    ] {
        let once = normalize_search_text(text);
        assert_eq!(normalize_search_text(&once), once, "{text}");
    }
}

#[test]
fn terms_are_normalized_before_splitting() {
    let terms = SearchTerms::parse("  ÄRGER   Report ");
    assert_eq!(terms.iter().collect::<Vec<_>>(), ["arger", "report"]);
    assert!(SearchTerms::parse("   ").is_empty());
}

#[test]
fn names_match_by_any_part() {
    let name = normalize_search_text("年度报告_MyReport_2024.pdf");
    for query in [
        "报告", "年度", "repo", "port", "MYREPORT", "t_20", "2024.pdf",
    ] {
        assert!(SearchTerms::parse(query).all_in_name(&name), "{query}");
    }
    assert!(!SearchTerms::parse("report 2025").all_in_name(&name));
}

#[test]
fn name_trigram_phrase_matches_exactly_when_the_name_contains_the_term() {
    let name = name_index_text("abcxbcd");
    let tokens = name.split(' ').collect::<Vec<_>>();
    let contains_phrase = |term: &str| {
        let phrase = name_term_tokens(term).expect("long enough");
        let phrase = phrase.split(' ').collect::<Vec<_>>();
        tokens.windows(phrase.len()).any(|window| window == phrase)
    };

    assert!(contains_phrase("abc"));
    assert!(contains_phrase("xbcd"));
    // "abcd" 的两个 trigram 都在，但不相邻：不是子串，短语也不命中。
    assert!(!contains_phrase("abcd"));
    assert_eq!(name_term_tokens("ab"), None);
}

#[test]
fn name_trigram_tokens_are_plain_hex() {
    let text = name_index_text("年度报告 x");
    assert!(text
        .split(' ')
        .all(|token| !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_hexdigit())));
    assert_eq!(text.split(' ').count(), "年度报告 x".chars().count() - 2);
}

#[test]
fn content_splits_words_and_each_unsegmented_character() {
    let tokens = content_tokens("Hello, Wörld! 年度报告 2024年");
    let texts = tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        ["hello", "world", "年", "度", "报", "告", "2024", "年"]
    );
    assert_eq!(tokens[1].char_start, 7);
    assert_eq!(tokens[2].char_start, 14);
}

#[test]
fn content_terms_are_found_by_word_or_by_character_sequence() {
    let text = "Annual report: 年度报告摘要";
    assert_eq!(locate_content_term(text, "REPORT"), Some(7));
    assert_eq!(locate_content_term(text, "报告"), Some(17));
    assert_eq!(locate_content_term(text, "度报"), Some(16));
    // 内容按词：词的一部分不算。
    assert_eq!(locate_content_term(text, "repo"), None);
    assert_eq!(content_term_tokens("--"), None);
}

#[test]
fn phrases_are_quoted_for_fts() {
    assert_eq!(fts_phrase("a b"), "\"a b\"");
    assert_eq!(fts_phrase("a\"b"), "\"a\"\"b\"");
}
