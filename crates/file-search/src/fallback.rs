use tokio_util::sync::CancellationToken;

use crate::config::SearchExcludeRules;
use crate::error::{SearchError, SearchResult};
use crate::filesystem::{
    display_name, ensure_not_cancelled, file_time_ms, mime_type_for_path, FilesystemObservation,
    LocalFilesystemBoundary, TraversalDepth, TraversalEvent,
};
use crate::model::{
    MatchSource, SearchFileKind, SearchHit, SearchMatchMode, SearchQuery, SearchScope, SizeRange,
    TimeRange,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryFallbackLimits {
    pub max_inspected_entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryFallbackCompletion {
    TraversalComplete { inspected_entries: usize },
    EntryBudgetReached { inspected_entries: usize },
}

pub fn search_directory_fallback(
    query: &SearchQuery,
    rules: &SearchExcludeRules,
    limits: DirectoryFallbackLimits,
    cancellation: &CancellationToken,
    mut emit_batch: impl FnMut(Vec<SearchHit>),
) -> SearchResult<DirectoryFallbackCompletion> {
    let SearchScope::Directory(root) = &query.scope else {
        return Err(SearchError::InvalidQuery(
            "directory fallback requires a directory scope".to_owned(),
        ));
    };
    if query.cursor.is_some() {
        return Err(SearchError::InvalidQuery(
            "directory fallback does not accept a cursor".to_owned(),
        ));
    }

    ensure_not_cancelled(cancellation)?;
    let boundary = match LocalFilesystemBoundary::observe(root, rules)? {
        FilesystemObservation::Complete(boundary) => boundary,
        FilesystemObservation::Inaccessible { scope } => {
            return Err(SearchError::Inaccessible {
                path: scope,
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            })
        }
        FilesystemObservation::Missing { scope } => {
            return Err(SearchError::Io {
                path: scope,
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
        }
        FilesystemObservation::PolicyExcluded { scope }
        | FilesystemObservation::DelegatedToNestedRoot { scope } => {
            return Err(SearchError::Io {
                path: scope,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "search root is excluded by filesystem policy",
                ),
            })
        }
    };
    let depth = if query.recursive {
        TraversalDepth::Recursive
    } else {
        TraversalDepth::DirectChildren
    };
    let mut walker = match boundary.walk_root(depth, cancellation)? {
        FilesystemObservation::Complete(walker) => walker,
        FilesystemObservation::Inaccessible { scope } => {
            return Err(SearchError::Inaccessible {
                path: scope,
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            })
        }
        FilesystemObservation::Missing { scope } => {
            return Err(SearchError::Io {
                path: scope,
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
        }
        FilesystemObservation::PolicyExcluded { scope }
        | FilesystemObservation::DelegatedToNestedRoot { scope } => {
            return Err(SearchError::Io {
                path: scope,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "search root is excluded by filesystem policy",
                ),
            })
        }
    };
    let terms = query
        .terms
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let name_regex = (query.match_mode == SearchMatchMode::Regex)
        .then(|| query.match_mode.name_regex(&query.terms))
        .transpose()?;
    let batch_size = query.limit.clamp(1, 200);
    let mut batch = Vec::with_capacity(batch_size);
    let mut inspected_entries = 0;

    loop {
        ensure_not_cancelled(cancellation)?;
        if inspected_entries == limits.max_inspected_entries {
            if !batch.is_empty() {
                emit_batch(batch);
                ensure_not_cancelled(cancellation)?;
            }
            return Ok(DirectoryFallbackCompletion::EntryBudgetReached { inspected_entries });
        }
        let Some(event) = walker.next_event()? else {
            break;
        };
        let TraversalEvent::Entry(entry) = event else {
            continue;
        };
        inspected_entries += 1;
        let name = display_name(entry.path());
        let folded_name = name.to_ascii_lowercase();
        let name_matches = match &name_regex {
            // 正则自行处理大小写折叠，不依赖 ASCII lowercase 视图。
            Some(name_regex) => name_regex.is_match(&name),
            None => terms.iter().all(|term| folded_name.contains(term)),
        };
        if !name_matches {
            continue;
        }

        let metadata = entry.metadata();
        let modified_ms = file_time_ms(metadata.modified().ok());
        let accessed_ms = file_time_ms(metadata.accessed().ok());
        let created_ms = file_time_ms(metadata.created().ok());
        let mime_type = if entry.kind() == SearchFileKind::File {
            mime_type_for_path(entry.path())
        } else {
            None
        };
        if !matches_filters(
            query,
            entry.path(),
            entry.kind(),
            mime_type.as_deref(),
            modified_ms,
            accessed_ms,
            created_ms,
            metadata.len(),
        ) {
            continue;
        }

        batch.push(SearchHit {
            path: entry.path().to_path_buf(),
            display_name: name,
            kind: entry.kind(),
            size: metadata.len(),
            modified_ms,
            accessed_ms,
            created_ms,
            rank: 0.0,
            snippet: None,
            match_source: if terms.is_empty() {
                MatchSource::Metadata
            } else {
                MatchSource::Name
            },
        });

        if batch.len() == batch_size {
            ensure_not_cancelled(cancellation)?;
            emit_batch(std::mem::replace(
                &mut batch,
                Vec::with_capacity(batch_size),
            ));
            ensure_not_cancelled(cancellation)?;
        }
    }

    ensure_not_cancelled(cancellation)?;
    if !batch.is_empty() {
        emit_batch(batch);
        ensure_not_cancelled(cancellation)?;
    }
    Ok(DirectoryFallbackCompletion::TraversalComplete { inspected_entries })
}

/// 八个事实全部来自同一次遍历条目的 metadata 快照，参数扁平但概念单一；
/// 收敛成结构体反而多一层只构造一次的中转。
#[allow(clippy::too_many_arguments)]
fn matches_filters(
    query: &SearchQuery,
    path: &std::path::Path,
    kind: SearchFileKind,
    mime_type: Option<&str>,
    modified_ms: Option<i64>,
    accessed_ms: Option<i64>,
    created_ms: Option<i64>,
    size: u64,
) -> bool {
    (query.filters.entry_type_rules.is_empty()
        || query
            .filters
            .entry_type_rules
            .iter()
            .any(|rule| entry_type_rule_matches(rule, kind, mime_type)))
        && extension_filter_matches(query, path, kind)
        && size_filter_matches(kind, size, query.filters.size)
        && time_matches(modified_ms, query.filters.modified)
        && time_matches(accessed_ms, query.filters.accessed)
        && time_matches(created_ms, query.filters.created)
}

/// 大小语义只作用于文件类条目：目录永不因大小被排除（与索引路径一致）。
fn size_filter_matches(kind: SearchFileKind, size: u64, range: Option<SizeRange>) -> bool {
    match range {
        Some(range) => {
            kind == SearchFileKind::Directory
                || (size >= range.min_bytes && range.max_bytes.is_none_or(|max| size < max))
        }
        None => true,
    }
}

/// 后缀语义只作用于文件类条目：目录即使名称以点后缀结尾也不匹配（与索引路径一致）。
fn extension_filter_matches(
    query: &SearchQuery,
    path: &std::path::Path,
    kind: SearchFileKind,
) -> bool {
    let extensions = &query.filters.extensions;
    extensions.is_empty()
        || (kind != SearchFileKind::Directory
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extensions
                        .iter()
                        .any(|token| extension.eq_ignore_ascii_case(token))
                }))
}

fn entry_type_rule_matches(
    rule: &crate::model::SearchEntryTypeRule,
    kind: SearchFileKind,
    mime_type: Option<&str>,
) -> bool {
    match rule {
        crate::model::SearchEntryTypeRule::Kind(expected) => *expected == kind,
        crate::model::SearchEntryTypeRule::Mime(pattern) => {
            mime_pattern_matches(pattern, mime_type)
        }
    }
}

fn mime_pattern_matches(pattern: &crate::model::MimePattern, mime_type: Option<&str>) -> bool {
    let Some(mime_type) = mime_type else {
        return false;
    };
    match pattern {
        crate::model::MimePattern::Exact(expected) => mime_type == expected,
        crate::model::MimePattern::Prefix(prefix) => mime_type.starts_with(prefix),
    }
}

fn time_matches(value: Option<i64>, range: Option<TimeRange>) -> bool {
    match range {
        Some(range) => value.is_some_and(|value| value >= range.start_ms && value <= range.end_ms),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use crate::config::SearchExcludeRules;
    use crate::error::SearchError;
    use crate::filesystem::file_time_ms;
    use crate::model::{
        MatchSource, SearchCursor, SearchEntryTypeRule, SearchFileKind, SearchFilters,
        SearchMatchMode, SearchQuery, SearchScope, SearchTextScope, SizeRange, TimeRange,
    };

    use super::{search_directory_fallback, DirectoryFallbackCompletion, DirectoryFallbackLimits};

    fn complete_traversal_limits() -> DirectoryFallbackLimits {
        DirectoryFallbackLimits {
            max_inspected_entries: usize::MAX,
        }
    }

    fn directory_query(root: PathBuf, terms: &str) -> SearchQuery {
        SearchQuery {
            query_id: 1,
            terms: terms.to_owned(),
            text_scope: SearchTextScope::NameAndContent,
            match_mode: SearchMatchMode::Plain,
            scope: SearchScope::Directory(root),
            recursive: true,
            filters: SearchFilters::default(),
            limit: 50,
            cursor: None,
        }
    }

    #[test]
    fn emits_every_name_match_in_bounded_batches() {
        let content = tempdir().unwrap();
        fs::write(content.path().join("Alpha Report 1.txt"), "one").unwrap();
        fs::write(content.path().join("alpha report 2.txt"), "two").unwrap();
        fs::write(content.path().join("alpha notes.txt"), "three").unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "ALPHA report");
        query.limit = 0;
        let mut batches = Vec::new();

        search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| batches.push(batch),
        )
        .unwrap();

        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|batch| batch.len() == 1));
        let mut names = batches
            .into_iter()
            .flatten()
            .map(|hit| hit.display_name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["Alpha Report 1.txt", "alpha report 2.txt"]);
    }

    #[test]
    fn extension_filters_match_files_case_insensitively_and_skip_directories() {
        let content = tempdir().unwrap();
        fs::write(content.path().join("report.PDF"), "one").unwrap();
        fs::write(content.path().join("notes.md"), "two").unwrap();
        fs::write(content.path().join("photo.jpg"), "three").unwrap();
        fs::create_dir(content.path().join("folder.pdf")).unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "");
        query.filters.extensions = vec!["pdf".to_owned(), "md".to_owned()];
        let mut batches = Vec::new();

        search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| batches.push(batch),
        )
        .unwrap();

        let mut names = batches
            .into_iter()
            .flatten()
            .map(|hit| hit.display_name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["notes.md", "report.PDF"]);
    }

    #[test]
    fn clamps_the_maximum_batch_size_to_two_hundred() {
        let content = tempdir().unwrap();
        for index in 0..201 {
            fs::write(content.path().join(format!("match-{index}.bin")), "match").unwrap();
        }
        let mut query = directory_query(content.path().to_path_buf(), "match");
        query.limit = usize::MAX;
        let mut batch_lengths = Vec::new();

        search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| batch_lengths.push(batch.len()),
        )
        .unwrap();

        assert_eq!(batch_lengths, vec![200, 1]);
    }

    #[test]
    fn regex_mode_matches_names_case_insensitively_and_reports_name_hits() {
        let content = tempdir().unwrap();
        fs::write(content.path().join("Alpha Report 1.txt"), "one").unwrap();
        fs::write(content.path().join("unrelated.txt"), "two").unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "^alpha \\w+ \\d\\.txt$");
        query.match_mode = SearchMatchMode::Regex;
        let mut batches = Vec::new();

        search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| batches.push(batch),
        )
        .unwrap();

        let hits = batches.into_iter().flatten().collect::<Vec<_>>();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "Alpha Report 1.txt");
        assert_eq!(hits[0].match_source, MatchSource::Name);
    }

    #[test]
    fn regex_mode_rejects_invalid_patterns_before_traversal() {
        let content = tempdir().unwrap();
        fs::write(content.path().join("note.txt"), "body").unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "foo(");
        query.match_mode = SearchMatchMode::Regex;

        let error = search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();

        assert!(matches!(error, SearchError::InvalidQuery(_)));
    }

    #[test]
    fn emits_matching_results_in_stable_batches() {
        let content = tempdir().unwrap();
        for index in 0..250 {
            fs::write(content.path().join(format!("match-{index}.bin")), "match").unwrap();
        }
        let mut query = directory_query(content.path().to_path_buf(), "match");
        query.limit = 100;
        let mut batch_lengths = Vec::new();

        let completion = search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| batch_lengths.push(batch.len()),
        )
        .unwrap();

        assert!(matches!(
            completion,
            DirectoryFallbackCompletion::TraversalComplete { .. }
        ));
        assert_eq!(batch_lengths, vec![100, 100, 50]);
    }

    #[test]
    fn applies_entry_type_or_and_time_filters_without_reading_content() {
        let content = tempdir().unwrap();
        let path = content.path().join("large.txt");
        let pdf_path = content.path().join("large.pdf");
        let directory_path = content.path().join("large-directory.txt");
        fs::write(&path, vec![b'x'; 4096]).unwrap();
        fs::write(&pdf_path, vec![b'y'; 2048]).unwrap();
        fs::create_dir(&directory_path).unwrap();
        let modified_ms = file_time_ms(fs::metadata(&path).unwrap().modified().ok()).unwrap();
        let pdf_modified_ms =
            file_time_ms(fs::metadata(&pdf_path).unwrap().modified().ok()).unwrap();
        let directory_modified_ms =
            file_time_ms(fs::metadata(&directory_path).unwrap().modified().ok()).unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "large");
        query.filters.entry_type_rules = vec![
            SearchEntryTypeRule::Kind(SearchFileKind::Directory),
            SearchEntryTypeRule::Mime(crate::model::MimePattern::Prefix("text/".to_owned())),
            SearchEntryTypeRule::Mime(crate::model::MimePattern::Exact(
                "application/pdf".to_owned(),
            )),
        ];
        query.filters.modified = Some(TimeRange {
            start_ms: modified_ms.min(pdf_modified_ms).min(directory_modified_ms),
            end_ms: modified_ms.max(pdf_modified_ms).max(directory_modified_ms),
        });
        let mut hits = Vec::new();

        search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |batch| hits.extend(batch),
        )
        .unwrap();

        let hits_by_path = hits
            .into_iter()
            .map(|hit| (hit.path.clone(), hit))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(hits_by_path.len(), 3);
        assert_eq!(hits_by_path[&path].size, 4096);
        assert_eq!(hits_by_path[&pdf_path].size, 2048);
        assert_eq!(
            hits_by_path[&directory_path].kind,
            SearchFileKind::Directory
        );
        assert!(hits_by_path.values().all(|hit| hit.snippet.is_none()));
    }

    #[test]
    fn fallback_applies_each_common_mime_pattern() {
        let content = tempdir().unwrap();
        let cases = [
            (
                "document.pdf",
                crate::model::MimePattern::Exact("application/pdf".to_owned()),
            ),
            (
                "image.png",
                crate::model::MimePattern::Prefix("image/".to_owned()),
            ),
            (
                "audio.flac",
                crate::model::MimePattern::Prefix("audio/".to_owned()),
            ),
            (
                "video.mp4",
                crate::model::MimePattern::Prefix("video/".to_owned()),
            ),
            (
                "archive.zip",
                crate::model::MimePattern::Exact("application/zip".to_owned()),
            ),
        ];
        for (file_name, _) in &cases {
            fs::write(content.path().join(file_name), "payload").unwrap();
        }

        for (expected_file_name, pattern) in cases {
            let mut query = directory_query(content.path().to_path_buf(), "");
            query.filters.entry_type_rules = vec![SearchEntryTypeRule::Mime(pattern)];
            let mut hits = Vec::new();
            search_directory_fallback(
                &query,
                &SearchExcludeRules::new(Vec::new()),
                complete_traversal_limits(),
                &CancellationToken::new(),
                |batch| hits.extend(batch),
            )
            .unwrap();

            assert_eq!(hits.len(), 1, "unexpected hits for {expected_file_name}");
            assert_eq!(
                hits[0].path.file_name().and_then(std::ffi::OsStr::to_str),
                Some(expected_file_name)
            );
        }
    }

    #[test]
    fn size_filters_apply_half_open_bounds_and_exempt_directories() {
        let content = tempdir().unwrap();
        fs::write(content.path().join("empty.bin"), "").unwrap();
        fs::write(content.path().join("one.bin"), "x").unwrap();
        fs::write(content.path().join("two.bin"), "xx").unwrap();
        fs::write(content.path().join("three.bin"), "xxx").unwrap();
        fs::write(content.path().join("four.bin"), "xxxx").unwrap();
        fs::create_dir(content.path().join("size-folder")).unwrap();

        let hit_names = |range: SizeRange| {
            let mut query = directory_query(content.path().to_path_buf(), "");
            query.filters.size = Some(range);
            let mut batches = Vec::new();
            search_directory_fallback(
                &query,
                &SearchExcludeRules::new(Vec::new()),
                complete_traversal_limits(),
                &CancellationToken::new(),
                |batch| batches.push(batch),
            )
            .unwrap();
            batches
                .into_iter()
                .flatten()
                .map(|hit| hit.display_name)
                .collect::<std::collections::BTreeSet<_>>()
        };

        // [2, 4)：two/three 命中；0、1、4 排除；目录永不被大小条件排除。
        let hits = hit_names(SizeRange {
            min_bytes: 2,
            max_bytes: Some(4),
        });
        assert!(hits.contains("size-folder"));
        assert_eq!(
            hits,
            ["size-folder", "three.bin", "two.bin"]
                .into_iter()
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
        );

        // 无上界：>= 2 的一切文件命中，目录仍豁免（其 metadata 大小不代表语义）。
        let hits = hit_names(SizeRange {
            min_bytes: 2,
            max_bytes: None,
        });
        assert!(hits.contains("size-folder"));
        assert!(hits.contains("four.bin"));
        assert!(!hits.contains("one.bin"));

        // 无下界缺省 0：包含 0 字节文件。
        let hits = hit_names(SizeRange {
            min_bytes: 0,
            max_bytes: Some(1),
        });
        assert!(hits.contains("size-folder"));
        assert!(hits.contains("empty.bin"));
        assert!(!hits.contains("two.bin"));
    }

    #[test]
    fn missing_time_value_does_not_match_a_selected_range() {
        assert!(!super::time_matches(
            None,
            Some(TimeRange {
                start_ms: 10,
                end_ms: 20,
            })
        ));
    }

    #[test]
    fn entry_budget_keeps_found_hits_and_has_a_distinct_completion() {
        let content = tempdir().unwrap();
        for index in 0..3 {
            fs::write(content.path().join(format!("entry-{index}.txt")), "entry").unwrap();
        }
        let query = directory_query(content.path().to_path_buf(), "");
        let rules = SearchExcludeRules::new(Vec::new());
        let mut hits = Vec::new();

        let completion = search_directory_fallback(
            &query,
            &rules,
            DirectoryFallbackLimits {
                max_inspected_entries: 2,
            },
            &CancellationToken::new(),
            |batch| hits.extend(batch),
        )
        .unwrap();

        assert_eq!(hits.len(), 2);
        assert_eq!(
            completion,
            DirectoryFallbackCompletion::EntryBudgetReached {
                inspected_entries: 2,
            }
        );

        let mut complete_hits = Vec::new();
        let complete = search_directory_fallback(
            &query,
            &rules,
            DirectoryFallbackLimits {
                max_inspected_entries: 4,
            },
            &CancellationToken::new(),
            |batch| complete_hits.extend(batch),
        )
        .unwrap();
        assert_eq!(complete_hits.len(), 3);
        assert_eq!(
            complete,
            DirectoryFallbackCompletion::TraversalComplete {
                inspected_entries: 3,
            }
        );
    }

    #[test]
    fn cancellation_before_and_after_the_first_batch_stops_traversal() {
        let content = tempdir().unwrap();
        for index in 0..4 {
            fs::write(content.path().join(format!("match-{index}.txt")), "match").unwrap();
        }
        let mut query = directory_query(content.path().to_path_buf(), "match");
        query.limit = 1;
        let rules = SearchExcludeRules::new(Vec::new());

        let pre_cancelled = CancellationToken::new();
        pre_cancelled.cancel();
        assert!(matches!(
            search_directory_fallback(
                &query,
                &rules,
                complete_traversal_limits(),
                &pre_cancelled,
                |_| {}
            ),
            Err(SearchError::Cancelled)
        ));

        let cancellation = CancellationToken::new();
        let cancellation_from_batch = cancellation.clone();
        let mut batches = 0;
        let error = search_directory_fallback(
            &query,
            &rules,
            complete_traversal_limits(),
            &cancellation,
            |_| {
                batches += 1;
                cancellation_from_batch.cancel();
            },
        )
        .unwrap_err();

        assert!(matches!(error, SearchError::Cancelled));
        assert_eq!(batches, 1);
    }

    #[test]
    fn rejects_global_scope_and_cursor_queries() {
        let rules = SearchExcludeRules::new(Vec::new());
        let cancellation = CancellationToken::new();
        assert!(matches!(
            search_directory_fallback(
                &SearchQuery::global(1, "name"),
                &rules,
                complete_traversal_limits(),
                &cancellation,
                |_| {}
            ),
            Err(SearchError::InvalidQuery(_))
        ));

        let content = tempdir().unwrap();
        let mut query = directory_query(content.path().to_path_buf(), "name");
        query.cursor = Some(SearchCursor { offset: 1 });
        assert!(matches!(
            search_directory_fallback(
                &query,
                &rules,
                complete_traversal_limits(),
                &cancellation,
                |_| {}
            ),
            Err(SearchError::InvalidQuery(_))
        ));
    }

    #[test]
    fn missing_root_returns_a_path_aware_io_error() {
        let content = tempdir().unwrap();
        let missing = content.path().join("missing");
        let query = directory_query(missing.clone(), "name");

        let error = search_directory_fallback(
            &query,
            &SearchExcludeRules::new(Vec::new()),
            complete_traversal_limits(),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();

        assert!(matches!(error, SearchError::Io { path, .. } if path == missing));
    }
}
