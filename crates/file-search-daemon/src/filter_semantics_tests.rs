//! 过滤语义矩阵：同一组用例分别经条目事实（本地回退）与索引 SQL 条件判定。

use std::path::{Path, PathBuf};

use crate::database::{EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase};
use crate::extractor::ExtractionStatus;
use crate::model::{
    MimePattern, SearchEntryTypeRule, SearchFileKind, SearchFilters, SearchQuery, SizeRange,
    TimeRange,
};
use crate::text_matching::normalize_search_text;
use file_search::filter_semantics::EntryFacts;

pub(crate) struct FilterCase {
    pub(crate) label: &'static str,
    pub(crate) filters: SearchFilters,
    pub(crate) display_name: &'static str,
    pub(crate) kind: SearchFileKind,
    pub(crate) mime_type: Option<&'static str>,
    pub(crate) size: u64,
    pub(crate) modified_ms: Option<i64>,
    pub(crate) expected: bool,
}

fn extensions(tokens: &[&str]) -> SearchFilters {
    SearchFilters {
        extensions: tokens.iter().map(|token| (*token).to_owned()).collect(),
        ..SearchFilters::default()
    }
}

fn case(
    label: &'static str,
    filters: SearchFilters,
    display_name: &'static str,
    kind: SearchFileKind,
    expected: bool,
) -> FilterCase {
    FilterCase {
        label,
        filters,
        display_name,
        kind,
        mime_type: None,
        size: 10,
        modified_ms: Some(1_000),
        expected,
    }
}

pub(crate) fn filter_cases() -> Vec<FilterCase> {
    let size = |min_bytes, max_bytes| SearchFilters {
        size: Some(SizeRange {
            min_bytes,
            max_bytes,
        }),
        ..SearchFilters::default()
    };
    let modified = |start_ms, end_ms| SearchFilters {
        modified: Some(TimeRange { start_ms, end_ms }),
        ..SearchFilters::default()
    };
    let rules = |rules: Vec<SearchEntryTypeRule>| SearchFilters {
        entry_type_rules: rules,
        ..SearchFilters::default()
    };
    vec![
        case(
            "multi-part suffix",
            extensions(&["tar.gz"]),
            "backup.tar.gz",
            SearchFileKind::File,
            true,
        ),
        case(
            "last part of a multi-part suffix",
            extensions(&["gz"]),
            "backup.tar.gz",
            SearchFileKind::File,
            true,
        ),
        case(
            "suffix needs a dot",
            extensions(&["gz"]),
            "backupgz",
            SearchFileKind::File,
            false,
        ),
        case(
            "suffix ignores case",
            extensions(&["pdf"]),
            "REPORT.PDF",
            SearchFileKind::File,
            true,
        ),
        case(
            "suffix ignores diacritics",
            extensions(&["jpeg"]),
            "photo.JPÉG",
            SearchFileKind::File,
            true,
        ),
        case(
            "directories never match a suffix",
            extensions(&["d"]),
            "conf.d",
            SearchFileKind::Directory,
            false,
        ),
        case(
            "size lower bound is inclusive",
            size(10, Some(11)),
            "a",
            SearchFileKind::File,
            true,
        ),
        case(
            "size upper bound is exclusive",
            size(0, Some(10)),
            "a",
            SearchFileKind::File,
            false,
        ),
        case(
            "directories are never excluded by size",
            size(100, None),
            "folder",
            SearchFileKind::Directory,
            true,
        ),
        case(
            "time range is inclusive",
            modified(1_000, 1_000),
            "a",
            SearchFileKind::File,
            true,
        ),
        case(
            "outside the time range",
            modified(1_001, 2_000),
            "a",
            SearchFileKind::File,
            false,
        ),
        FilterCase {
            mime_type: Some("image/png"),
            ..case(
                "mime prefix",
                rules(vec![SearchEntryTypeRule::Mime(MimePattern::Prefix(
                    "image/".to_owned(),
                ))]),
                "a.png",
                SearchFileKind::File,
                true,
            )
        },
        case(
            "kind rule",
            rules(vec![SearchEntryTypeRule::Kind(SearchFileKind::Directory)]),
            "a",
            SearchFileKind::File,
            false,
        ),
        FilterCase {
            modified_ms: None,
            ..case(
                "missing time never matches a time range",
                modified(0, i64::MAX),
                "a",
                SearchFileKind::File,
                false,
            )
        },
    ]
}

#[test]
fn index_sql_follows_the_same_filter_matrix() {
    for case in filter_cases() {
        let database = SearchDatabase::in_memory().unwrap();
        database
            .upsert_file(&IndexedFile {
                path: Path::new("/tmp").join(case.display_name),
                parent_path: PathBuf::from("/tmp"),
                display_name: case.display_name.to_owned(),
                kind: case.kind,
                size: case.size,
                modified_ms: case.modified_ms,
                accessed_ms: None,
                created_ms: None,
                mime_type: case.mime_type.map(str::to_owned),
                stage_state: IndexedEntryStageState {
                    metadata: EntryStageProgress::Complete,
                    content: EntryStageProgress::Complete,
                },
                content: None,
                extraction_status: ExtractionStatus::Unsupported,
                device: None,
                inode: None,
                mtime_ns: None,
                ctime_ns: None,
            })
            .unwrap();
        let mut query = SearchQuery::global(1, "");
        query.filters = case.filters.clone();

        let matched = !database.search(&query).unwrap().hits.is_empty();

        assert_eq!(matched, case.expected, "{}", case.label);
    }
}

#[test]
fn entry_facts_follow_the_filter_matrix() {
    for case in filter_cases() {
        let search_name = normalize_search_text(case.display_name);
        let entry = EntryFacts {
            search_name: &search_name,
            kind: case.kind,
            mime_type: case.mime_type,
            size: case.size,
            modified_ms: case.modified_ms,
            accessed_ms: None,
            created_ms: None,
        };
        assert_eq!(
            case.filters.matches(&entry),
            case.expected,
            "{}",
            case.label
        );
    }
}
