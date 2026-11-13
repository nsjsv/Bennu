//! 用户确认的排序标准用例（任务 11-12-search-ranking 的 research/golden-cases.md），
//! 经正式查询路径逐条断言。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::tempdir;

use crate::extractor::ExtractionStatus;
use crate::model::{SearchFileKind, SearchQuery, SearchScope};
use crate::query_state::QueryState;
use crate::recent_files::RecentFiles;

use super::{EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

struct Fixture<'a> {
    path: &'a str,
    content: &'a str,
    modified_ago_ms: i64,
}

fn file(path: &str) -> Fixture<'_> {
    Fixture {
        path,
        content: "",
        modified_ago_ms: 400 * DAY_MS,
    }
}

fn with_content<'a>(path: &'a str, content: &'a str) -> Fixture<'a> {
    Fixture {
        content,
        ..file(path)
    }
}

fn modified<'a>(path: &'a str, modified_ago_ms: i64) -> Fixture<'a> {
    Fixture {
        modified_ago_ms,
        ..file(path)
    }
}

fn indexed(fixture: &Fixture<'_>) -> IndexedFile {
    let path = PathBuf::from(fixture.path);
    IndexedFile {
        parent_path: path.parent().unwrap().to_path_buf(),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        kind: SearchFileKind::File,
        size: fixture.content.len() as u64,
        modified_ms: Some(now_ms() - fixture.modified_ago_ms),
        accessed_ms: None,
        created_ms: None,
        mime_type: Some("text/plain".to_owned()),
        stage_state: IndexedEntryStageState {
            metadata: EntryStageProgress::Complete,
            content: EntryStageProgress::Complete,
        },
        content: Some(fixture.content.to_owned()),
        extraction_status: ExtractionStatus::Indexed,
        device: Some(1),
        inode: Some(1),
        mtime_ns: Some(1),
        ctime_ns: Some(1),
    }
}

fn ranked(
    fixtures: &[Fixture<'_>],
    terms: &str,
    scope: Option<&str>,
    query_state: &QueryState,
) -> Vec<String> {
    let database = SearchDatabase::in_memory().unwrap();
    for fixture in fixtures {
        database.upsert_file(&indexed(fixture)).unwrap();
    }
    let mut query = SearchQuery::global(1, terms);
    if let Some(scope) = scope {
        query.scope = SearchScope::Directory(PathBuf::from(scope));
    }
    database
        .search(&query, query_state)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.path.to_string_lossy().into_owned())
        .collect()
}

fn in_folder(fixtures: &[Fixture<'_>], terms: &str, folder: &str) -> Vec<String> {
    ranked(fixtures, terms, Some(folder), &QueryState::default())
}

#[test]
fn e1_report() {
    assert_eq!(
        in_folder(
            &[
                with_content("/d/notes.txt", "report report report report report"),
                file("/d/misreported.log"),
                file("/d/annual-report.pdf"),
                file("/d/MyReport.doc"),
                file("/d/reporter.txt"),
                file("/d/report-2024.pdf"),
                file("/d/report.txt"),
            ],
            "report",
            "/d",
        ),
        [
            "/d/report.txt",
            "/d/report-2024.pdf",
            "/d/reporter.txt",
            "/d/MyReport.doc",
            "/d/annual-report.pdf",
            "/d/misreported.log",
            "/d/notes.txt",
        ]
    );
}

#[test]
fn e2_and_e3_case_diacritics_and_full_width() {
    assert_eq!(
        in_folder(
            &[file("/d/resumes.txt"), file("/d/Résumé.pdf")],
            "resume",
            "/d"
        ),
        ["/d/Résumé.pdf", "/d/resumes.txt"]
    );
    let fixtures = [file("/d/maintain.txt"), file("/d/ai-notes.md")];
    assert_eq!(
        in_folder(&fixtures, "ａｉ", "/d"),
        in_folder(&fixtures, "ai", "/d")
    );
}

#[test]
fn e4_two_letters() {
    assert_eq!(
        in_folder(
            &[
                file("/d/maintain.txt"),
                file("/d/OpenAI.txt"),
                file("/d/ai-notes.md")
            ],
            "ai",
            "/d",
        ),
        ["/d/ai-notes.md", "/d/OpenAI.txt", "/d/maintain.txt"]
    );
}

#[test]
fn e5_and_e6_word_starts_after_digits_and_dots() {
    assert_eq!(
        in_folder(
            &[file("/d/explanation.txt"), file("/d/Q3Plan.xlsx")],
            "plan",
            "/d"
        ),
        ["/d/Q3Plan.xlsx", "/d/explanation.txt"]
    );
    assert_eq!(
        in_folder(
            &[file("/d/manual.pdf"), file("/d/pdf-tools.txt")],
            "pdf",
            "/d"
        ),
        ["/d/pdf-tools.txt", "/d/manual.pdf"]
    );
}

#[test]
fn e7_words_in_order() {
    assert_eq!(
        in_folder(
            &[
                with_content("/d/notes.txt", "the project plan for next year"),
                file("/d/plan-for-project.txt"),
                file("/d/project-plan.md"),
            ],
            "project plan",
            "/d",
        ),
        [
            "/d/project-plan.md",
            "/d/plan-for-project.txt",
            "/d/notes.txt"
        ]
    );
}

#[test]
fn e8_e10_depth_within_a_tier_but_not_across_tiers() {
    assert_eq!(
        in_folder(
            &[
                file("/w/a/b/c/budget.xlsx"),
                file("/w/2023/budget.xlsx"),
                file("/w/budget.xlsx"),
                file("/w/budget-notes.txt"),
            ],
            "budget",
            "/w",
        ),
        [
            "/w/budget.xlsx",
            "/w/2023/budget.xlsx",
            "/w/a/b/c/budget.xlsx",
            "/w/budget-notes.txt",
        ]
    );
}

#[test]
fn e9_recently_modified_first() {
    assert_eq!(
        in_folder(
            &[
                modified("/d/invoice-0102.pdf", 180 * DAY_MS),
                modified("/d/invoice-0311.pdf", HOUR_MS),
            ],
            "invoice",
            "/d",
        ),
        ["/d/invoice-0311.pdf", "/d/invoice-0102.pdf"]
    );
}

#[test]
fn c1_report_in_chinese() {
    assert_eq!(
        in_folder(
            &[
                with_content("/文档/会议记录.txt", "本次会议讨论了年度报告和预算报告"),
                file("/文档/周报告模板.md"),
                file("/文档/季度报告.docx"),
                file("/文档/2024年报告.pdf"),
                file("/文档/Q3报告.xlsx"),
                file("/文档/报告模板.md"),
                file("/文档/报告.docx"),
            ],
            "报告",
            "/文档",
        ),
        [
            "/文档/报告.docx",
            "/文档/报告模板.md",
            "/文档/Q3报告.xlsx",
            "/文档/2024年报告.pdf",
            "/文档/季度报告.docx",
            "/文档/周报告模板.md",
            "/文档/会议记录.txt",
        ]
    );
}

#[test]
fn c2_quarterly_report() {
    assert_eq!(
        in_folder(
            &[
                with_content("/文档/笔记.txt", "下周要交季度报告"),
                file("/文档/第三季度报告汇总.xlsx"),
                file("/文档/2024季度报告.pdf"),
                file("/文档/季度报告.docx"),
            ],
            "季度报告",
            "/文档",
        ),
        [
            "/文档/季度报告.docx",
            "/文档/2024季度报告.pdf",
            "/文档/第三季度报告汇总.xlsx",
            "/文档/笔记.txt",
        ]
    );
}

#[test]
fn c3_mixed_terms_in_order() {
    assert_eq!(
        in_folder(
            &[
                file("/文档/Q3汇总.xlsx"),
                file("/文档/报告-Q3.pdf"),
                file("/文档/Q3报告.xlsx")
            ],
            "Q3 报告",
            "/文档",
        ),
        ["/文档/Q3报告.xlsx", "/文档/报告-Q3.pdf"]
    );
}

#[test]
fn c4_single_character() {
    assert_eq!(
        in_folder(
            &[file("/文档/周报.md"), file("/文档/报销单.xlsx")],
            "报",
            "/文档"
        ),
        ["/文档/报销单.xlsx", "/文档/周报.md"]
    );
}

#[test]
fn c5_content_only_by_relevance() {
    assert_eq!(
        in_folder(
            &[
                with_content("/文档/甲.txt", "报告 其他 内容 很多 文字"),
                with_content("/文档/乙.txt", "报告 报告 报告 报告 报告"),
            ],
            "报告",
            "/文档",
        ),
        ["/文档/乙.txt", "/文档/甲.txt"]
    );
}

#[test]
fn c6_tier_outweighs_depth() {
    assert_eq!(
        in_folder(
            &[file("/文档/季度报告.docx"), file("/文档/a/b/c/报告.docx")],
            "报告",
            "/文档",
        ),
        ["/文档/a/b/c/报告.docx", "/文档/季度报告.docx"]
    );
}

#[test]
fn g1_global_search_ignores_depth() {
    // 全局搜索时层数不加分：名字更短的深层文件排在名字更长的浅层文件前面。
    assert_eq!(
        ranked(
            &[file("/w/budget-x.xlsx"), file("/w/a/b/c/budget.xlsx2")],
            "budget",
            None,
            &QueryState::default(),
        ),
        ["/w/a/b/c/budget.xlsx2", "/w/budget-x.xlsx"]
    );
}

fn recently_used(entries: &[(&str, i64)]) -> (tempfile::TempDir, QueryState) {
    let directory = tempdir().unwrap();
    let source = directory.path().join("recently-used.xbel");
    let bookmarks = entries
        .iter()
        .map(|(path, used_ago_ms)| {
            let used_ms = now_ms() - used_ago_ms;
            let seconds = used_ms / 1_000;
            let (days, seconds_of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
            format!(
                r#"<bookmark href="file://{path}" modified="{}T{:02}:{:02}:{:02}Z"/>"#,
                civil_date(days),
                seconds_of_day / 3_600,
                seconds_of_day % 3_600 / 60,
                seconds_of_day % 60
            )
        })
        .collect::<String>();
    std::fs::write(&source, format!("<xbel version=\"1.0\">{bookmarks}</xbel>")).unwrap();
    let query_state = QueryState {
        recent_files: RecentFiles::at(source),
        ..QueryState::default()
    };
    (directory, query_state)
}

/// 1970-01-01 起的天数 → YYYY-MM-DD（Howard Hinnant 的 civil_from_days）。
fn civil_date(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[test]
fn r1_recently_opened_first() {
    let (_directory, query_state) = recently_used(&[("/d/notes-a.txt", DAY_MS)]);
    assert_eq!(
        ranked(
            &[file("/d/notes-b.txt"), file("/d/notes-a.txt")],
            "notes",
            Some("/d"),
            &query_state,
        ),
        ["/d/notes-a.txt", "/d/notes-b.txt"]
    );
}

#[test]
fn r2_opened_outranks_modified() {
    let (_directory, query_state) = recently_used(&[("/d/plan-a.md", 20 * HOUR_MS)]);
    assert_eq!(
        ranked(
            &[
                modified("/d/plan-b.md", HOUR_MS),
                modified("/d/plan-a.md", 365 * DAY_MS)
            ],
            "plan",
            Some("/d"),
            &query_state,
        ),
        ["/d/plan-a.md", "/d/plan-b.md"]
    );
}
