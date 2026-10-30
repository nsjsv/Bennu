use std::collections::{BTreeSet, HashSet};

use super::{fill_placeholders, highest_placeholder, placeholder_number};
use crate::catalog::TABLES;
use crate::UiLanguage;

fn placeholders(template: &str) -> BTreeSet<usize> {
    template
        .as_bytes()
        .windows(2)
        .filter_map(|pair| placeholder_number(pair[0], pair[1]))
        .collect()
}

#[test]
fn every_message_appears_once_and_both_languages_use_the_same_placeholders() {
    let mut seen = HashSet::new();
    for &(english, chinese) in TABLES.iter().copied().flatten() {
        assert!(seen.insert(english), "duplicate message: {english}");
        let expected = (1..=highest_placeholder(english)).collect::<BTreeSet<_>>();
        assert_eq!(
            placeholders(english),
            expected,
            "use %1..%N in order: {english}"
        );
        assert_eq!(
            placeholders(chinese),
            expected,
            "Chinese placeholders differ: {english}"
        );
    }
}

#[test]
fn arguments_are_filled_in_after_the_sentence_is_chosen() {
    // 参数长得像文案（文件夹叫 "Retry"）也只是被原样填进去。
    assert_eq!(
        crate::tr_in!(UiLanguage::Chinese, "Could not load: %1", "Retry"),
        "无法加载：Retry"
    );
    assert_eq!(
        crate::tr_in!(UiLanguage::English, "Could not load: %1", "Retry"),
        "Could not load: Retry"
    );
}

#[test]
fn sentences_can_reorder_arguments_and_keep_other_percent_signs() {
    assert_eq!(fill_placeholders("%2 和 %1", &[&"a", &"b"]), "b 和 a");
    assert_eq!(fill_placeholders("100% %", &[]), "100% %");
    assert_eq!(
        crate::tr_in!(UiLanguage::Chinese, "Volume %1%", 80),
        "音量 80%"
    );
}
