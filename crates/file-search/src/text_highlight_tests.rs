// 区间数组就是“要加粗的片段”，只有一个片段的写法是本意，不是误写的序列。
#![allow(clippy::single_range_in_vec_init)]

use super::*;

fn highlighted<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
    ranges.iter().map(|range| &text[range.clone()]).collect()
}

#[test]
fn name_ranges_cover_the_original_text_of_every_term() {
    let terms = SearchTerms::parse("report 2024");
    let text = "Annual-Report_2024.pdf";
    assert_eq!(
        highlighted(text, &name_match_ranges(text, &terms)),
        ["Report", "2024"]
    );
}

#[test]
fn name_ranges_survive_case_folding_width_and_diacritics() {
    for (text, query, expected) in [
        ("ÄRGER.md", "arger", "ÄRGER"),
        ("Straße.txt", "strasse", "Straße"),
        ("ＲＥＰＯＲＴ.txt", "report", "ＲＥＰＯＲＴ"),
        ("年度报告.docx", "报告", "报告"),
    ] {
        let terms = SearchTerms::parse(query);
        assert_eq!(
            highlighted(text, &name_match_ranges(text, &terms)),
            [expected],
            "{text}"
        );
    }
}

#[test]
fn overlapping_and_repeated_hits_are_merged_in_order() {
    let terms = SearchTerms::parse("aba ab");
    let text = "ababa";
    assert_eq!(name_match_ranges(text, &terms), [0..5]);
    let terms = SearchTerms::parse("na");
    assert_eq!(
        highlighted("banana", &name_match_ranges("banana", &terms)),
        ["nana"]
    );
}

#[test]
fn content_ranges_follow_word_rules() {
    // 正文按整词命中：port 不算 report 的一部分；中文逐字成词，度报 命中 年度报告。
    let terms = SearchTerms::parse("report");
    let text = "The Report, not the reporter.";
    assert_eq!(
        highlighted(text, &content_match_ranges(text, &terms)),
        ["Report"]
    );
    let terms = SearchTerms::parse("port");
    assert!(content_match_ranges(text, &terms).is_empty());
    let terms = SearchTerms::parse("度报");
    let text = "今年的年度报告";
    assert_eq!(
        highlighted(text, &content_match_ranges(text, &terms)),
        ["度报"]
    );
}

#[test]
fn a_phrase_term_covers_its_words_and_the_gap_between_them() {
    let terms = SearchTerms::parse("年度报告");
    let text = "附件：年度 报告。";
    assert_eq!(
        highlighted(text, &content_match_ranges(text, &terms)),
        ["年度 报告"]
    );
}
