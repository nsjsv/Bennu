use super::*;

const NOW: i64 = 1_800_000_000_000;

fn facts<'a>(display_name: &'a str, search_name: &'a str) -> RankingFacts<'a> {
    RankingFacts {
        display_name,
        search_name,
        depth: Some(0),
        modified_ms: None,
        last_used_ms: None,
        content_bm25: None,
    }
}

/// 只有名字不同时的排序；不含搜索词的名字代表只有正文命中。
fn order<'a>(query: &str, names: &[&'a str]) -> Vec<&'a str> {
    let terms = SearchTerms::parse(query);
    let mut ranked = names
        .iter()
        .map(|name| {
            let search_name = normalize_search_text(name);
            (relevance(&terms, &facts(name, &search_name), NOW), *name)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.total_cmp(&left.0));
    ranked.into_iter().map(|(_, name)| name).collect()
}

fn tier(query: &str, name: &str) -> NameTier {
    name_match(name, &SearchTerms::parse(query)).tier
}

#[test]
fn e1_whole_names_then_prefixes_then_word_starts_then_infixes_then_content() {
    assert_eq!(
        order(
            "report",
            &[
                "notes.txt",
                "misreported.log",
                "annual-report.pdf",
                "MyReport.doc",
                "reporter.txt",
                "report-2024.pdf",
                "report.txt",
            ],
        ),
        [
            "report.txt",
            "report-2024.pdf",
            "reporter.txt",
            "MyReport.doc",
            "annual-report.pdf",
            "misreported.log",
            "notes.txt",
        ]
    );
}

#[test]
fn e2_case_and_diacritics_do_not_stop_an_exact_name() {
    assert_eq!(tier("resume", "Résumé.pdf"), NameTier::Exact);
}

#[test]
fn e3_full_width_queries_match_like_half_width() {
    assert_eq!(SearchTerms::parse("ａｉ"), SearchTerms::parse("ai"));
}

#[test]
fn e4_two_letter_words_rank_by_where_they_start() {
    assert_eq!(
        order("ai", &["maintain.txt", "OpenAI.txt", "ai-notes.md"]),
        ["ai-notes.md", "OpenAI.txt", "maintain.txt"]
    );
}

#[test]
fn e5_camel_case_after_a_digit_starts_a_word() {
    assert_eq!(tier("plan", "Q3Plan.xlsx"), NameTier::WordStart);
    assert_eq!(tier("parser", "HTMLParser.rs"), NameTier::WordStart);
}

#[test]
fn e6_an_extension_is_a_word() {
    assert_eq!(
        order("pdf", &["manual.pdf", "pdf-tools.txt"]),
        ["pdf-tools.txt", "manual.pdf"]
    );
}

#[test]
fn e7_words_in_the_typed_order_come_first() {
    assert_eq!(
        order(
            "project plan",
            &["notes.txt", "plan-for-project.txt", "project-plan.md"]
        ),
        ["project-plan.md", "plan-for-project.txt", "notes.txt"]
    );
}

#[test]
fn c1_chinese_names_rank_from_whole_name_to_content() {
    assert_eq!(
        order(
            "报告",
            &[
                "会议记录.txt",
                "周报告模板.md",
                "季度报告.docx",
                "2024年报告.pdf",
                "Q3报告.xlsx",
                "报告模板.md",
                "报告.docx",
            ],
        ),
        [
            "报告.docx",
            "报告模板.md",
            "Q3报告.xlsx",
            "2024年报告.pdf",
            "季度报告.docx",
            "周报告模板.md",
            "会议记录.txt",
        ]
    );
}

#[test]
fn c2_a_word_after_a_number_and_its_unit_starts_a_word() {
    assert_eq!(
        order(
            "季度报告",
            &["第三季度报告汇总.xlsx", "2024季度报告.pdf", "季度报告.docx"]
        ),
        ["季度报告.docx", "2024季度报告.pdf", "第三季度报告汇总.xlsx"]
    );
    assert_eq!(tier("报告", "第3章报告.md"), NameTier::WordStart);
}

#[test]
fn c3_mixed_scripts_in_the_typed_order_come_first() {
    assert_eq!(
        order("Q3 报告", &["报告-Q3.pdf", "Q3报告.xlsx"]),
        ["Q3报告.xlsx", "报告-Q3.pdf"]
    );
}

#[test]
fn c4_a_single_character_ranks_prefix_before_infix() {
    assert_eq!(
        order("报", &["周报.md", "报销单.xlsx"]),
        ["报销单.xlsx", "周报.md"]
    );
}

#[test]
fn separators_inside_a_term_do_not_break_the_typed_order() {
    assert!(name_match("file_name-v2.txt", &SearchTerms::parse("file_name v2")).in_order);
}

#[test]
fn a_dot_name_or_a_long_suffix_is_not_an_extension() {
    assert_eq!(name_stem(".bashrc"), ".bashrc");
    assert_eq!(name_stem("v1.2 notes"), "v1.2 notes");
    assert_eq!(name_stem("report.tar.gz"), "report.tar");
}

fn score(facts: RankingFacts<'_>) -> f64 {
    relevance(&SearchTerms::parse("budget"), &facts, NOW)
}

#[test]
fn e8_shallower_files_win_within_a_tier() {
    let shallow = score(RankingFacts {
        depth: Some(0),
        ..facts("budget.xlsx", "budget.xlsx")
    });
    let deeper = score(RankingFacts {
        depth: Some(1),
        ..facts("budget.xlsx", "budget.xlsx")
    });
    let deepest = score(RankingFacts {
        depth: Some(3),
        ..facts("budget.xlsx", "budget.xlsx")
    });
    assert!(shallow > deeper && deeper > deepest);
}

#[test]
fn e9_recently_modified_files_win_within_a_tier() {
    let today = score(RankingFacts {
        modified_ms: Some(NOW - 3_600_000),
        ..facts("budget-0311.pdf", "budget-0311.pdf")
    });
    let half_a_year_ago = score(RankingFacts {
        modified_ms: Some(NOW - 180 * DAY_MILLISECONDS),
        ..facts("budget-0102.pdf", "budget-0102.pdf")
    });
    assert!(today > half_a_year_ago);
}

#[test]
fn e10_and_c6_the_tier_outweighs_depth() {
    let exact_deep = score(RankingFacts {
        depth: Some(3),
        ..facts("budget.xlsx", "budget.xlsx")
    });
    let prefix_shallow = score(facts("budget-notes.txt", "budget-notes.txt"));
    assert!(exact_deep > prefix_shallow);
}

#[test]
fn c5_content_only_hits_rank_by_content_relevance() {
    let many = score(RankingFacts {
        content_bm25: Some(-3.0),
        ..facts("a.txt", "a.txt")
    });
    let once = score(RankingFacts {
        content_bm25: Some(-1.0),
        ..facts("b.txt", "b.txt")
    });
    assert!(many > once);
}

#[test]
fn g1_global_search_has_no_depth_bonus() {
    let global = score(RankingFacts {
        depth: None,
        ..facts("budget.xlsx", "budget.xlsx")
    });
    let deepest = score(RankingFacts {
        depth: Some(10),
        ..facts("budget.xlsx", "budget.xlsx")
    });
    assert_eq!(global, deepest);
}

#[test]
fn r1_and_r2_recently_opened_files_outrank_recently_modified_ones() {
    let opened = score(RankingFacts {
        last_used_ms: Some(NOW - 20 * 3_600_000),
        modified_ms: Some(NOW - 365 * DAY_MILLISECONDS),
        ..facts("budget-a.md", "budget-a.md")
    });
    let modified = score(RankingFacts {
        modified_ms: Some(NOW - 3_600_000),
        ..facts("budget-b.md", "budget-b.md")
    });
    let untouched = score(facts("budget-c.md", "budget-c.md"));
    assert!(opened > modified && modified > untouched);
}
