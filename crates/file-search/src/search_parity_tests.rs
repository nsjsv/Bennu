//! 共享语料：同一批文件既经正式写入路径进索引，又留在磁盘上给本地回退；同一组查询两边
//! 执行，命中集合必须相同（名称、大小写、变音、全角、中文、1–2 个字、多段后缀、大小与时间）。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::config::SearchExcludeRules;
use crate::database::{EntryStageProgress, IndexedEntryStageState, IndexedFile, SearchDatabase};
use crate::extractor::ExtractionStatus;
use crate::fallback::{search_directory_fallback, DirectoryFallbackLimits};
use crate::filesystem::{display_name, file_time_ms, mime_type_for_path};
use crate::model::{
    SearchFileKind, SearchFilters, SearchMatchMode, SearchQuery, SearchScope, SearchTextScope,
    SizeRange, TimeRange,
};

const CORPUS: &[(&str, usize)] = &[
    ("年度报告.pdf", 10),
    ("docs/季度报告摘要.txt", 20),
    ("report_2024.pdf", 30),
    ("MyReport.docx", 40),
    ("Ärger.md", 0),
    ("ＲＥＰＯＲＴ２０２５.txt", 50),
    ("backup.tar.gz", 60),
    ("archive.tar", 70),
    ("notes.gz", 80),
    ("docs/Crème brûlée.txt", 5),
];

fn index_directory(database: &SearchDatabase, directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let kind = if metadata.is_dir() {
            SearchFileKind::Directory
        } else {
            SearchFileKind::File
        };
        database
            .upsert_file(&IndexedFile {
                path: path.clone(),
                parent_path: path.parent().unwrap().to_path_buf(),
                display_name: display_name(&path),
                kind,
                size: metadata.len(),
                modified_ms: file_time_ms(metadata.modified().ok()),
                accessed_ms: file_time_ms(metadata.accessed().ok()),
                created_ms: file_time_ms(metadata.created().ok()),
                mime_type: (kind == SearchFileKind::File)
                    .then(|| mime_type_for_path(&path))
                    .flatten(),
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
        if kind == SearchFileKind::Directory {
            index_directory(database, &path);
        }
    }
}

fn directory_query(root: &Path, terms: &str, filters: SearchFilters) -> SearchQuery {
    SearchQuery {
        query_id: 1,
        terms: terms.to_owned(),
        text_scope: SearchTextScope::NameOnly,
        match_mode: SearchMatchMode::Plain,
        scope: SearchScope::Directory(root.to_path_buf()),
        recursive: true,
        filters,
        limit: 200,
        cursor: None,
    }
}

fn index_names(database: &SearchDatabase, query: &SearchQuery) -> BTreeSet<String> {
    database
        .search(query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.display_name)
        .collect()
}

fn fallback_names(query: &SearchQuery) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    search_directory_fallback(
        query,
        &SearchExcludeRules::new(Vec::new()),
        DirectoryFallbackLimits {
            max_inspected_entries: usize::MAX,
        },
        &CancellationToken::new(),
        |batch| names.extend(batch.into_iter().map(|hit| hit.display_name)),
    )
    .unwrap();
    names
}

#[test]
fn index_and_fallback_return_the_same_hits_for_the_same_query() {
    let corpus = tempdir().unwrap();
    for (relative, size) in CORPUS {
        let path = corpus.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![b'x'; *size]).unwrap();
    }
    let database = SearchDatabase::in_memory().unwrap();
    index_directory(&database, corpus.path());
    let modified_ms = file_time_ms(
        fs::metadata(corpus.path().join("年度报告.pdf"))
            .unwrap()
            .modified()
            .ok(),
    )
    .unwrap();

    let extensions = |tokens: &[&str]| SearchFilters {
        extensions: tokens.iter().map(|token| (*token).to_owned()).collect(),
        ..SearchFilters::default()
    };
    let cases = [
        ("报告", SearchFilters::default()),
        ("报", SearchFilters::default()),
        ("repo", SearchFilters::default()),
        ("REPORT", SearchFilters::default()),
        ("report 2025", SearchFilters::default()),
        ("arger", SearchFilters::default()),
        ("creme", SearchFilters::default()),
        ("t_2", SearchFilters::default()),
        ("docs", SearchFilters::default()),
        ("", extensions(&["tar.gz"])),
        ("", extensions(&["gz"])),
        ("", extensions(&["TAR"])),
        (
            "",
            SearchFilters {
                size: Some(SizeRange {
                    min_bytes: 1,
                    max_bytes: Some(40),
                }),
                ..SearchFilters::default()
            },
        ),
        (
            "",
            SearchFilters {
                modified: Some(TimeRange {
                    start_ms: modified_ms - 60_000,
                    end_ms: modified_ms + 60_000,
                }),
                ..SearchFilters::default()
            },
        ),
    ];
    for (terms, filters) in cases {
        let mut filters = filters;
        filters.extensions = filters
            .extensions
            .iter()
            .flat_map(|token| crate::model::normalize_extension_tokens(token).unwrap())
            .collect();
        let query = directory_query(corpus.path(), terms, filters);

        let index = index_names(&database, &query);
        let fallback = fallback_names(&query);

        assert!(
            !index.is_empty() || terms == "report 2025",
            "{terms:?} found nothing"
        );
        assert_eq!(index, fallback, "{terms:?} {:?}", query.filters);
    }
}
